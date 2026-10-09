//! Where provider requests go: straight to each provider with the user's own
//! keys (default), or through a LUMA key proxy that holds the keys.
//!
//! With a proxy, the app only has a per-device token. Gemini and Sarvam REST
//! requests go through the proxy, which adds the real key; speech-to-text
//! gets a short-lived AssemblyAI token from it and connects directly.
//! Sarvam streaming and Gemini Live are not proxied: LUMA falls back to
//! Sarvam REST and the standard pipeline.

use anyhow::{anyhow, Result};
use std::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Proxy {
    /// e.g. "https://luma-proxy.example.com" (no trailing slash needed)
    pub url: String,
    /// Per-device bearer token issued by whoever runs the proxy.
    pub token: String,
}

static PROXY: RwLock<Option<Proxy>> = RwLock::new(None);

/// Configure (or clear) the proxy for the whole process.
pub fn set_proxy(p: Option<Proxy>) {
    *PROXY.write().unwrap() = p.filter(|p| !p.url.trim().is_empty() && !p.token.trim().is_empty());
}

pub fn proxy() -> Option<Proxy> {
    PROXY.read().unwrap().clone()
}

fn base(p: &Proxy) -> &str {
    p.url.trim().trim_end_matches('/')
}

/// A REST request to a provider: the URL to call and the auth header.
pub struct Target {
    pub url: String,
    pub header: (&'static str, String),
}

pub fn gemini(path_and_query: &str, api_key: &str) -> Target {
    match proxy() {
        Some(p) => Target { url: format!("{}/gemini/{path_and_query}", base(&p)), header: ("authorization", format!("Bearer {}", p.token)) },
        None => Target {
            url: format!("https://generativelanguage.googleapis.com/{path_and_query}"),
            header: ("x-goog-api-key", api_key.to_string()),
        },
    }
}

pub fn sarvam(path: &str, api_key: &str) -> Target {
    match proxy() {
        Some(p) => Target { url: format!("{}/sarvam/{path}", base(&p)), header: ("authorization", format!("Bearer {}", p.token)) },
        None => Target { url: format!("https://api.sarvam.ai/{path}"), header: ("api-subscription-key", api_key.to_string()) },
    }
}

/// Through a proxy: a short-lived AssemblyAI streaming token.
pub async fn assemblyai_token() -> Result<Option<String>> {
    let Some(p) = proxy() else { return Ok(None) };
    let r = reqwest::Client::new()
        .get(format!("{}/assemblyai/token", base(&p)))
        .bearer_auth(&p.token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| anyhow!("LUMA proxy unreachable: {e}"))?;
    let status = r.status();
    if !status.is_success() {
        return Err(anyhow!("LUMA proxy refused the speech token ({status})"));
    }
    let v: serde_json::Value = r.json().await?;
    v.get("token").and_then(|t| t.as_str()).map(|t| Some(t.to_string())).ok_or_else(|| anyhow!("LUMA proxy sent no token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_direct_or_through_the_proxy() {
        set_proxy(None);
        let t = gemini("v1beta/models/m:generateContent", "KEY");
        assert_eq!(t.url, "https://generativelanguage.googleapis.com/v1beta/models/m:generateContent");
        assert_eq!(t.header, ("x-goog-api-key", "KEY".to_string()));
        set_proxy(Some(Proxy { url: "https://p.example/".into(), token: "dev1".into() }));
        let t = sarvam("text-to-speech", "KEY");
        assert_eq!(t.url, "https://p.example/sarvam/text-to-speech");
        assert_eq!(t.header, ("authorization", "Bearer dev1".to_string()));
        set_proxy(Some(Proxy { url: "https://p.example".into(), token: " ".into() }));
        assert!(proxy().is_none(), "a blank token means no proxy");
        set_proxy(None);
    }
}
