//! Provider smoke test: checks every key and the full voice loop without a
//! microphone. Gemini writes a sentence → Sarvam speaks it → the audio is
//! streamed (in real time) into AssemblyAI → the transcript must match.
//!
//!   npm run smoke

use anyhow::{anyhow, bail, Context, Result};
use luma_core::audio::{i16_to_le_bytes, MonoResampler};
use luma_net::gemini::Gemini;
use luma_net::sarvam::SarvamTts;
use std::path::Path;
use std::time::{Duration, Instant};

fn key(names: &[&str]) -> Option<String> {
    names.iter().filter_map(|v| std::env::var(v).ok()).map(|v| v.trim().to_string()).find(|v| !v.is_empty())
}

/// Minimal PCM16 WAV reader → (sample_rate, channels, samples as f32).
fn read_wav(bytes: &[u8]) -> Result<(u32, u16, Vec<f32>)> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into()?) as usize;
        let body = &bytes[i + 8..(i + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            channels = u16::from_le_bytes(body[2..4].try_into()?);
            rate = u32::from_le_bytes(body[4..8].try_into()?);
            bits = u16::from_le_bytes(body[14..16].try_into()?);
        } else if id == b"data" {
            if bits != 16 {
                bail!("expected 16-bit PCM, got {bits}-bit");
            }
            let s = body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
            return Ok((rate, channels, s));
        }
        i += 8 + len + (len & 1);
    }
    Err(anyhow!("no data chunk"))
}

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env"));
    let http = reqwest::Client::new();
    let mut ok = true;

    // 1. Gemini
    let gemini_key = key(&["LUMA_GEMINI_API_KEY", "GEMINI_API_KEY"]).context("LUMA_GEMINI_API_KEY missing")?;
    let model = std::env::args().nth(1).unwrap_or_else(|| "gemini-3.8-flash".into());
    let g = Gemini { client: http.clone(), api_key: gemini_key, model: model.clone(), thinking_level: std::env::var("LUMA_THINKING").unwrap_or_else(|_| "low".into()), media_resolution: "MEDIA_RESOLUTION_HIGH" };
    let body = g.build_body(
        "Reply with exactly one short friendly sentence and nothing else.",
        &[],
        "<context>smoke test</context>",
        &[],
        "Introduce yourself as LUMA, a screen companion, in under twelve words.",
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let t0 = Instant::now();
    let task = tokio::spawn({
        let g = g.clone();
        async move { g.stream(&body, tx).await }
    });
    let mut sentence = String::new();
    let mut ttft = None;
    while let Some(d) = rx.recv().await {
        ttft.get_or_insert(t0.elapsed());
        sentence.push_str(&d);
    }
    let sentence = sentence.trim().to_string();
    match task.await? {
        Ok(()) if !sentence.is_empty() => println!(
            "✓ Gemini ({model}): first token {} ms, total {} ms — \"{sentence}\"",
            ttft.unwrap_or_default().as_millis(),
            t0.elapsed().as_millis()
        ),
        Ok(()) => {
            ok = false;
            println!("✗ Gemini returned no text");
        }
        Err(e) => {
            println!("✗ Gemini: {e:#}");
            bail!("Gemini failed; fix the key/model before continuing");
        }
    }

    // 2. Sarvam
    let Some(sarvam_key) = key(&["LUMA_SARVAM_API_KEY", "SARVAM_API_KEY"]) else {
        println!("- Sarvam: no key (captions-only mode); skipping voice loop");
        return Ok(());
    };
    let tts = SarvamTts { client: http.clone(), api_key: sarvam_key, speaker: "shubh".into(), language: "en-IN".into(), pace: 1.1 };
    let t1 = Instant::now();
    let wav = match tts.synthesize(&sentence).await {
        Ok(w) => w,
        Err(e) => {
            println!("✗ Sarvam: {e:#}");
            bail!("Sarvam failed");
        }
    };
    let (rate, channels, samples) = read_wav(&wav)?;
    println!(
        "✓ Sarvam (bulbul:v3): {} ms, {:.1}s of audio at {rate} Hz",
        t1.elapsed().as_millis(),
        samples.len() as f64 / channels as f64 / rate as f64
    );

    // 3. AssemblyAI, fed the synthesized speech in real time (100 ms chunks)
    let Some(aai_key) = key(&["LUMA_ASSEMBLYAI_API_KEY", "ASSEMBLYAI_API_KEY"]) else {
        bail!("LUMA_ASSEMBLYAI_API_KEY missing");
    };
    let mut rs = MonoResampler::new(rate, channels, luma_net::STT_RATE);
    let mut pcm = rs.process(&samples);
    pcm.extend(std::iter::repeat(0).take(luma_net::STT_RATE as usize / 2)); // trailing silence
    let (atx, arx) = tokio::sync::mpsc::channel(64);
    let (etx, _erx) = tokio::sync::mpsc::unbounded_channel();
    let feeder = tokio::spawn(async move {
        for chunk in pcm.chunks(1600) {
            if atx.send(i16_to_le_bytes(chunk)).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    let stt_model = std::env::args().nth(2).unwrap_or_else(|| "universal-3-5-pro".into());
    let t2 = Instant::now();
    let transcript = luma_net::assemblyai::transcribe(&aai_key, &stt_model, arx, etx).await;
    feeder.abort();
    match transcript {
        Ok(t) => {
            let said = words(&sentence);
            let heard = words(&t);
            let matched = said.iter().filter(|w| heard.contains(w)).count();
            let ratio = matched as f64 / said.len().max(1) as f64;
            let mark = if ratio >= 0.7 { "✓" } else { "✗" };
            ok &= ratio >= 0.7;
            println!(
                "{mark} AssemblyAI ({stt_model}): \"{t}\" — {:.0}% word match, {} ms after audio start",
                ratio * 100.0,
                t2.elapsed().as_millis()
            );
        }
        Err(e) => {
            ok = false;
            println!("✗ AssemblyAI: {e:#}");
        }
    }
    if ok {
        println!("\nAll providers OK — voice loop verified end to end.");
        Ok(())
    } else {
        Err(anyhow!("smoke test failed"))
    }
}
