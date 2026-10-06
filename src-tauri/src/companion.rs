//! The turn loop: hotkey → listen → see → think → speak + draw.
//!
//! Latency strategy:
//! * Screen capture and the STT connection both start the instant the hotkey
//!   goes down, so they are hidden behind the user's own speech.
//! * Gemini output is streamed; speech is cut into utterances as it arrives
//!   and synthesized concurrently, so the first words play while the model is
//!   still writing.
//! * Annotations ride in the same ordered queue as the audio and fire via
//!   playback callbacks, so they appear exactly as the related sentence starts.
//!
//! Barge-in: every turn carries an epoch. Pressing the hotkey again bumps the
//! epoch, stops playback, and every in-flight task notices and quits.

use crate::audio::{start_mic, MicHandle, Speaker, SpeakerCmd};
use luma_net::assemblyai::{self, SttEvent};
use luma_net::gemini::{Gemini, HistoryTurn, ImagePart};
use luma_net::sarvam::SarvamTts;
use crate::screen::{self, CaptureOptions, Snapshot};
use crate::settings::{get_key, Prefs, Provider};
use crate::AppState;
use anyhow::{anyhow, Result};
use luma_core::annotation::{Annotation, Resolver};
use luma_core::markup::{MarkupParser, Segment};
use luma_core::prompt::{context_block, TurnContext, SYSTEM_PROMPT};
use luma_core::sequencer::{Released, Sequencer};
use luma_core::session::Turn;
use luma_core::speech::SpeechChunker;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, Semaphore};
use tauri::async_runtime::JoinHandle;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Idle,
    Listening,
    Thinking,
    Speaking,
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
    snapshot: JoinHandle<Result<Snapshot>>,
    started: Instant,
}

pub struct Companion {
    epoch: AtomicU64,
    speaker: Option<Speaker>,
    http: reqwest::Client,
    listening: Mutex<Option<Listening>>,
    hud_display: Mutex<Option<usize>>,
}

/// The app's foreground app was on the exclusion list.
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
        }
    }

    fn current(&self, epoch: u64) -> bool {
        self.epoch.load(Ordering::SeqCst) == epoch
    }

    /// Stop talking, clear the screen, cancel everything in flight.
    pub fn interrupt(&self, app: &AppHandle) -> u64 {
        let e = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(s) = &self.speaker {
            s.stop();
        }
        if let Some(l) = self.listening.lock().unwrap().take() {
            l.stt.abort();
            l.snapshot.abort();
        }
        let _ = app.emit("luma://clear", ());
        e
    }

    pub fn status(&self, app: &AppHandle, phase: Phase, message: Option<String>) {
        let display = *self.hud_display.lock().unwrap();
        let _ = app.emit("luma://status", StatusEvent { phase, message, display });
    }

    pub fn on_press(self: &Arc<Self>, app: &AppHandle) {
        let prefs = app.state::<AppState>().prefs.lock().unwrap().clone();
        if prefs.paused {
            self.status(app, Phase::Paused, Some("LUMA is paused. Resume it from the menu bar.".into()));
            return;
        }
        let epoch = self.interrupt(app);
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
        let snapshot = self.spawn_snapshot(app, &prefs);
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
        self.status(app, Phase::Listening, None);
    }

    pub fn on_release(self: &Arc<Self>, app: &AppHandle) {
        let Some(mut l) = self.listening.lock().unwrap().take() else { return };
        // Closing the mic closes the audio channel, which makes the STT
        // client force the end of the turn.
        drop(l.mic.take());
        let too_short = l.started.elapsed().as_millis() < 250;
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
                let msg = if too_short { "Hold the shortcut while you talk." } else { "I didn't catch that." };
                this.status(&app, Phase::Idle, Some(msg.into()));
                return;
            }
            let _ = app.emit("luma://question", &transcript);
            let snap = match l.snapshot.await {
                Ok(s) => s,
                Err(_) => return,
            };
            this.answer(&app, l.epoch, transcript, snap).await;
        });
    }

    /// Typed question (panel input). Same pipeline minus the microphone.
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
        let snapshot = self.spawn_snapshot(app, &prefs);
        self.status(app, Phase::Thinking, None);
        let _ = app.emit("luma://question", &question);
        let this = self.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(snap) = snapshot.await {
                this.answer(&app, epoch, question, snap).await;
            }
        });
    }

    fn spawn_snapshot(&self, app: &AppHandle, prefs: &Prefs) -> JoinHandle<Result<Snapshot>> {
        let app = app.clone();
        let opts = CaptureOptions { max_edge: prefs.max_image_edge, closeup: prefs.send_closeup };
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
            screen::snapshot(displays, pointer, &opts)
        })
    }

    fn fail(&self, app: &AppHandle, epoch: u64, e: &anyhow::Error) {
        if !self.current(epoch) {
            return;
        }
        log::warn!("turn failed: {e:#}");
        self.status(app, Phase::Error, Some(assemblyai::redact(&format!("{e}"))));
    }

    async fn answer(self: &Arc<Self>, app: &AppHandle, epoch: u64, question: String, snap: Result<Snapshot>) {
        let snap = match snap {
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
        let context_key = snap.context_key();

        // ---- build the request (text-only memory + this turn's images)
        let (body, prior_items, turn_no) = {
            let mut session = st.session.lock().unwrap();
            session.observe_level_request(&question);
            let marked = session.describe_items(&context_key, &snap.images[0].sent, &snap.displays);
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
            let gemini = self.gemini(&prefs);
            let images: Vec<ImagePart> = snap.images.iter().map(|i| ImagePart { jpeg: &i.jpeg }).collect();
            let body = gemini.build_body(SYSTEM_PROMPT, &history, &ctx, &images, &question);
            (body, session.items_for(&context_key), session.next_turn_number())
        };

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let gemini = self.gemini(&prefs);
        let llm = tokio::spawn(async move { gemini.stream(&body, tx).await });

        let tts = get_key(Provider::Sarvam).map(|k| SarvamTts {
            client: self.http.clone(),
            api_key: k,
            speaker: prefs.tts_speaker.clone(),
            language: prefs.tts_language.clone(),
            pace: prefs.tts_pace,
        });
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

        let mut handle_segments = |segs: Vec<Segment>, this: &Arc<Self>, first_audio: &mut bool| {
            for s in segs {
                match s {
                    Segment::Text(t) => {
                        spoken.push_str(&t);
                        for u in chunker.push(&t) {
                            this.enqueue_utterance(app, epoch, &seq, &limiter, tts.clone(), u, first_audio);
                        }
                    }
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
            handle_segments(segs, self, &mut first_audio);
        }
        let segs = parser.finish();
        handle_segments(segs, self, &mut first_audio);
        if let Some(u) = chunker.finish() {
            self.enqueue_utterance(app, epoch, &seq, &limiter, tts.clone(), u, &mut first_audio);
        }

        match llm.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.fail(app, epoch, &e);
                return;
            }
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
        let _ = app.emit("luma://answer", &assistant);
        if assistant.is_empty() {
            self.status(app, Phase::Idle, Some("I don't have an answer for that.".into()));
        }

        // ---- when the queue drains, go idle
        let this = self.clone();
        let app2 = app.clone();
        let seq2 = seq.clone();
        tokio::spawn(async move {
            loop {
                if !this.current(epoch) {
                    return;
                }
                if seq2.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
            match &this.speaker {
                Some(s) if tts_present(&app2) => {
                    let this2 = this.clone();
                    let app3 = app2.clone();
                    s.send(SpeakerCmd::Callback(Box::new(move || {
                        if this2.current(epoch) {
                            this2.status(&app3, Phase::Idle, None);
                        }
                    })));
                }
                _ => this.status(&app2, Phase::Idle, None),
            }
        });
    }

    fn gemini(&self, prefs: &Prefs) -> Gemini {
        Gemini {
            client: self.http.clone(),
            api_key: get_key(Provider::Gemini).unwrap_or_default(),
            model: prefs.gemini_model.clone(),
            thinking_level: prefs.thinking_level.clone(),
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
            self.status(app, Phase::Speaking, None);
        }
        let Some(tts) = tts.filter(|_| self.speaker.is_some()) else {
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
                            emit_marker(&app, &m);
                        }
                    })));
                }
                (Released::Marker(m), None) => emit_marker(app, &m),
            }
        }
    }
}

fn tts_present(_app: &AppHandle) -> bool {
    get_key(Provider::Sarvam).is_some()
}

fn emit_marker(app: &AppHandle, m: &Marker) {
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
                }
                None => {
                    let _ = app.emit("luma://annotate", a);
                }
            }
        }
    }
}
