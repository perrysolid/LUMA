//! LUMA key proxy: provider keys stay on the server; each device gets a
//! bearer token with access to exactly the requests LUMA makes, rate-limited.
//!
//! * `POST /gemini/v1beta/models/{model}:streamGenerateContent?alt=sse` and
//!   `:generateContent`: forwarded with the Gemini key, streamed back.
//! * `POST /sarvam/text-to-speech`: forwarded with the Sarvam key.
//! * `GET  /assemblyai/token`: a 60-second AssemblyAI streaming token; the
//!   device then connects to AssemblyAI directly with it.
//! * `GET  /health`
//!
//! Everything else is 404. Configure with environment variables (see
//! `Config::from_env`).

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub gemini_key: Option<String>,
    pub sarvam_key: Option<String>,
    pub assemblyai_key: Option<String>,
    /// Device tokens allowed to use the proxy.
    pub tokens: Vec<String>,
    /// Requests per minute per device token.
    pub per_minute: u32,
    pub gemini_base: String,
    pub sarvam_base: String,
    pub assemblyai_base: String,
}

impl Config {
    /// `GEMINI_API_KEY`, `SARVAM_API_KEY`, `ASSEMBLYAI_API_KEY`,
    /// `LUMA_PROXY_TOKENS` (comma-separated), `LUMA_PROXY_RPM` (default 60).
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        Self {
            gemini_key: var("GEMINI_API_KEY"),
            sarvam_key: var("SARVAM_API_KEY"),
            assemblyai_key: var("ASSEMBLYAI_API_KEY"),
            tokens: var("LUMA_PROXY_TOKENS")
                .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| s.len() >= 16).collect())
                .unwrap_or_default(),
            per_minute: var("LUMA_PROXY_RPM").and_then(|v| v.parse().ok()).unwrap_or(60),
            gemini_base: var("LUMA_PROXY_GEMINI_BASE").unwrap_or_else(|| "https://generativelanguage.googleapis.com".into()),
            sarvam_base: var("LUMA_PROXY_SARVAM_BASE").unwrap_or_else(|| "https://api.sarvam.ai".into()),
            assemblyai_base: var("LUMA_PROXY_ASSEMBLYAI_BASE").unwrap_or_else(|| "https://streaming.assemblyai.com".into()),
        }
    }
}

struct Inner {
    cfg: Config,
    http: reqwest::Client,
    /// token → (window start, requests in window)
    usage: Mutex<HashMap<String, (Instant, u32)>>,
}

type Shared = Arc<Inner>;

pub fn router(cfg: Config) -> Router {
    let state: Shared = Arc::new(Inner { cfg, http: reqwest::Client::new(), usage: Mutex::new(HashMap::new()) });
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/gemini/v1beta/models/{model_action}", post(gemini))
        .route("/sarvam/text-to-speech", post(sarvam))
        .route("/assemblyai/token", get(assemblyai_token))
        // screenshots are a few MB as base64
        .layer(DefaultBodyLimit::max(24 * 1024 * 1024))
        .with_state(state)
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, axum::Json(serde_json::json!({ "error": msg }))).into_response()
}

/// Constant-time comparison so tokens cannot be guessed byte by byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Authenticate and count one request. Returns the device token.
fn admit(s: &Inner, headers: &HeaderMap) -> Result<String, Response> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or("");
    let Some(t) = s.cfg.tokens.iter().find(|t| same(t, token)) else {
        return Err(err(StatusCode::UNAUTHORIZED, "unknown device token"));
    };
    let mut usage = s.usage.lock().unwrap();
    let e = usage.entry(t.clone()).or_insert((Instant::now(), 0));
    if e.0.elapsed() > Duration::from_secs(60) {
        *e = (Instant::now(), 0);
    }
    e.1 += 1;
    if e.1 > s.cfg.per_minute {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "rate limit: try again in a minute"));
    }
    Ok(t.clone())
}

/// `gemini-3.8-flash:streamGenerateContent` and the like, nothing else.
fn allowed_model_action(s: &str) -> bool {
    let Some((model, action)) = s.split_once(':') else { return false };
    matches!(action, "streamGenerateContent" | "generateContent")
        && !model.is_empty()
        && model.len() < 80
        && model.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Stream an upstream response back to the device.
fn relay(r: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let ctype = r.headers().get("content-type").cloned();
    let mut resp = Response::new(Body::from_stream(r.bytes_stream()));
    *resp.status_mut() = status;
    if let Some(c) = ctype {
        resp.headers_mut().insert("content-type", c);
    }
    resp
}

async fn gemini(State(s): State<Shared>, Path(model_action): Path<String>, RawQuery(q): RawQuery, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Err(r) = admit(&s, &headers) {
        return r;
    }
    if !allowed_model_action(&model_action) {
        return err(StatusCode::NOT_FOUND, "not an allowed Gemini call");
    }
    let Some(key) = &s.cfg.gemini_key else { return err(StatusCode::SERVICE_UNAVAILABLE, "no Gemini key on the proxy") };
    let query = match q.as_deref() {
        Some("alt=sse") => "?alt=sse",
        None => "",
        Some(_) => return err(StatusCode::BAD_REQUEST, "unexpected query"),
    };
    let url = format!("{}/v1beta/models/{model_action}{query}", s.cfg.gemini_base);
    match s.http.post(url).header("x-goog-api-key", key).header("content-type", "application/json").body(body).send().await {
        Ok(r) => relay(r),
        Err(e) => err(StatusCode::BAD_GATEWAY, &format!("Gemini unreachable: {e}")),
    }
}

async fn sarvam(State(s): State<Shared>, headers: HeaderMap, body: axum::body::Bytes) -> Response {
    if let Err(r) = admit(&s, &headers) {
        return r;
    }
    let Some(key) = &s.cfg.sarvam_key else { return err(StatusCode::SERVICE_UNAVAILABLE, "no Sarvam key on the proxy") };
    let url = format!("{}/text-to-speech", s.cfg.sarvam_base);
    match s.http.post(url).header("api-subscription-key", key).header("content-type", "application/json").body(body).send().await {
        Ok(r) => relay(r),
        Err(e) => err(StatusCode::BAD_GATEWAY, &format!("Sarvam unreachable: {e}")),
    }
}

async fn assemblyai_token(State(s): State<Shared>, headers: HeaderMap) -> Response {
    if let Err(r) = admit(&s, &headers) {
        return r;
    }
    let Some(key) = &s.cfg.assemblyai_key else { return err(StatusCode::SERVICE_UNAVAILABLE, "no AssemblyAI key on the proxy") };
    let url = format!("{}/v3/token?expires_in_seconds=60", s.cfg.assemblyai_base);
    match s.http.get(url).header("authorization", key).send().await {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => match v.get("token").and_then(|t| t.as_str()) {
                Some(t) => axum::Json(serde_json::json!({ "token": t, "expires_in_seconds": 60 })).into_response(),
                None => err(StatusCode::BAD_GATEWAY, "AssemblyAI returned no token"),
            },
            Err(_) => err(StatusCode::BAD_GATEWAY, "AssemblyAI returned no token"),
        },
        Ok(r) => err(StatusCode::BAD_GATEWAY, &format!("AssemblyAI refused: {}", r.status())),
        Err(e) => err(StatusCode::BAD_GATEWAY, &format!("AssemblyAI unreachable: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::any;

    const DEV: &str = "device-token-0123456789";

    /// A fake provider that echoes which key and path it was called with.
    async fn fake_upstream() -> String {
        let app = Router::new().route(
            "/{*rest}",
            any(|headers: HeaderMap, uri: axum::http::Uri, body: axum::body::Bytes| async move {
                let key = ["x-goog-api-key", "api-subscription-key", "authorization"]
                    .iter()
                    .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()).map(|v| format!("{h}={v}")))
                    .unwrap_or_default();
                if uri.path() == "/v3/token" {
                    return axum::Json(serde_json::json!({ "token": "tmp-123", "key": key })).into_response();
                }
                format!("{} {} {}", uri, key, body.len()).into_response()
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    async fn proxy(per_minute: u32) -> String {
        let up = fake_upstream().await;
        let cfg = Config {
            gemini_key: Some("G-KEY".into()),
            sarvam_key: Some("S-KEY".into()),
            assemblyai_key: Some("A-KEY".into()),
            tokens: vec![DEV.into()],
            per_minute,
            gemini_base: up.clone(),
            sarvam_base: up.clone(),
            assemblyai_base: up,
        };
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, router(cfg)).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn forwards_only_luma_calls_with_server_keys() {
        let p = proxy(100).await;
        let c = reqwest::Client::new();
        // no / wrong token
        assert_eq!(c.post(format!("{p}/sarvam/text-to-speech")).send().await.unwrap().status(), 401);
        assert_eq!(c.post(format!("{p}/sarvam/text-to-speech")).bearer_auth("nope").send().await.unwrap().status(), 401);
        // gemini: key injected, query kept, body passed through
        let r = c
            .post(format!("{p}/gemini/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse"))
            .bearer_auth(DEV)
            .body("{\"x\":1}")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        assert_eq!(r.text().await.unwrap(), "/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse x-goog-api-key=G-KEY 7");
        // other Gemini methods and odd queries are refused
        for path in ["v1beta/models/gemini-3.8-flash:countTokens", "v1beta/models/..%2Fx:generateContent"] {
            assert_eq!(c.post(format!("{p}/gemini/{path}")).bearer_auth(DEV).send().await.unwrap().status(), 404, "{path}");
        }
        assert_eq!(
            c.post(format!("{p}/gemini/v1beta/models/m:generateContent?key=steal")).bearer_auth(DEV).send().await.unwrap().status(),
            400
        );
        assert_eq!(c.get(format!("{p}/gemini/v1beta/models")).bearer_auth(DEV).send().await.unwrap().status(), 404);
        // sarvam
        let r = c.post(format!("{p}/sarvam/text-to-speech")).bearer_auth(DEV).body("{}").send().await.unwrap();
        assert_eq!(r.text().await.unwrap(), "/text-to-speech api-subscription-key=S-KEY 2");
        // assemblyai token: the device never sees the account key
        let v: serde_json::Value = c.get(format!("{p}/assemblyai/token")).bearer_auth(DEV).send().await.unwrap().json().await.unwrap();
        assert_eq!(v["token"], "tmp-123");
        assert!(v.get("key").is_none());
    }

    #[tokio::test]
    async fn rate_limits_per_device() {
        let p = proxy(2).await;
        let c = reqwest::Client::new();
        let mut codes = Vec::new();
        for _ in 0..3 {
            codes.push(c.get(format!("{p}/assemblyai/token")).bearer_auth(DEV).send().await.unwrap().status().as_u16());
        }
        assert_eq!(codes, vec![200, 200, 429]);
    }

    #[test]
    fn short_tokens_are_ignored_in_config() {
        std::env::set_var("LUMA_PROXY_TOKENS", "short, device-token-0123456789 ,");
        assert_eq!(Config::from_env().tokens, vec!["device-token-0123456789".to_string()]);
        std::env::remove_var("LUMA_PROXY_TOKENS");
    }
}
