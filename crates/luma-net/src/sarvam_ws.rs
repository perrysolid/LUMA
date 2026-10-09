//! Sarvam Bulbul streaming TTS over a websocket.
//!
//! ~200 ms to first audio versus ~1.2 s for a REST request per sentence.
//! One connection per turn (opened while the user is still talking).
//! Utterances are synthesized strictly in order: each `say` sends the text
//! plus a flush, and the server answers with audio chunks followed by a
//! `final` event, so audio is unambiguously attributed to its sentence.

use anyhow::{anyhow, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

pub const SAMPLE_RATE: u32 = 24_000;

#[derive(Debug)]
pub enum TtsEvent {
    /// PCM16 mono samples at `SAMPLE_RATE`.
    Audio(Vec<i16>),
    /// The current utterance is fully synthesized.
    End,
    Error(String),
}

#[derive(Clone)]
pub struct TtsConfig {
    pub api_key: String,
    pub speaker: String,
    pub language: String,
    pub pace: f32,
}

/// Handle for sending utterances; dropping it closes the stream.
pub struct TtsStream {
    tx: mpsc::UnboundedSender<String>,
}

impl TtsStream {
    pub fn say(&self, text: &str) -> bool {
        self.tx.send(text.to_string()).is_ok()
    }
}

pub async fn open(cfg: &TtsConfig) -> Result<(TtsStream, mpsc::UnboundedReceiver<TtsEvent>)> {
    if crate::route::proxy().is_some() {
        return Err(anyhow!("streaming voice is not available through the LUMA proxy (using REST)"));
    }
    let mut req = "wss://api.sarvam.ai/text-to-speech/ws?model=bulbul:v3&send_completion_event=true".into_client_request()?;
    req.headers_mut().insert("Api-Subscription-Key", HeaderValue::from_str(&cfg.api_key)?);
    let (ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(req))
        .await
        .map_err(|_| anyhow!("voice connection timed out"))?
        .map_err(|e| anyhow!("voice connection failed: {e}"))?;
    let (mut sink, mut stream) = ws.split();
    let config = json!({"type": "config", "data": {
        "speaker": cfg.speaker,
        "target_language_code": cfg.language,
        "language_code": cfg.language,
        "pace": cfg.pace,
        "output_audio_codec": "linear16",
        "speech_sample_rate": SAMPLE_RATE,
        "min_buffer_size": 30,
        "max_chunk_length": 200,
    }});
    sink.send(Message::Text(config.to_string().into())).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<TtsEvent>();

    tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            let t = json!({"type": "text", "data": {"text": text}}).to_string();
            if sink.send(Message::Text(t.into())).await.is_err()
                || sink.send(Message::Text(r#"{"type":"flush"}"#.into())).await.is_err()
            {
                break;
            }
        }
        let _ = sink.close().await;
    });
    tokio::spawn(async move {
        while let Some(msg) = stream.next().await {
            let text = match msg {
                Ok(Message::Text(t)) => t,
                Ok(Message::Close(_)) | Err(_) => break,
                _ => continue,
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            let ev = match v.get("type").and_then(Value::as_str) {
                Some("audio") => {
                    let b64 = v.pointer("/data/audio").and_then(Value::as_str).unwrap_or("");
                    match base64::engine::general_purpose::STANDARD.decode(b64) {
                        Ok(bytes) => TtsEvent::Audio(pcm16(&bytes)),
                        Err(_) => continue,
                    }
                }
                Some("event") if v.pointer("/data/event_type").and_then(Value::as_str) == Some("final") => TtsEvent::End,
                Some("error") => TtsEvent::Error(
                    v.pointer("/data/message").and_then(Value::as_str).unwrap_or("voice error").to_string(),
                ),
                _ => continue,
            };
            if ev_tx.send(ev).is_err() {
                break;
            }
        }
    });
    Ok((TtsStream { tx }, ev_rx))
}

/// Raw little-endian PCM16 (tolerates a WAV header if one ever appears).
pub fn pcm16(bytes: &[u8]) -> Vec<i16> {
    let data = if bytes.len() > 44 && &bytes[0..4] == b"RIFF" {
        bytes.windows(4).position(|w| w == b"data").map(|p| &bytes[p + 8..]).unwrap_or(&bytes[44..])
    } else {
        bytes
    };
    data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

#[cfg(test)]
mod tests {
    use super::pcm16;

    #[test]
    fn pcm_decoding() {
        assert_eq!(pcm16(&[1, 0, 255, 255, 7]), vec![1, -1]);
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.extend([0u8; 20]);
        wav.extend(b"data\x04\0\0\0");
        wav.extend([2, 0, 3, 0]);
        assert_eq!(pcm16(&wav), vec![2, 3]);
    }
}
