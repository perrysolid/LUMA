//! Sarvam Bulbul text-to-speech (REST, one request per utterance).
//!
//! Per-utterance requests give exact sentence↔audio alignment, which is what
//! lets annotations appear in sync with speech. Requests are pipelined, so
//! only the first sentence's latency is ever audible.

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone)]
pub struct SarvamTts {
    pub client: reqwest::Client,
    pub api_key: String,
    pub speaker: String,
    pub language: String,
    pub pace: f32,
}

#[derive(Deserialize)]
struct Resp {
    audios: Vec<String>,
}

impl SarvamTts {
    /// Returns a WAV clip.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        let body = json!({
            "text": text,
            "language_code": self.language,
            "speaker": self.speaker,
            "model": "bulbul:v3",
            "pace": self.pace,
            "speech_sample_rate": 24000,
            "enable_preprocessing": true,
        });
        let r = self
            .client
            .post("https://api.sarvam.ai/text-to-speech")
            .header("api-subscription-key", &self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = r.status();
        if !status.is_success() {
            let t = r.text().await.unwrap_or_default();
            return Err(anyhow!("Sarvam TTS {status}: {}", t.chars().take(300).collect::<String>()));
        }
        let resp: Resp = r.json().await?;
        let b64 = resp.audios.into_iter().next().ok_or_else(|| anyhow!("Sarvam returned no audio"))?;
        Ok(base64::engine::general_purpose::STANDARD.decode(b64)?)
    }
}
