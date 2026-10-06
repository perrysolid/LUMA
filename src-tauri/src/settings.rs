//! API keys live in the OS credential store (macOS Keychain / Windows
//! Credential Manager). Non-secret preferences live in a JSON file in the app
//! config directory. Keys are never logged, never sent to the webview, and
//! never written to disk by LUMA itself.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SERVICE: &str = "com.luma.companion";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    Assemblyai,
    Sarvam,
}

impl Provider {
    pub const ALL: [Provider; 3] = [Provider::Gemini, Provider::Assemblyai, Provider::Sarvam];

    fn account(self) -> &'static str {
        match self {
            Provider::Gemini => "gemini-api-key",
            Provider::Assemblyai => "assemblyai-api-key",
            Provider::Sarvam => "sarvam-api-key",
        }
    }

    /// Developer override, handy for running the eval harness headless.
    fn env_var(self) -> &'static str {
        match self {
            Provider::Gemini => "LUMA_GEMINI_API_KEY",
            Provider::Assemblyai => "LUMA_ASSEMBLYAI_API_KEY",
            Provider::Sarvam => "LUMA_SARVAM_API_KEY",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Gemini => "Gemini",
            Provider::Assemblyai => "AssemblyAI",
            Provider::Sarvam => "Sarvam",
        }
    }
}

pub fn get_key(p: Provider) -> Option<String> {
    if let Ok(v) = std::env::var(p.env_var()) {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }
    keyring::Entry::new(SERVICE, p.account())
        .ok()?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

pub fn set_key(p: Provider, key: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(SERVICE, p.account())?;
    let key = key.trim();
    if key.is_empty() {
        let _ = entry.delete_credential();
    } else {
        entry.set_password(key)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Hold to talk. Uses the global-shortcut plugin's accelerator syntax.
    pub hotkey: String,
    pub gemini_model: String,
    /// minimal | low | medium | high
    pub thinking_level: String,
    pub stt_model: String,
    pub tts_speaker: String,
    pub tts_language: String,
    pub tts_pace: f32,
    /// Long edge of the full-screen image sent to the model, in pixels.
    pub max_image_edge: u32,
    /// Also send a high-detail close-up around the pointer.
    pub send_closeup: bool,
    /// LUMA will not look at the screen while one of these apps is in front.
    pub excluded_apps: Vec<String>,
    /// When paused, the hotkey does nothing and nothing is captured.
    pub paused: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            hotkey: if cfg!(target_os = "macos") { "Command+Shift+Space" } else { "Control+Alt+Space" }.into(),
            gemini_model: "gemini-3.8-flash".into(),
            thinking_level: "low".into(),
            stt_model: "universal-3-5-pro".into(),
            tts_speaker: "shubh".into(),
            tts_language: "en-IN".into(),
            tts_pace: 1.1,
            max_image_edge: 1920,
            send_closeup: true,
            excluded_apps: [
                "1Password",
                "Bitwarden",
                "Keychain Access",
                "LastPass",
                "Dashlane",
                "KeePassXC",
                "Passwords",
            ]
            .map(String::from)
            .to_vec(),
            paused: false,
        }
    }
}

impl Prefs {
    pub fn load(path: &PathBuf) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn is_excluded(&self, app: &str) -> bool {
        let a = app.to_lowercase();
        self.excluded_apps.iter().any(|e| !e.trim().is_empty() && a.contains(&e.trim().to_lowercase()))
    }
}
