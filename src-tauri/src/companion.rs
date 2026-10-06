//! The turn loop: hotkey → listen → see → think → speak (+ draw) (+ act).
//!
//! Modes (decided when the shortcut is released):
//! * **Voice** — quick press: small screenshot, medium media resolution, no
//!   drawing. Cheapest per turn.
//! * **Annotate** — held ≥ `long_press_ms` (or "always"): full-resolution
//!   screenshot plus pointer close-up, boxes/arrows/labels on screen.
//! * **Task** — in either mode the model may answer with `<task goal=…/>`,
//!   which hands off to the agent loop (`agent.rs`).
//!
//! Latency: capture and the STT connection start the moment the key goes
//! down; Gemini streams; speech is synthesized per utterance while the model
//! is still writing; annotations are synchronized through the audio queue.
//! Barge-in: every turn carries an epoch; pressing the key bumps it.

use crate::agent::{self, Pending, TaskState};
use crate::audio::{start_mic, MicHandle, Speaker, SpeakerCmd};
use crate::screen::{self, CaptureOptions, RawSnapshot, Snapshot};
use crate::settings::{get_key, Prefs, Provider};
use crate::AppState;
use anyhow::{anyhow, Result};
use luma_core::action::{yes_no, YesNo};
use luma_core::annotation::{Annotation, Resolver};
use luma_core::markup::{MarkupParser, Segment};
use luma_core::prompt::{context_block, system_prompt, TurnContext};
use luma_core::sequencer::{Released, Sequencer};
use luma_core::session::Turn;
use luma_core::speech::SpeechChunker;
use luma_net::assemblyai::{self, SttEvent};
use luma_net::gemini::{Gemini, HistoryTurn, ImagePart};
use luma_net::sarvam::SarvamTts;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Semaphore};

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Listening,
    Thinking,
    Speaking,
    /// Carrying out a task on the computer.
    Acting,
    /// A task is paused, waiting for the user's yes/no or an answer.
    Waiting,
    Paused,
    Error,
}

#[derive(Clone, Serialize)]
pub struct StatusEvent {
    pub phase: Phase,
    pub message: Option<String>,
    /// Display whose overlay should show the HUD.
    pub display: Option<usize>,
}

#[derive(Debug, Clone)]
enum Marker {
    Annotate(Annotation),
    Caption(String),
}

struct Listening {
    epoch: u64,
    mic: Option<MicHandle>,
    stt: JoinHandle<Result<String>>,
    snapshot: JoinHandle<Result<RawSnapshot>>,
    started: Instant,
}

/// Annotations currently on screen, and what the screen looked like then.
/// The watcher clears them when the user switches window or the content
/// changes (scroll, next slide, navigation).
pub struct Watch {
    pub display: usize,
    pub window_key: String,
    pub baseline: Option<Vec<u8>>,
    pub last_mark: Instant,
}

pub struct Companion {
    epoch: AtomicU64,
    speaker: Option<Speaker>,
    http: reqwest::Client,
    listening: Mutex<Option<Listening>>,
    hud_display: Mutex<Option<usize>>,
    pending: Mutex<Option<Pending>>,
    task_running: AtomicBool,
    speaking: AtomicBool,
    pub watch: Mutex<Option<Watch>>,
}

/// The foreground app was on the exclusion list.
#[derive(Debug)]
pub struct Excluded(pub String);
impl std::fmt::Display for Excluded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LUMA doesn't look at the screen while {} is in front.", self.0)
    }
}
impl std::error::Error for Excluded {}

impl Companion {
    pub fn new() -> Self {
        let speaker = match Speaker::spawn() {
            Ok(s) => Some(s),
            Err(e) => {
                log::error!("audio output unavailable: {e}");
                None
            }
        };
        Self {
            epoch: AtomicU64::new(0),
            speaker,
            http: reqwest::Client::builder()
                .pool_idle_timeout(std::time::Duration::from_secs(90))
                .build()
                .expect("http client"),
            listening: Mutex::new(None),
            hud_display: Mutex::new(None),
            pending: Mutex::new(None),
            task_running: AtomicBool::new(false),
            speaking: AtomicBool::new(false),
            watch: Mutex::new(None),
        }
    }

    pub fn current(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::SeqCst) == epoch
    }

    pub fn is_task_running(&self) -> bool {
        self.task_running.load(Ordering::SeqCst)
    }

    pub fn set_task_running(&self, v: bool) {
        self.task_running.store(v, Ordering::SeqCst);
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }

    pub fn set_pending(&self, p: Pending) {
        *self.pending.lock().unwrap() = Some(p);
    }

    /// Stop talking, clear the screen, cancel everything in flight. A paused
    /// task (`pending`) survives so the user's reply can resume it.
    pub fn interrupt(&self, app: &AppHandle) -> u64 {
        let e = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(s) = &self.speaker {
            s.stop();
        }
        self.speaking.store(false, Ordering::SeqCst);
        if let Some(l) = self.listening.lock().unwrap().take() {
            l.stt.abort();
            l.snapshot.abort();
        }
        *self.watch.lock().unwrap() = None;
        let _ = app.emit("luma://clear", ());
        e
    }

    /// Cancel any paused task too (Stop button, Forget, Pause).
    pub fn cancel_all(&self, app: &AppHandle) -> u64 {
        *self.pending.lock().unwrap() = None;
        self.interrupt(app)
    }

    pub fn status(&self, app: &AppHandle, phase: Phase, message: Option<String>) {
        let display = *self.hud_display.lock().unwrap();
        let _ = app.emit("luma://status", StatusEvent { phase, message, display });
    }

    // ------------------------------------------------------------ hotkey

    pub fn on_press(self: &Arc<Self>, app: &AppHandle) {
        let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
        if prefs.paused {
            self.status(app, Phase::Paused, Some("LUMA is paused. Resume it from the menu bar.".into()));
            return;
        }
        let was_running_task = self.is_task_running();
        let epoch = self.interrupt(app);
        if was_running_task {
            *self.pending.lock().unwrap() = None;
            let _ = app.emit("luma://notice", "Stopped the task.");
        }
        let missing: Vec<&str> = [Provider::Gemini, Provider::Assemblyai]
            .into_iter()
            .filter(|p| get_key(*p).is_none())
            .map(|p| p.name())
            .collect();
        if !missing.is_empty() {
            self.status(app, Phase::Error, Some(format!("Add your {} API key in LUMA settings.", missing.join(" and "))));
            crate::show_panel(app);
            return;
        }
        if !screen::has_screen_permission() {
            let e = screen::request_screen_permission();
            self.status(app, Phase::Error, Some(e.to_string()));
            crate::show_panel(app);
            return;
        }
        let snapshot = self.spawn_capture(app, &prefs);
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(256);
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<SttEvent>();
        let app2 = app.clone();
        let mic = match start_mic(audio_tx, {
            let app = app.clone();
            let last = Mutex::new(Instant::now());
            move |lvl| {
                let mut l = last.lock().unwrap();
                if l.elapsed().as_millis() >= 60 {
                    *l = Instant::now();
                    let _ = app.emit("luma://level", lvl);
                }
            }
        }) {
            Ok(m) => m,
            Err(e) => {
                snapshot.abort();
                self.status(app, Phase::Error, Some(e.to_string()));
                return;
            }
        };
        let key = get_key(Provider::Assemblyai).unwrap_or_default();
        let model = prefs.stt_model.clone();
        let stt = tauri::async_runtime::spawn(async move {
            let fwd = tokio::spawn(async move {
                while let Some(SttEvent::Partial(t)) = ev_rx.recv().await {
                    let _ = app2.emit("luma://transcript", &t);
                }
            });
            let r = assemblyai::transcribe(&key, &model, audio_rx, ev_tx).await;
            fwd.abort();
            r
        });
        *self.listening.lock().unwrap() =
            Some(Listening { epoch, mic: Some(mic), stt, snapshot, started: Instant::now() });
        let waiting = self.pending.lock().unwrap().is_some();
        self.status(app, Phase::Listening, waiting.then(|| "Listening for your answer…".to_string()));

        // Long-press cue: tell the overlay when the turn becomes "annotate".
        if prefs.annotate == "long_press" && !waiting {
            let this = self.clone();
            let app = app.clone();
            let ms = prefs.long_press_ms;
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                let still = this.listening.lock().unwrap().as_ref().is_some_and(|l| l.epoch == epoch);
                if still {
                    let _ = app.emit("luma://mode", "annotate");
                }
            });
        } else if prefs.annotate == "always" {
            let _ = app.emit("luma://mode", "annotate");
        }
    }

    pub fn on_release(self: &Arc<Self>, app: &AppHandle) {
        let Some(mut l) = self.listening.lock().unwrap().take() else { return };
        // Closing the mic closes the audio channel, which forces the end of the turn.
        drop(l.mic.take());
        let held = l.started.elapsed();
        let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
        let annotate = match prefs.annotate.as_str() {
            "always" => true,
            "never" => false,
            _ => held.as_millis() as u64 >= prefs.long_press_ms,
        };
        let _ = app.emit("luma://mode", if annotate { "annotate" } else { "voice" });
        self.status(app, Phase::Thinking, None);
        let this = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let transcript = match l.stt.await {
                Ok(Ok(t)) => t,
                Ok(Err(e)) => {
                    this.fail(&app, l.epoch, &e);
                    return;
                }
                Err(_) => return, // aborted by barge-in
            };
            if !this.current(l.epoch) {
                return;
            }
            if transcript.trim().is_empty() {
                l.snapshot.abort();
                let msg = if held.as_millis() < 250 { "Hold the shortcut while you talk." } else { "I didn't catch that." };
                let waiting = this.pending.lock().unwrap().is_some();
                this.status(&app, if waiting { Phase::Waiting } else { Phase::Idle }, Some(msg.into()));
                return;
            }
            let _ = app.emit("luma://question", &transcript);

            // A paused task gets first claim on the reply.
            if this.resume_pending(&app, l.epoch, &transcript) {
                l.snapshot.abort();
                return;
            }
            let raw = match l.snapshot.await {
                Ok(s) => s,
                Err(_) => return,
            };
            this.answer(&app, l.epoch, transcript, raw, annotate).await;
        });
    }

    /// Returns true if the reply was consumed by a paused task.
    fn resume_pending(self: &Arc<Self>, app: &AppHandle, epoch: u64, reply: &str) -> bool {
        let Some(p) = self.pending.lock().unwrap().take() else { return false };
        match p {
            Pending::Approval { task, action } => match yes_no(reply) {
                YesNo::Yes => {
                    tauri::async_runtime::spawn(agent::run(self.clone(), app.clone(), epoch, task, Some(action)));
                    true
                }
                YesNo::No => {
                    self.say(app, epoch, "Okay, I won't do that. I've stopped the task.");
                    self.finish_when_quiet(app, epoch);
                    true
                }
                // Something else entirely: drop the task, treat it as a new request.
                YesNo::Unclear => false,
            },
            Pending::Answer { mut task } => {
                task.history.push(format!("the user said: \"{reply}\""));
                task.last_changed = None;
                tauri::async_runtime::spawn(agent::run(self.clone(), app.clone(), epoch, task, None));
                true
            }
        }
    }

    /// Typed question (panel input). Same pipeline minus the microphone;
    /// typed questions always get annotations (no long-press possible).
    pub fn ask_text(self: &Arc<Self>, app: &AppHandle, question: String) {
        let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
        if prefs.paused {
            self.status(app, Phase::Paused, Some("LUMA is paused.".into()));
            return;
        }
        let epoch = self.interrupt(app);
        if get_key(Provider::Gemini).is_none() {
            self.status(app, Phase::Error, Some("Add your Gemini API key in LUMA settings.".into()));
            return;
        }
        if !screen::has_screen_permission() {
            let e = screen::request_screen_permission();
            self.status(app, Phase::Error, Some(e.to_string()));
            return;
        }
        let _ = app.emit("luma://question", &question);
        if self.resume_pending(app, epoch, &question) {
            return;
        }
        let snapshot = self.spawn_capture(app, &prefs);
        self.status(app, Phase::Thinking, None);
        let this = self.clone();
        let app = app.clone();
        let annotate = prefs.annotate != "never";
        tauri::async_runtime::spawn(async move {
            if let Ok(raw) = snapshot.await {
                this.answer(&app, epoch, question, raw, annotate).await;
            }
        });
    }

    fn spawn_capture(&self, app: &AppHandle, prefs: &Prefs) -> JoinHandle<Result<RawSnapshot>> {
        let app = app.clone();
        let prefs = prefs.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let displays = screen::displays()?;
            let pointer = screen::pointer(&app, &displays);
            if let Some(p) = pointer.and_then(|p| luma_core::geometry::display_at(&displays, p)) {
                let st = app.state::<AppState>();
                *st.companion.hud_display.lock().unwrap() = Some(p.index);
            }
            crate::sync_overlays(&app, &displays);
            if let Some(w) = screen::active_window() {
                if prefs.is_excluded(&w.app) {
                    return Err(anyhow!(Excluded(w.app)));
                }
            }
            screen::capture(displays, pointer, None)
        })
    }

    fn fail(&self, app: &AppHandle, epoch: u64, e: &anyhow::Error) {
        if !self.current(epoch) {
            return;
        }
        log::warn!("turn failed: {e:#}");
        self.status(app, Phase::Error, Some(assemblyai::redact(&format!("{e}"))));
    }

    pub fn gemini_for(&self, prefs: &Prefs, media_resolution: &'static str) -> Gemini {
        Gemini {
            client: self.http.clone(),
            api_key: get_key(Provider::Gemini).unwrap_or_default(),
            model: prefs.gemini_model.clone(),
            thinking_level: prefs.thinking_level.clone(),
            media_resolution,
        }
    }

    fn tts(&self, prefs: &Prefs) -> Option<SarvamTts> {
        get_key(Provider::Sarvam).filter(|_| self.speaker.is_some()).map(|k| SarvamTts {
            client: self.http.clone(),
            api_key: k,
            speaker: prefs.tts_speaker.clone(),
            language: prefs.tts_language.clone(),
            pace: prefs.tts_pace,
        })
    }

    // ------------------------------------------------------------ answer

    async fn answer(self: &Arc<Self>, app: &AppHandle, epoch: u64, question: String, raw: Result<RawSnapshot>, annotate: bool) {
        let raw = match raw {
            Ok(s) => s,
            Err(e) => {
                if let Some(x) = e.downcast_ref::<Excluded>() {
                    self.status(app, Phase::Idle, Some(x.to_string()));
                } else {
                    self.fail(app, epoch, &e);
                }
                return;
            }
        };
        let st = app.state::<AppState>();
        let prefs = st.prefs.lock().unwrap().clone();

        // Voice turns: one smaller image, medium detail. Annotate turns: full
        // detail plus the pointer close-up, needed for precise boxes.
        let opts = if annotate {
            CaptureOptions { max_edge: prefs.max_image_edge, closeup: prefs.send_closeup }
        } else {
            CaptureOptions { max_edge: 1280, closeup: false }
        };
        let snap: Snapshot = match tauri::async_runtime::spawn_blocking(move || raw.encode(&opts)).await {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return self.fail(app, epoch, &e),
            Err(_) => return,
        };
        let context_key = snap.context_key();
        let media = if annotate { "MEDIA_RESOLUTION_HIGH" } else { "MEDIA_RESOLUTION_MEDIUM" };

        let (body, prior_items, turn_no) = {
            let mut session = st.session.lock().unwrap();
            session.observe_level_request(&question);
            let marked = if annotate { session.describe_items(&context_key, &snap.images[0].sent, &snap.displays) } else { String::new() };
            let ctx = context_block(&TurnContext {
                app: Some(snap.window.app.as_str()).filter(|s| !s.is_empty()),
                window_title: Some(snap.window.title.as_str()),
                pointer: snap.pointer_norm,
                pointer_closeup: snap.pointer_norm_closeup,
                has_closeup: snap.images.len() > 1,
                display_count: snap.displays.len(),
                level: session.level,
                marked_items: &marked,
            });
            let history: Vec<HistoryTurn> =
                session.turns().map(|t| HistoryTurn { user: &t.user, assistant: &t.assistant }).collect();
            let images: Vec<ImagePart> = snap.images.iter().map(|i| ImagePart { jpeg: &i.jpeg }).collect();
            let body = self.gemini_for(&prefs, media).build_body(
                &system_prompt(annotate, prefs.can_act),
                &history,
                &ctx,
                &images,
                &question,
            );
            (body, session.items_for(&context_key), session.next_turn_number())
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let gemini = self.gemini_for(&prefs, media);
        let llm = tokio::spawn(async move { gemini.stream(&body, tx).await });

        let tts = self.tts(&prefs);
        if tts.is_none() {
            let _ = app.emit("luma://notice", "No Sarvam key: answers are shown as captions only.");
        }
        let seq: Arc<Mutex<Sequencer<Vec<u8>, Marker>>> = Arc::new(Mutex::new(Sequencer::new()));
        let limiter = Arc::new(Semaphore::new(3));
        let images: Vec<_> = snap.images.iter().map(|i| i.sent).collect();
        let mut resolver = Resolver::new(&snap.displays, &images, prior_items, turn_no);
        let mut parser = MarkupParser::new();
        let mut chunker = SpeechChunker::new();
        let mut spoken = String::new();
        let mut first_audio = true;
        let mut task_goal: Option<String> = None;

        let mut handle_segments = |segs: Vec<Segment>, this: &Arc<Self>, first_audio: &mut bool, task_goal: &mut Option<String>| {
            for s in segs {
                match s {
                    Segment::Text(t) => {
                        spoken.push_str(&t);
                        for u in chunker.push(&t) {
                            this.enqueue_utterance(app, epoch, &seq, &limiter, tts.clone(), u, first_audio);
                        }
                    }
                    Segment::Tag(tag) if tag.name == "task" => {
                        if let Some(g) = tag.attr("goal").map(str::trim).filter(|g| !g.is_empty()) {
                            task_goal.get_or_insert_with(|| g.to_string());
                        }
                    }
                    Segment::Tag(_) if !annotate => {} // voice-only turn: never draw
                    Segment::Tag(tag) => match resolver.resolve(&tag) {
                        Ok(a) => {
                            seq.lock().unwrap().push_marker(Marker::Annotate(a));
                            this.release(app, epoch, &seq);
                        }
                        Err(e) => log::debug!("dropped tag <{}>: {}", e.tag, e.reason),
                    },
                }
            }
        };

        while let Some(delta) = rx.recv().await {
            if !self.current(epoch) {
                llm.abort();
                return;
            }
            let segs = parser.push(&delta);
            handle_segments(segs, self, &mut first_audio, &mut task_goal);
        }
        let segs = parser.finish();
        handle_segments(segs, self, &mut first_audio, &mut task_goal);
        if let Some(u) = chunker.finish() {
            self.enqueue_utterance(app, epoch, &seq, &limiter, tts.clone(), u, &mut first_audio);
        }

        match llm.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return self.fail(app, epoch, &e),
            Err(_) => return,
        }
        if !self.current(epoch) {
            return;
        }

        // ---- remember this turn (text + marked items only)
        let items: Vec<_> = resolver.items.values().filter(|m| m.turn == turn_no).cloned().collect();
        let assistant = luma_core::speech::clean(&spoken).unwrap_or_default();
        st.session.lock().unwrap().record(
            Turn { user: question.clone(), assistant: assistant.clone(), context_key },
            items,
        );
        if !assistant.is_empty() {
            let _ = app.emit("luma://answer", &assistant);
        }

        // ---- hand off to the agent, or go idle when the speech queue drains
        let this = self.clone();
        let app2 = app.clone();
        let display = snap.display.index;
        tokio::spawn(async move {
            this.wait_queue_drained(&seq, epoch).await;
            if !this.current(epoch) {
                return;
            }
            match task_goal {
                Some(goal) if prefs.can_act => {
                    agent::run(this.clone(), app2.clone(), epoch, TaskState::new(goal, Some(display)), None).await;
                }
                _ => {
                    if assistant.is_empty() {
                        this.status(&app2, Phase::Idle, Some("I don't have an answer for that.".into()));
                    } else {
                        this.finish_when_quiet(&app2, epoch);
                    }
                }
            }
        });
    }

    async fn wait_queue_drained(&self, seq: &Arc<Mutex<Sequencer<Vec<u8>, Marker>>>, epoch: u64) {
        loop {
            if !self.current(epoch) || seq.lock().unwrap().is_empty() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
    }

    /// Go idle once everything queued on the speaker has played.
    pub fn finish_when_quiet(self: &Arc<Self>, app: &AppHandle, epoch: u64) {
        match &self.speaker {
            Some(s) if get_key(Provider::Sarvam).is_some() => {
                let this = self.clone();
                let app = app.clone();
                s.send(SpeakerCmd::Callback(Box::new(move || {
                    if this.current(epoch) {
                        this.speaking.store(false, Ordering::SeqCst);
                        this.status(&app, Phase::Idle, None);
                    }
                })));
            }
            _ => {
                self.speaking.store(false, Ordering::SeqCst);
                self.status(app, Phase::Idle, None);
            }
        }
    }

    /// Speak one line outside the streaming pipeline, without waiting.
    pub fn say(self: &Arc<Self>, app: &AppHandle, epoch: u64, text: &str) {
        let this = self.clone();
        let app = app.clone();
        let text = text.to_string();
        tauri::async_runtime::spawn(async move { this.speak(&app, epoch, &text).await });
    }

    /// Speak one line; returns once its audio is queued on the speaker, so
    /// anything queued afterwards plays after it.
    pub async fn speak(self: &Arc<Self>, app: &AppHandle, epoch: u64, text: &str) {
        let text = text.trim().to_string();
        if text.is_empty() || !self.current(epoch) {
            return;
        }
        let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
        let Some(tts) = self.tts(&prefs) else {
            let _ = app.emit("luma://caption", &text);
            return;
        };
        match tts.synthesize(&text).await {
            Ok(audio) if self.current(epoch) => {
                if let Some(s) = &self.speaker {
                    let app2 = app.clone();
                    let caption = text.clone();
                    s.send(SpeakerCmd::Callback(Box::new(move || {
                        let _ = app2.emit("luma://caption", &caption);
                    })));
                    s.send(SpeakerCmd::Play(audio));
                }
            }
            Ok(_) => {}
            Err(e) => {
                log::warn!("tts failed: {e}");
                let _ = app.emit("luma://caption", &text);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_utterance(
        self: &Arc<Self>,
        app: &AppHandle,
        epoch: u64,
        seq: &Arc<Mutex<Sequencer<Vec<u8>, Marker>>>,
        limiter: &Arc<Semaphore>,
        tts: Option<SarvamTts>,
        text: String,
        first_audio: &mut bool,
    ) {
        let id = {
            let mut s = seq.lock().unwrap();
            s.push_marker(Marker::Caption(text.clone()));
            s.push_speech(text.clone())
        };
        if *first_audio {
            *first_audio = false;
            self.speaking.store(true, Ordering::SeqCst);
            self.status(app, Phase::Speaking, None);
        }
        let Some(tts) = tts else {
            seq.lock().unwrap().fulfill(id, None);
            self.release(app, epoch, seq);
            return;
        };
        let this = self.clone();
        let app = app.clone();
        let seq = seq.clone();
        let limiter = limiter.clone();
        tokio::spawn(async move {
            let _permit = limiter.acquire().await;
            if !this.current(epoch) {
                return;
            }
            let audio = match tts.synthesize(&text).await {
                Ok(a) => Some(a),
                Err(e) => {
                    log::warn!("tts failed: {e}");
                    let _ = app.emit("luma://notice", "Voice unavailable for part of this answer.");
                    None
                }
            };
            seq.lock().unwrap().fulfill(id, audio);
            this.release(&app, epoch, &seq);
        });
    }

    /// Move everything that is ready from the sequencer to the speaker.
    fn release(self: &Arc<Self>, app: &AppHandle, epoch: u64, seq: &Arc<Mutex<Sequencer<Vec<u8>, Marker>>>) {
        if !self.current(epoch) {
            return;
        }
        let ready = seq.lock().unwrap().drain_ready();
        for r in ready {
            match (r, &self.speaker) {
                (Released::Audio { audio, .. }, Some(s)) => s.send(SpeakerCmd::Play(audio)),
                (Released::Audio { .. }, None) => {}
                (Released::Marker(m), Some(s)) => {
                    let this = self.clone();
                    let app = app.clone();
                    s.send(SpeakerCmd::Callback(Box::new(move || {
                        if this.current(epoch) {
                            this.emit_marker(&app, &m);
                        }
                    })));
                }
                (Released::Marker(m), None) => self.emit_marker(app, &m),
            }
        }
    }

    fn emit_marker(&self, app: &AppHandle, m: &Marker) {
        match m {
            Marker::Caption(t) => {
                let _ = app.emit("luma://caption", t);
            }
            Marker::Annotate(a) => {
                let target = match a {
                    Annotation::Shape { display, .. }
                    | Annotation::Point { display, .. }
                    | Annotation::Arrow { display, .. }
                    | Annotation::Step { display, .. }
                    | Annotation::Spotlight { display, .. }
                    | Annotation::Zoom { display, .. }
                    | Annotation::Focus { display, .. }
                    | Annotation::Label { display, .. } => Some(*display),
                    Annotation::Clear { .. } => None,
                };
                match target {
                    Some(d) => {
                        let _ = app.emit_to(format!("overlay-{d}"), "luma://annotate", a);
                        // (re)arm the watcher; a new mark resets the baseline
                        let key = screen::active_window().map(|w| w.key()).unwrap_or_default();
                        *self.watch.lock().unwrap() =
                            Some(Watch { display: d, window_key: key, baseline: None, last_mark: Instant::now() });
                    }
                    None => {
                        let _ = app.emit("luma://annotate", a);
                    }
                }
            }
        }
    }
}
