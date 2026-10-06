//! Gemini `streamGenerateContent` client (SSE).

use anyhow::{anyhow, Result};
use base64::Engine;
use futures_util::StreamExt;
use luma_core::sse::{parse_gemini_chunk, SseParser};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct Gemini {
    pub client: reqwest::Client,
    pub api_key: String,
    pub model: String,
    pub thinking_level: String,
}

pub struct ImagePart<'a> {
    pub jpeg: &'a [u8],
}

pub struct HistoryTurn<'a> {
    pub user: &'a str,
    pub assistant: &'a str,
}

impl Gemini {
    pub fn build_body(
        &self,
        system: &str,
        history: &[HistoryTurn],
        context: &str,
        images: &[ImagePart],
        user: &str,
    ) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        for t in history {
            contents.push(json!({"role": "user", "parts": [{"text": t.user}]}));
            contents.push(json!({"role": "model", "parts": [{"text": t.assistant}]}));
        }
        let mut parts = vec![json!({"text": context})];
        for img in images {
            parts.push(json!({"inlineData": {
                "mimeType": "image/jpeg",
                "data": base64::engine::general_purpose::STANDARD.encode(img.jpeg),
            }}));
        }
        parts.push(json!({"text": format!("The user says: \"{user}\"")}));
        contents.push(json!({"role": "user", "parts": parts}));
        json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": contents,
            "generationConfig": {
                "thinkingConfig": {"thinkingLevel": self.thinking_level},
                "mediaResolution": "MEDIA_RESOLUTION_HIGH",
                "maxOutputTokens": 4096,
            },
        })
    }

    /// Streams visible answer text into `out`. Returns when the response ends.
    pub async fn stream(&self, body: &Value, out: mpsc::UnboundedSender<String>) -> Result<()> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse",
            self.model
        );
        let resp = self
            .client
            .post(url)
            .header("x-goog-api-key", &self.api_key)
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow!("couldn't reach Gemini: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let t = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&t)
                .ok()
                .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_else(|| t.chars().take(200).collect());
            return Err(anyhow!("Gemini {status}: {msg}"));
        }
        let mut sse = SseParser::new();
        let mut stream = resp.bytes_stream();
        let handle = |payload: String| -> Result<()> {
            let d = parse_gemini_chunk(&payload);
            if let Some(e) = d.error {
                return Err(anyhow!("Gemini: {e}"));
            }
            if let Some(b) = d.block_reason {
                return Err(anyhow!("Gemini declined to answer ({b})"));
            }
            if !d.text.is_empty() {
                let _ = out.send(d.text);
            }
            Ok(())
        };
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow!("Gemini stream interrupted: {e}"))?;
            for payload in sse.push(&chunk) {
                handle(payload)?;
            }
        }
        if let Some(p) = sse.finish() {
            handle(p)?;
        }
        Ok(())
    }
}
