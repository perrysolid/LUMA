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

    /// Environment variables (or `.env` entries) checked before the keychain.
    fn env_vars(self) -> [&'static str; 2] {
        match self {
            Provider::Gemini => ["LUMA_GEMINI_API_KEY", "GEMINI_API_KEY"],
            Provider::Assemblyai => ["LUMA_ASSEMBLYAI_API_KEY", "ASSEMBLYAI_API_KEY"],
            Provider::Sarvam => ["LUMA_SARVAM_API_KEY", "SARVAM_API_KEY"],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Env,
    Keychain,
}

/// Load `.env` files without overriding variables already set in the real
/// environment. First file wins for any given key:
/// 1. `.env` in the working directory
/// 2. the repository's `.env` (debug builds only, so `npm run tauri dev` works)
/// 3. `.env` in LUMA's config directory (for installed builds)
pub fn load_env_files(config_dir: Option<&std::path::Path>) {
    let _ = dotenvy::dotenv();
    #[cfg(debug_assertions)]
    let _ = dotenvy::from_path(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.env"));
    if let Some(dir) = config_dir {
        let _ = dotenvy::from_path(dir.join(".env"));
    }
}

fn env_key(p: Provider) -> Option<String> {
    p.env_vars()
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

pub fn key_source(p: Provider) -> Option<KeySource> {
    if env_key(p).is_some() {
        Some(KeySource::Env)
    } else if keychain_key(p).is_some() {
        Some(KeySource::Keychain)
    } else {
        None
    }
}

/// `.env` / environment first, then the OS keychain.
pub fn get_key(p: Provider) -> Option<String> {
    env_key(p).or_else(|| keychain_key(p))
}

fn keychain_key(p: Provider) -> Option<String> {
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
    /// When to draw on screen: "long_press" (hold the shortcut ≥ long_press_ms),
    /// "always", or "never". Voice-only turns use a smaller image (fewer tokens).
    pub annotate: String,
    pub long_press_ms: u64,
    /// Let LUMA click and type to carry out tasks you ask for.
    pub can_act: bool,
    /// Upper bound on agent steps per task.
    pub max_task_steps: usize,
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
            annotate: "long_press".into(),
            long_press_ms: 1800,
            can_act: true,
            max_task_steps: 25,
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
