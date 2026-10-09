//! AssemblyAI Universal streaming speech-to-text (v3 websocket).
//!
//! Push-to-talk flow: connect while the user starts speaking (audio is
//! buffered in the channel meanwhile), stream 100 ms chunks, and when the key
//! is released send `ForceEndpoint` so the final formatted transcript arrives
//! immediately instead of waiting for silence detection.

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

/// Hands-free: give up if the user says nothing for this long.
pub const HANDS_FREE_SILENCE: Duration = Duration::from_millis(5000);

#[derive(Debug, Clone)]
pub enum SttEvent {
    Partial(String),
}

#[derive(Deserialize)]
struct ServerMsg {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    transcript: String,
    #[serde(default)]
    end_of_turn: bool,
    #[serde(default)]
    turn_order: u32,
    #[serde(default)]
    error: Option<String>,
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// An open streaming session, ready for audio. Opening one ahead of time
/// ("warm") removes the connection setup from the next turn.
pub struct SttConn {
    ws: Ws,
    hands_free: bool,
    pub opened: std::time::Instant,
}

impl SttConn {
    pub fn hands_free(&self) -> bool {
        self.hands_free
    }
}

pub async fn connect(api_key: &str, model: &str, hands_free: bool) -> Result<SttConn> {
    let url = format!(
        "wss://streaming.assemblyai.com/v3/ws?sample_rate={}&encoding=pcm_s16le&speech_model={}&max_turn_silence={}",
        crate::STT_RATE,
        model,
        if hands_free { 1400 } else { 4000 }
    );
    // Through a key proxy: a short-lived token instead of the account key.
    let req = match crate::route::assemblyai_token().await? {
        Some(token) => format!("{url}&token={token}").into_client_request()?,
        None => {
            let mut r = url.into_client_request()?;
            r.headers_mut().insert("Authorization", HeaderValue::from_str(api_key)?);
            r
        }
    };
    let (ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| anyhow!("speech-to-text connection timed out"))?
        .map_err(|e| anyhow!("speech-to-text connection failed: {}", redact(&e.to_string())))?;
    Ok(SttConn { ws, hands_free, opened: std::time::Instant::now() })
}

/// Push-to-talk: streams `audio` until the channel closes (key released),
/// then returns the full transcript.
///
/// Hands-free (`hands_free = true`): returns at the first natural end of
/// turn, or after `HANDS_FREE_SILENCE` with no speech at all (empty string).
pub async fn transcribe(
    api_key: &str,
    model: &str,
    audio: mpsc::Receiver<Vec<u8>>,
    events: mpsc::UnboundedSender<SttEvent>,
    hands_free: bool,
) -> Result<String> {
    let conn = connect(api_key, model, hands_free).await?;
    transcribe_on(conn, audio, events).await
}

/// Stream a turn over an already open session.
pub async fn transcribe_on(
    conn: SttConn,
    mut audio: mpsc::Receiver<Vec<u8>>,
    events: mpsc::UnboundedSender<SttEvent>,
) -> Result<String> {
    let hands_free = conn.hands_free;
    let ws = conn.ws;
    let started = std::time::Instant::now();
    let mut heard_anything = false;
    let (mut tx, mut rx) = ws.split();

    let writer = async move {
        while let Some(chunk) = audio.recv().await {
            tx.send(Message::Binary(chunk.into())).await?;
        }
        tx.send(Message::Text(r#"{"type":"ForceEndpoint"}"#.into())).await?;
        anyhow::Ok(tx)
    };

    // Final transcript = finished turns in order, plus the live tail.
    let mut finals: Vec<(u32, String)> = Vec::new();
    let mut tail = String::new();
    let mut writer_done = false;
    let mut tx_back = None;
    tokio::pin!(writer);
    loop {
        tokio::select! {
            w = &mut writer, if !writer_done => {
                writer_done = true;
                tx_back = Some(w?);
            }
            _ = tokio::time::sleep(Duration::from_millis(250)), if hands_free => {
                if !heard_anything && started.elapsed() > HANDS_FREE_SILENCE {
                    break;
                }
                if started.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
            msg = async {
                if writer_done {
                    // After release, wait briefly for the forced final turn.
                    tokio::time::timeout(Duration::from_millis(2500), rx.next()).await.ok().flatten()
                } else {
                    rx.next().await
                }
            } => {
                let Some(msg) = msg else { break };
                let text = match msg? {
                    Message::Text(t) => t,
                    Message::Close(f) => {
                        if let Some(f) = f.filter(|f| u16::from(f.code) >= 4000) {
                            return Err(anyhow!("speech-to-text closed: {} {}", u16::from(f.code), f.reason));
                        }
                        break;
                    }
                    _ => continue,
                };
                let Ok(m) = serde_json::from_str::<ServerMsg>(&text) else { continue };
                if let Some(e) = m.error {
                    return Err(anyhow!("speech-to-text error: {e}"));
                }
                match m.kind.as_str() {
                    "Turn" if m.end_of_turn => {
                        finals.retain(|(o, _)| *o != m.turn_order);
                        finals.push((m.turn_order, m.transcript.trim().to_string()));
                        tail.clear();
                        let has_words = !m.transcript.trim().is_empty();
                        heard_anything |= has_words;
                        if writer_done || (hands_free && has_words) {
                            break;
                        }
                    }
                    "Turn" => {
                        tail = m.transcript.trim().to_string();
                        heard_anything |= !tail.is_empty();
                    }
                    "Termination" => break,
                    _ => {}
                }
                let mut live: Vec<&str> = finals.iter().map(|(_, t)| t.as_str()).collect();
                if !tail.is_empty() {
                    live.push(&tail);
                }
                let _ = events.send(SttEvent::Partial(live.join(" ")));
            }
        }
    }
    if let Some(mut tx) = tx_back {
        let _ = tx.send(Message::Text(r#"{"type":"Terminate"}"#.into())).await;
        let _ = tx.close().await;
    }
    finals.sort_by_key(|(o, _)| *o);
    let mut parts: Vec<String> = finals.into_iter().map(|(_, t)| t).filter(|t| !t.is_empty()).collect();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Ok(parts.join(" "))
}

/// Never let a key that ended up in an error string reach logs or the UI.
pub fn redact(s: &str) -> String {
    s.split_whitespace()
        .map(|w| if w.len() >= 24 && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { "[redacted]" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}
