//! `cargo run -p luma-proxy --release` with GEMINI_API_KEY, SARVAM_API_KEY,
//! ASSEMBLYAI_API_KEY and LUMA_PROXY_TOKENS set. PORT defaults to 8787.
//! Put it behind HTTPS (any reverse proxy or platform TLS) before use.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cfg = luma_proxy::Config::from_env();
    if cfg.tokens.is_empty() {
        anyhow::bail!("set LUMA_PROXY_TOKENS to one or more device tokens (16+ characters each)");
    }
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    log::info!(
        "LUMA proxy on :{port} for {} device(s); gemini {} sarvam {} assemblyai {}",
        cfg.tokens.len(),
        cfg.gemini_key.is_some(),
        cfg.sarvam_key.is_some(),
        cfg.assemblyai_key.is_some()
    );
    axum::serve(listener, luma_proxy::router(cfg)).await?;
    Ok(())
}
