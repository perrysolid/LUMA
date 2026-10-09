//! Gemini Live (native audio) "fast mode": one websocket per turn carries
//! the user's voice in and LUMA's voice out, with no separate STT or TTS hop.
//!
//! Push-to-talk maps onto manual activity detection: `activityStart` when the
//! shortcut goes down, `activityEnd` when it comes up. The screenshot goes in
//! as a video frame. Native audio cannot carry inline tags, so drawing is a
//! `draw` function (non-blocking) whose arguments mirror the tag vocabulary;
//! the app resolves each call with the same `Resolver` as the cascade.

use anyhow::{anyhow, Result};
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

pub const OUTPUT_RATE: u32 = 24_000;

#[derive(Debug)]
pub enum LiveEvent {
    /// PCM16 mono at `OUTPUT_RATE`.
    Audio(Vec<i16>),
    /// Transcript of what LUMA is saying (incremental).
    OutputText(String),
    /// Transcript of what the user said (incremental).
    InputText(String),
    ToolCall { id: String, name: String, args: Value },
    TurnComplete,
    Interrupted,
    Error(String),
}

#[derive(Clone)]
pub struct LiveConfig {
    pub api_key: String,
    pub model: String,
    /// System prompt plus the turn's context block.
    pub system: String,
    /// Offer the `draw` function.
    pub draw_tool: bool,
}

/// Handle for sending to the session; dropping it closes the socket.
pub struct LiveSession {
    tx: mpsc::UnboundedSender<Message>,
}

/// The `draw` function: one call per mark, arguments as in the tag protocol.
pub fn draw_tool() -> Value {
    json!({
        "functionDeclarations": [{
            "name": "draw",
            "description": "Draw one mark on the user's screen, right before you talk about that element. Coordinates are box=\"ymin xmin ymax xmax\" integers 0-1000 relative to the latest screen image.",
            "behavior": "NON_BLOCKING",
            "parameters": {
                "type": "OBJECT",
                "properties": {
                    "kind": { "type": "STRING", "enum": ["box", "circle", "highlight", "underline", "point", "arrow", "step", "label", "spotlight", "zoom", "focus", "clear", "board", "node", "sketch"] },
                    "id": { "type": "STRING", "description": "Short meaningful id, reused across the conversation" },
                    "box": { "type": "STRING", "description": "ymin xmin ymax xmax, 0-1000" },
                    "label": { "type": "STRING", "description": "1-4 word label" },
                    "from": { "type": "STRING", "description": "arrow start: an id" },
                    "to": { "type": "STRING", "description": "arrow end: an id" },
                    "target": { "type": "STRING", "description": "id of an earlier mark (step, label, spotlight, zoom, focus, clear)" },
                    "n": { "type": "INTEGER", "description": "step number" },
                    "text": { "type": "STRING", "description": "label or node text" },
                    "title": { "type": "STRING", "description": "board title" },
                    "path": { "type": "STRING", "description": "sketch: points as \"y x; y x; ...\", 0-1000" },
                    "closed": { "type": "STRING", "description": "sketch: \"true\" to close the loop" },
                    "color": { "type": "STRING", "description": "sketch colour to match the drawing: green, blue, purple, orange, pink, yellow, white, red" }
                },
                "required": ["kind"]
            }
        }]
    })
}

pub fn setup_message(cfg: &LiveConfig) -> Value {
    let mut setup = json!({
        "model": format!("models/{}", cfg.model),
        "generationConfig": { "responseModalities": ["AUDIO"] },
        "systemInstruction": { "parts": [{ "text": cfg.system }] },
        "realtimeInputConfig": { "automaticActivityDetection": { "disabled": true } },
        "inputAudioTranscription": {},
        "outputAudioTranscription": {}
    });
    if cfg.draw_tool {
        setup["tools"] = json!([draw_tool()]);
    }
    json!({ "setup": setup })
}

/// Connect and complete setup (≤ 5 s).
pub async fn open(cfg: &LiveConfig) -> Result<(LiveSession, mpsc::UnboundedReceiver<LiveEvent>)> {
    if crate::route::proxy().is_some() {
        return Err(anyhow!("fast mode is not available through the LUMA proxy"));
    }
    let url = format!(
        "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent?key={}",
        cfg.api_key
    );
    let (ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(url))
        .await
        .map_err(|_| anyhow!("Gemini Live connection timed out"))?
        .map_err(|e| anyhow!("Gemini Live connection failed: {}", crate::assemblyai::redact(&e.to_string())))?;
    let (mut sink, mut stream) = ws.split();
    sink.send(Message::Text(setup_message(cfg).to_string().into())).await?;

    // wait for setupComplete
    let ready = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(m) = stream.next().await {
            let v = match m? {
                Message::Text(t) => serde_json::from_str::<Value>(&t).ok(),
                Message::Binary(b) => serde_json::from_slice::<Value>(&b).ok(),
                Message::Close(f) => return Err(anyhow!("Gemini Live closed during setup: {}", f.map(|f| f.reason.to_string()).unwrap_or_default())),
                _ => None,
            };
            if v.as_ref().is_some_and(|v| v.get("setupComplete").is_some()) {
                return Ok(());
            }
        }
        Err(anyhow!("Gemini Live closed during setup"))
    })
    .await
    .map_err(|_| anyhow!("Gemini Live setup timed out"))?;
    ready?;

    let (tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(m) = stream.next().await {
            let v = match m {
                Ok(Message::Text(t)) => serde_json::from_str::<Value>(&t).ok(),
                Ok(Message::Binary(b)) => serde_json::from_slice::<Value>(&b).ok(),
                Ok(Message::Close(f)) => {
                    if let Some(f) = f.filter(|f| u16::from(f.code) != 1000) {
                        let _ = ev_tx.send(LiveEvent::Error(format!("Gemini Live closed: {} {}", u16::from(f.code), f.reason)));
                    }
                    break;
                }
                Ok(_) => None,
                Err(e) => {
                    let _ = ev_tx.send(LiveEvent::Error(e.to_string()));
                    break;
                }
            };
            let Some(v) = v else { continue };
            for e in parse_server(&v) {
                if ev_tx.send(e).is_err() {
                    return;
                }
            }
        }
    });
    Ok((LiveSession { tx }, ev_rx))
}

/// Events in one server message.
pub fn parse_server(v: &Value) -> Vec<LiveEvent> {
    let mut out = Vec::new();
    if let Some(sc) = v.get("serverContent") {
        if let Some(parts) = sc.pointer("/modelTurn/parts").and_then(Value::as_array) {
            for p in parts {
                if let Some(d) = p.pointer("/inlineData/data").and_then(Value::as_str) {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(d) {
                        out.push(LiveEvent::Audio(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()));
                    }
                }
            }
        }
        if let Some(t) = sc.pointer("/outputTranscription/text").and_then(Value::as_str) {
            out.push(LiveEvent::OutputText(t.to_string()));
        }
        if let Some(t) = sc.pointer("/inputTranscription/text").and_then(Value::as_str) {
            out.push(LiveEvent::InputText(t.to_string()));
        }
        if sc.get("interrupted").and_then(Value::as_bool) == Some(true) {
            out.push(LiveEvent::Interrupted);
        }
        if sc.get("turnComplete").and_then(Value::as_bool) == Some(true) {
            out.push(LiveEvent::TurnComplete);
        }
    }
    if let Some(calls) = v.pointer("/toolCall/functionCalls").and_then(Value::as_array) {
        for c in calls {
            out.push(LiveEvent::ToolCall {
                id: c.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                name: c.get("name").and_then(Value::as_str).unwrap_or_default().to_string(),
                args: c.get("args").cloned().unwrap_or(Value::Null),
            });
        }
    }
    if let Some(e) = v.get("error") {
        out.push(LiveEvent::Error(e.to_string()));
    }
    out
}

impl LiveSession {
    fn send(&self, v: Value) -> bool {
        self.tx.send(Message::Text(v.to_string().into())).is_ok()
    }

    /// A screen image (JPEG) as a video frame.
    pub fn send_image(&self, jpeg: &[u8]) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(jpeg);
        self.send(json!({ "realtimeInput": { "video": { "data": data, "mimeType": "image/jpeg" } } }))
    }

    /// The user started talking (shortcut down).
    pub fn activity_start(&self) -> bool {
        self.send(json!({ "realtimeInput": { "activityStart": {} } }))
    }

    /// 16 kHz mono PCM16 little-endian.
    pub fn send_audio(&self, pcm16le: &[u8]) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(pcm16le);
        self.send(json!({ "realtimeInput": { "audio": { "data": data, "mimeType": "audio/pcm;rate=16000" } } }))
    }

    /// The user stopped talking (shortcut up): the model answers now.
    pub fn activity_end(&self) -> bool {
        self.send(json!({ "realtimeInput": { "activityEnd": {} } }))
    }

    /// The screen image as part of the conversation (full resolution, unlike
    /// video frames), optionally with a typed question that ends the turn.
    pub fn send_image_turn(&self, jpeg: &[u8], question: Option<&str>) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let mut parts = vec![json!({ "inlineData": { "mimeType": "image/jpeg", "data": data } })];
        if let Some(q) = question {
            parts.push(json!({ "text": q }));
        }
        self.send(json!({ "clientContent": { "turns": [{ "role": "user", "parts": parts }], "turnComplete": question.is_some() } }))
    }

    /// The model sometimes ends a turn without saying anything; ask again.
    pub fn nudge(&self) -> bool {
        self.send_text("(Answer the user's question now, out loud.)")
    }

    /// A typed question instead of speech.
    pub fn send_text(&self, text: &str) -> bool {
        self.send(json!({ "realtimeInput": { "text": text } }))
    }

    /// Acknowledge a draw call. A non-blocking call can end the model turn,
    /// so WHEN_IDLE lets the model carry on talking once it is drawn.
    pub fn tool_response(&self, id: &str, name: &str) -> bool {
        self.send(json!({ "toolResponse": { "functionResponses": [{ "id": id, "name": name, "response": { "result": "drawn; now say it" }, "scheduling": "WHEN_IDLE" }] } }))
    }
}

/// A `draw` call as a tag for the shared resolver.
pub fn draw_call_to_tag(args: &Value) -> Option<luma_core::markup::Tag> {
    let obj = args.as_object()?;
    let kind = obj.get("kind")?.as_str()?.trim().to_lowercase();
    if !luma_core::markup::KNOWN_TAGS.contains(&kind.as_str()) || kind == "task" || kind == "lesson" {
        return None;
    }
    let attrs = obj
        .iter()
        .filter(|(k, _)| k.as_str() != "kind")
        .filter_map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Array(a) => a.iter().filter_map(|x| x.as_f64()).map(|f| f.to_string()).collect::<Vec<_>>().join(" "),
                _ => return None,
            };
            Some((k.clone(), s))
        })
        .collect();
    Some(luma_core::markup::Tag { name: kind, attrs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_uses_push_to_talk_and_the_draw_tool() {
        let s = setup_message(&LiveConfig { api_key: "k".into(), model: "gemini-3.8-live".into(), system: "sys".into(), draw_tool: true });
        assert_eq!(s["setup"]["model"], "models/gemini-3.8-live");
        assert_eq!(s["setup"]["realtimeInputConfig"]["automaticActivityDetection"]["disabled"], true);
        assert_eq!(s["setup"]["tools"][0]["functionDeclarations"][0]["name"], "draw");
        assert!(s.to_string().contains("outputAudioTranscription"));
    }

    #[test]
    fn server_messages_become_events() {
        let pcm = base64::engine::general_purpose::STANDARD.encode([1u8, 0, 255, 255]);
        let v = json!({ "serverContent": { "modelTurn": { "parts": [{ "inlineData": { "mimeType": "audio/pcm;rate=24000", "data": pcm } }] }, "outputTranscription": { "text": "Here" } } });
        let ev = parse_server(&v);
        assert!(matches!(&ev[0], LiveEvent::Audio(s) if s == &vec![1, -1]));
        assert!(matches!(&ev[1], LiveEvent::OutputText(t) if t == "Here"));
        let v = json!({ "toolCall": { "functionCalls": [{ "id": "c1", "name": "draw", "args": { "kind": "box", "box": "1 2 3 4", "id": "share", "label": "Share" } }] } });
        match &parse_server(&v)[0] {
            LiveEvent::ToolCall { id, args, .. } => {
                assert_eq!(id, "c1");
                let tag = draw_call_to_tag(args).unwrap();
                assert_eq!(tag.name, "box");
                assert_eq!(tag.attr("box"), Some("1 2 3 4"));
                assert_eq!(tag.attr("label"), Some("Share"));
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(parse_server(&json!({ "serverContent": { "turnComplete": true } }))[0], LiveEvent::TurnComplete));
        assert!(draw_call_to_tag(&json!({ "kind": "task", "goal": "x" })).is_none());
        let t = draw_call_to_tag(&json!({ "kind": "step", "n": 2, "box": [10, 20, 30, 40] })).unwrap();
        assert_eq!((t.attr("n"), t.attr("box")), (Some("2"), Some("10 20 30 40")));
    }
}
