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
use luma_core::action::{assess, parse_action, Action, Risk, AGENT_TAGS};
use luma_core::prompt::{agent_context, context_block, system_prompt, AgentContext, Draw, TurnContext, AGENT_PROMPT};
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
    media: &'static str,
    draw: Draw,
    /// Run locate/refer cases through Gemini Live (native audio + draw calls).
    live: Option<String>,
    /// Hedge slow answers with the fast model after 6 s, like the app.
    hedge: bool,
}

fn args() -> Args {
    let mut a = Args {
        filter: None,
        model: "gemini-3.8-flash".into(),
        thinking: "low".into(),
        repeat: 1,
        max_edge: 1920,
        closeup: true,
        media: "MEDIA_RESOLUTION_HIGH",
        draw: Draw::Always,
        live: None,
        hedge: false,
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
            "--media" => {
                a.media = match it.next().as_deref() {
                    Some("low") => "MEDIA_RESOLUTION_LOW",
                    Some("medium") => "MEDIA_RESOLUTION_MEDIUM",
                    _ => "MEDIA_RESOLUTION_HIGH",
                }
            }
            // the shortcut turn: model draws only when it helps
            "--when-useful" => a.draw = Draw::WhenUseful,
            "--hedge" => a.hedge = true,
            "--live" => a.live = Some(it.next().unwrap_or_else(|| "gemini-3.8-live".into())),
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
    let gemini = Gemini { client: reqwest::Client::new(), api_key: key, model: a.model.clone(), thinking_level: a.thinking.clone(), media_resolution: a.media };
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
                let r = match case.kind.as_str() {
                    "locate" | "refer" | "flow" | "explain" | "sketch" | "draws" if a.live.is_some() => {
                        run_live_case(&gemini, a.live.as_deref().unwrap(), &fx, case, &png, &display).await
                    }
                    _ if a.live.is_some() => continue,
                    "route" => run_route(&gemini, &fx, case, &png, &display).await,
                    "agent" => run_agent(&gemini, &fx, case, &png, &display).await,
                    "lesson" => run_lesson(&gemini, &fx, case, &png, &display).await,
                    _ => run_case(&gemini, &fx, case, &png, &display, &a).await,
                };
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

/// Routing: in a voice turn with acting enabled, does the model start a task
/// exactly when it should?
async fn run_route(gemini: &Gemini, fx: &Fixture, case: &Case, png: &image::DynamicImage, display: &Display) -> Result<CaseResult> {
    let prepared = luma_net::vision::prepare(png, display, None, 1280, false)?;
    let ctx = context_block(&TurnContext {
        app: Some(fx.app.as_str()).filter(|s| !s.is_empty()),
        window_title: Some(fx.title.as_str()),
        pointer: None,
        pointer_closeup: None,
        has_closeup: false,
        display_count: 1,
        level: Level::Standard,
        marked_items: "",
        focused: None,
        selection: None,
        lesson: None,
    });
    let g = Gemini { media_resolution: "MEDIA_RESOLUTION_MEDIUM", ..gemini.clone() };
    let body = g.build_body(&system_prompt(Draw::WhenUseful, true), &[], &ctx, &[ImagePart { jpeg: &prepared.images[0].jpeg }], &case.q);
    let t0 = Instant::now();
    let text = g.complete(&body).await?;
    let total = t0.elapsed().as_millis() as u64;
    let mut parser = MarkupParser::new();
    let segs: Vec<Segment> = parser.push(&text).into_iter().chain(parser.finish()).collect();
    let started = segs.iter().any(|s| matches!(s, Segment::Tag(t) if t.name == "task" && t.attr("goal").is_some()));
    let lesson = segs.iter().any(|s| matches!(s, Segment::Tag(t) if t.name == "lesson" && t.attr("goal").is_some()));
    let drew = segs.iter().any(|s| matches!(s, Segment::Tag(t) if t.name != "task" && t.name != "lesson"));
    Ok(CaseResult {
        id: case.id.clone(),
        kind: case.kind.clone(),
        // A lesson's first step is drawn by the lesson loop, not this reply.
        pass: Some(started) == case.task && lesson == case.lesson.unwrap_or(false) && (!drew || lesson),
        ttft_ms: total,
        total_ms: total,
        speech: text.clone(),
        raw: text,
        ..Default::default()
    })
}

/// Fast mode: the same case through Gemini Live. The question goes in as
/// text (the eval has no microphone); answers come back as audio, an output
/// transcript and draw calls, which are scored like tags.
async fn run_live_case(gemini: &Gemini, model: &str, fx: &Fixture, case: &Case, png: &image::DynamicImage, display: &Display) -> Result<CaseResult> {
    use luma_net::live::{draw_call_to_tag, open, LiveConfig, LiveEvent};
    let pointer = match &case.pointer {
        Some(id) => Some(fx.targets.get(id).with_context(|| format!("unknown pointer target {id}"))?.rect().center()),
        None => None,
    };
    let prepared = luma_net::vision::prepare(png, display, pointer.map(|p| Point::new(p.x, p.y)), 1600, false)?;
    let level = luma_core::session::Session::new().observe_level_request(&case.q).unwrap_or(Level::Standard);
    let ctx = context_block(&TurnContext {
        app: Some(fx.app.as_str()).filter(|s| !s.is_empty()),
        window_title: Some(fx.title.as_str()),
        pointer: prepared.pointer_norm,
        pointer_closeup: None,
        has_closeup: false,
        display_count: 1,
        level,
        marked_items: "",
        focused: None,
        selection: None,
        lesson: None,
    });
    let cfg = LiveConfig {
        api_key: gemini.api_key.clone(),
        model: model.to_string(),
        system: format!("{}\n\n{ctx}", luma_core::prompt::live_system_prompt(true)),
        draw_tool: true,
    };
    let t0 = Instant::now();
    let (session, mut events) = open(&cfg).await?;
    let setup_ms = t0.elapsed().as_millis();
    if std::env::var("LIVE_VIDEO").is_ok() {
        session.send_image(&prepared.images[0].jpeg);
        session.send_text(&case.q);
    } else {
        session.send_image_turn(&prepared.images[0].jpeg, Some(&case.q));
    }
    let sent: Vec<_> = prepared.images.iter().map(|i| i.sent).collect();
    let displays = [display.clone()];
    let mut resolver = Resolver::new(&displays, &sent, vec![], 1);
    let (mut speech, mut raw, mut marks, mut dropped, mut first_audio, mut samples) = (String::new(), String::new(), Vec::new(), 0, None, 0usize);
    let mut nudges = 0;
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(25));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            e = events.recv() => {
                if std::env::var("LIVE_DEBUG").is_ok() {
                    match &e { Some(LiveEvent::Audio(a)) => eprintln!("  [{} ms] audio {}", t0.elapsed().as_millis(), a.len()), other => eprintln!("  [{} ms] {other:?}", t0.elapsed().as_millis()) }
                }
                match e {
                None => break,
                // Non-blocking draw calls split the answer into several model
                // turns; it is over once nothing more arrives for a moment.
                Some(LiveEvent::TurnComplete) if samples == 0 && marks.is_empty() && nudges < 2 => {
                    nudges += 1;
                    raw += "[empty turn, nudged] ";
                    session.nudge();
                }
                Some(LiveEvent::TurnComplete) => {
                    if samples > 0 {
                        deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_millis(1800));
                    }
                }
                Some(LiveEvent::Audio(a)) => {
                    first_audio.get_or_insert(t0.elapsed().as_millis() as u64);
                    samples += a.len();
                    deadline.as_mut().reset(tokio::time::Instant::now() + std::time::Duration::from_secs(8));
                }
                Some(LiveEvent::OutputText(t)) => speech.push_str(&t),
                Some(LiveEvent::ToolCall { id, name, args }) => {
                    raw += &format!("[{name} {args}] ");
                    session.tool_response(&id, &name);
                    match draw_call_to_tag(&args).map(|t| resolver.resolve(&t)) {
                        Some(Ok(m)) => {
                            let m = luma_net::vision::place_board(m, &mut resolver, png, display);
                            marks.push(luma_net::vision::snap_sketch_to_ink(&m, png, display))
                        }
                        Some(Err(e)) if e.reason.starts_with("waiting") => {}
                        _ => dropped += 1,
                    }
                    marks.extend(resolver.take_ready());
                }
                Some(LiveEvent::Error(e)) => return Err(anyhow!(e)),
                Some(_) => {}
            }}
        }
    }
    let total = t0.elapsed().as_millis() as u64;
    raw = format!("setup {setup_ms} ms, {:.1}s audio. {raw}{speech}", samples as f64 / 24000.0);
    Ok(score_case(fx, case, &speech, &marks, dropped, first_audio.unwrap_or(total), total, raw))
}

/// Teaching mode, first move: a <next> step pointing at the right control.
async fn run_lesson(gemini: &Gemini, fx: &Fixture, case: &Case, png: &image::DynamicImage, display: &Display) -> Result<CaseResult> {
    use luma_core::lesson::{lesson_context, parse_move, split_reply, Lesson, LessonContext, Move, LESSON_PROMPT};
    let prepared = luma_net::vision::prepare(png, display, None, 1600, false)?;
    let lesson = Lesson::new(case.goal.clone().unwrap_or_else(|| case.q.clone()));
    let ctx = lesson_context(&LessonContext {
        lesson: &lesson,
        app: Some(fx.app.as_str()),
        window_title: Some(fx.title.as_str()),
        changed: None,
        level_instruction: luma_core::prompt::level_instruction(Level::Standard),
    });
    let body = gemini.build_body(LESSON_PROMPT, &[], &ctx, &[ImagePart { jpeg: &prepared.images[0].jpeg }], "What is the next move?");
    let t0 = Instant::now();
    let text = gemini.complete(&body).await?;
    let total = t0.elapsed().as_millis() as u64;
    let mut r = CaseResult { id: case.id.clone(), kind: case.kind.clone(), ttft_ms: total, total_ms: total, raw: text.clone(), ..Default::default() };
    let (speech, tag) = split_reply(&text);
    r.speech = speech.clone();
    let Some(tag) = tag else { return Ok(r) };
    let Ok(Move::Next(step)) = parse_move(&tag, 1, &prepared.images[0].sent, display) else { return Ok(r) };
    // any listed target is a valid first step (toolbar icon or the menu that holds it)
    let expect = case.expect.first().map(String::as_str).unwrap_or("");
    let hit = step.target.is_some_and(|t| case.expect.iter().any(|e| score::target_hit(fx, e, &t.rect)));
    r.marks = step.target.is_some() as usize;
    r.best_iou = step.target.and_then(|t| fx.targets.get(expect).map(|x| t.rect.iou(&x.rect())));
    r.first_hit = Some(hit);
    r.pass = hit && !speech.is_empty();
    Ok(r)
}

/// First agent step: right target (or right URL), and approval policy.
async fn run_agent(gemini: &Gemini, fx: &Fixture, case: &Case, png: &image::DynamicImage, display: &Display) -> Result<CaseResult> {
    let prepared = luma_net::vision::prepare(png, display, None, 1440, false)?;
    let goal = case.goal.clone().unwrap_or_else(|| case.q.clone());
    let ctx = agent_context(&AgentContext {
        goal: &goal,
        app: Some(fx.app.as_str()),
        window_title: Some(fx.title.as_str()),
        step: 1,
        max_steps: 25,
        history: &[],
        last_changed: None,
        platform: "macOS",
    });
    let body = gemini.build_body(AGENT_PROMPT, &[], &ctx, &[ImagePart { jpeg: &prepared.images[0].jpeg }], "Decide the next action.");
    let t0 = Instant::now();
    let text = gemini.complete(&body).await?;
    let total = t0.elapsed().as_millis() as u64;
    let mut parser = MarkupParser::with_tags(AGENT_TAGS);
    let tag = parser.push(&text).into_iter().chain(parser.finish()).find_map(|s| match s {
        Segment::Tag(t) => Some(t),
        _ => None,
    });
    let mut r = CaseResult { id: case.id.clone(), kind: case.kind.clone(), ttft_ms: total, total_ms: total, raw: text.clone(), speech: text, ..Default::default() };
    let Some(tag) = tag else { return Ok(r) };
    let Ok(pa) = parse_action(&tag, &prepared.images[0].sent, display) else { return Ok(r) };
    let expect = case.expect.first().map(String::as_str).unwrap_or("");
    let right_target = match &pa.action {
        Action::Click { at, .. } => {
            r.marks = 1;
            r.best_iou = fx.targets.get(expect).map(|t| at.rect.iou(&t.rect()));
            score::target_hit(fx, expect, &at.rect)
        }
        Action::Open { url } => case.url.as_ref().is_some_and(|u| url.contains(u.as_str())),
        _ => false,
    };
    r.first_hit = Some(right_target);
    let label = match &pa.action {
        Action::Click { label, .. } => label.clone(),
        _ => String::new(),
    };
    let needs_ok = assess(&pa.action, pa.model_risk_high, &label) != Risk::Safe;
    let approval_ok = match case.approval {
        Some(true) => needs_ok,
        Some(false) => !needs_ok,
        None => true,
    };
    r.pass = right_target && approval_ok;
    Ok(r)
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
        pointer_closeup: prepared.pointer_norm_closeup,
        has_closeup: prepared.images.len() > 1,
        display_count: 1,
        level,
        marked_items: "",
        focused: None,
        selection: None,
        lesson: None,
    });
    let images: Vec<ImagePart> = prepared.images.iter().map(|i| ImagePart { jpeg: &i.jpeg }).collect();
    let body = gemini.build_body(&system_prompt(a.draw, false), &[], &ctx, &images, &case.q);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let t0 = Instant::now();
    // --hedge: like the app, start the fast model if this one is silent for 6 s
    let fallback = a.hedge.then(|| {
        let g = Gemini { model: "gemini-3.5-flash".into(), thinking_level: "minimal".into(), ..gemini.clone() };
        let b = g.build_body(&system_prompt(a.draw, false), &[], &ctx, &images, &case.q);
        (g, b)
    });
    let task = gemini.clone().hedged(body, fallback, std::time::Duration::from_millis(6000), tx);
    let mut text = String::new();
    let mut ttft = None;
    while let Some(d) = rx.recv().await {
        ttft.get_or_insert_with(|| t0.elapsed().as_millis() as u64);
        text.push_str(&d);
    }
    let answered_by = task.await??;
    if answered_by != gemini.model {
        text = format!("[answered by {answered_by}] {text}");
    }
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
            Segment::Tag(t) => {
                match resolver.resolve(&t) {
                    Ok(m) => {
                        let m = luma_net::vision::place_board(m, &mut resolver, png, display);
                        marks.push(luma_net::vision::snap_sketch_to_ink(&m, png, display))
                    }
                    Err(e) if e.reason.starts_with("waiting") => {}
                    Err(_) => dropped += 1,
                }
                marks.extend(resolver.take_ready());
            }
        }
    }
    dump_replay(fx, case, &marks);
    Ok(score_case(fx, case, &speech, &marks, dropped, ttft.unwrap_or(total), total, text))
}

/// `LUMA_REPLAY=1`: save the resolved marks so dev/replay.html can draw
/// them over the fixture screenshot with the real overlay.
fn dump_replay(fx: &Fixture, case: &Case, marks: &[Annotation]) {
    if std::env::var("LUMA_REPLAY").is_err() {
        return;
    }
    let dir = repo_root().join("eval/out/replay");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(
        dir.join(format!("{}.json", case.id)),
        serde_json::to_string_pretty(&serde_json::json!({ "fixture": fx.name, "q": case.q, "marks": marks })).unwrap_or_default(),
    );
}
