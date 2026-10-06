//! LUMA eval: runs the real prompt + parsing pipeline against rendered
//! fixtures with known ground truth and scores grounding quality.
//!
//!   node eval/render.mjs
//!   cargo run -p luma-eval --release -- [--filter editor] [--model id] [--thinking low] [--repeat 3]
//!
//! Metrics (per case kind):
//! * locate / refer – first mark's centre inside the target (ScreenSpot-style
//!   "click accuracy"), best IoU, whether any mark hit it.
//! * flow – component coverage and pairwise order of the walkthrough.
//! * ambiguous – asks a question instead of committing to one target.
//! * explain – required concepts are mentioned.
//! Plus time-to-first-token and total latency.

mod score;

use anyhow::{anyhow, Context, Result};
use luma_core::annotation::{Annotation, Resolver};
use luma_core::geometry::{Display, Point, Rect};
use luma_core::markup::{MarkupParser, Segment};
use luma_core::prompt::{context_block, TurnContext, SYSTEM_PROMPT};
use luma_core::session::Level;
use luma_net::gemini::{Gemini, ImagePart};
use score::{score_case, Case, CaseResult, Fixture};
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Args {
    filter: Option<String>,
    model: String,
    thinking: String,
    repeat: usize,
    max_edge: u32,
    closeup: bool,
}

fn args() -> Args {
    let mut a = Args {
        filter: None,
        model: "gemini-3.8-flash".into(),
        thinking: "low".into(),
        repeat: 1,
        max_edge: 1920,
        closeup: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        match k.as_str() {
            "--filter" => a.filter = it.next(),
            "--model" => a.model = it.next().unwrap_or(a.model),
            "--thinking" => a.thinking = it.next().unwrap_or(a.thinking),
            "--repeat" => a.repeat = it.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--max-edge" => a.max_edge = it.next().and_then(|s| s.parse().ok()).unwrap_or(1920),
            "--no-closeup" => a.closeup = false,
            _ => eprintln!("unknown argument {k}"),
        }
    }
    a
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[tokio::main]
async fn main() -> Result<()> {
    let a = args();
    let _ = dotenvy::from_path(repo_root().join(".env"));
    let key = ["LUMA_GEMINI_API_KEY", "GEMINI_API_KEY"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|k| !k.trim().is_empty())
        .ok_or_else(|| anyhow!("set LUMA_GEMINI_API_KEY in .env (or the environment) to run the eval"))?;
    let gemini = Gemini { client: reqwest::Client::new(), api_key: key, model: a.model.clone(), thinking_level: a.thinking.clone() };
    let out = repo_root().join("eval/out");
    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(&out)
        .with_context(|| "run `node eval/render.mjs` first")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    fixtures.sort();

    let mut results: Vec<CaseResult> = Vec::new();
    for path in fixtures {
        let fx: Fixture = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let png = image::open(path.with_extension("png"))?;
        let display = Display {
            index: 0,
            name: "eval".into(),
            // The screenshot defines the screen; headless Chrome's reported
            // innerHeight can differ from the captured area.
            input_frame: Rect::new(0.0, 0.0, png.width() as f64 / fx.dpr, png.height() as f64 / fx.dpr),
            input_per_point: 1.0,
            scale_factor: fx.dpr,
            is_primary: true,
        };
        for case in &fx.cases {
            if let Some(f) = &a.filter {
                if !case.id.contains(f.as_str()) && !fx.name.contains(f.as_str()) {
                    continue;
                }
            }
            for run in 0..a.repeat {
                let r = run_case(&gemini, &fx, case, &png, &display, &a).await;
                match r {
                    Ok(r) => {
                        println!("{}", r.line());
                        results.push(r);
                    }
                    Err(e) => {
                        println!("{:<28} ERROR {e:#}", case.id);
                        results.push(CaseResult::error(&case.id, &case.kind, run, &format!("{e:#}")));
                    }
                }
            }
        }
    }
    let summary = score::summarize(&results);
    println!("\n{summary}");
    let dir = repo_root().join("eval/results");
    std::fs::create_dir_all(&dir)?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let file = dir.join(format!("{stamp}-{}.json", a.model));
    std::fs::write(
        &file,
        serde_json::to_string_pretty(&serde_json::json!({
            "model": a.model, "thinking": a.thinking, "max_edge": a.max_edge, "closeup": a.closeup,
            "results": results,
        }))?,
    )?;
    println!("results: {}", file.display());
    Ok(())
}

async fn run_case(
    gemini: &Gemini,
    fx: &Fixture,
    case: &Case,
    png: &image::DynamicImage,
    display: &Display,
    a: &Args,
) -> Result<CaseResult> {
    let pointer = match &case.pointer {
        Some(id) => Some(fx.targets.get(id).with_context(|| format!("unknown pointer target {id}"))?.rect().center()),
        None => None,
    };
    let prepared = luma_net::vision::prepare(png, display, pointer.map(|p| Point::new(p.x, p.y)), a.max_edge, a.closeup)?;
    let level = luma_core::session::Session::new().observe_level_request(&case.q).unwrap_or(Level::Standard);
    let ctx = context_block(&TurnContext {
        app: Some(fx.app.as_str()).filter(|s| !s.is_empty()),
        window_title: Some(fx.title.as_str()),
        pointer: prepared.pointer_norm,
        has_closeup: prepared.images.len() > 1,
        display_count: 1,
        level,
        marked_items: "",
    });
    let images: Vec<ImagePart> = prepared.images.iter().map(|i| ImagePart { jpeg: &i.jpeg }).collect();
    let body = gemini.build_body(SYSTEM_PROMPT, &[], &ctx, &images, &case.q);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let t0 = Instant::now();
    let g = gemini.clone();
    let task = tokio::spawn(async move { g.stream(&body, tx).await });
    let mut text = String::new();
    let mut ttft = None;
    while let Some(d) = rx.recv().await {
        ttft.get_or_insert_with(|| t0.elapsed().as_millis() as u64);
        text.push_str(&d);
    }
    task.await??;
    let total = t0.elapsed().as_millis() as u64;

    let sent: Vec<_> = prepared.images.iter().map(|i| i.sent).collect();
    let displays = [display.clone()];
    let mut resolver = Resolver::new(&displays, &sent, vec![], 1);
    let mut parser = MarkupParser::new();
    let mut speech = String::new();
    let mut marks: Vec<Annotation> = Vec::new();
    let mut dropped = 0;
    let segs: Vec<Segment> = parser.push(&text).into_iter().chain(parser.finish()).collect();
    for s in segs {
        match s {
            Segment::Text(t) => speech.push_str(&t),
            Segment::Tag(t) => match resolver.resolve(&t) {
                Ok(m) => marks.push(m),
                Err(_) => dropped += 1,
            },
        }
    }
    Ok(score_case(fx, case, &speech, &marks, dropped, ttft.unwrap_or(total), total, text))
}
