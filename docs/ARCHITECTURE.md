# LUMA architecture

LUMA is a desktop companion for macOS and Windows. Hold a shortcut, talk, and it
answers out loud while drawing boxes, arrows, labels and steps directly over
your real screen.

## Shape of the system

```
┌──────────────────────────────── Tauri app (Rust) ─────────────────────────────────┐
│ hotkey ─► companion.rs (turn loop, epochs for barge-in)                           │
│             │                                                                     │
│             ├─ audio.rs     mic → 16 kHz PCM ─► luma-net::assemblyai (WS stream)  │
│             ├─ screen.rs    displays, pointer, active window, capture             │
│             │                └─► luma-net::vision (full image + pointer close-up) │
│             ├─ luma-net::gemini  streamGenerateContent (SSE)                      │
│             │      text ─► luma-core::markup ─► speech text │ visual tags         │
│             │                   speech ─► luma-core::speech ─► luma-net::sarvam   │
│             │                   tags   ─► luma-core::annotation::Resolver         │
│             └─ luma-core::sequencer  orders audio + annotations; playback         │
│                callbacks fire each annotation as its sentence starts              │
│                                                                                   │
│  settings.rs  keys in OS keychain; prefs JSON                                     │
└───────────────┬───────────────────────────────────────────────────────────────────┘
                │ events (display-space geometry only, never keys)
     ┌──────────┴───────────┐        ┌────────────────────────┐
     │ overlay-N windows    │        │ panel window           │
     │ one per display,     │        │ settings, typed asks,  │
     │ transparent, click-  │        │ conversation log       │
     │ through, excluded    │        └────────────────────────┘
     │ from capture         │
     │ Scene (SVG) + HUD    │
     └──────────────────────┘
```

| Crate / dir | Responsibility | Why separate |
|---|---|---|
| `crates/luma-core` | Coordinate spaces, tag parser, annotation resolver, speech chunking, sequencing, session memory, prompt, resampler | Pure and deterministic, so it is fully unit tested. No OS, network or clock. |
| `crates/luma-net` | Gemini, AssemblyAI and Sarvam clients; image preparation | Shared by the app and the eval, so the eval measures the real pipeline. |
| `crates/luma-eval` | Grounding/explanation benchmark against rendered fixtures | Runs headless with a key; regression gate for prompt and model changes. |
| `src-tauri` | OS integration: hotkey, mic, speaker, capture, windows, tray, keychain | The only code that differs per OS. |
| `src/overlay` | Annotation renderer and status HUD | The visual layer evolves independently of grounding. |
| `src/panel` | Settings and conversation UI | |

## Key decisions

**Tauri 2 (Rust + system webview), not Electron or native Swift.**
Mac and Windows from one codebase, installers around 10 MB, low memory use.
Rust gives direct access to each OS's capture, input and accessibility APIs. SVG
in a webview is the best tool for smooth, crisp, animated annotations.

**All network calls and keys stay in Rust.** Sarvam and AssemblyAI authenticate
with headers that a webview cannot set safely. Keeping network code in Rust also
means a key can never reach page JavaScript, logs or the UI. Keys live in the OS
credential store, and errors are passed through `redact()`.

**Cascaded voice pipeline (AssemblyAI → Gemini → Sarvam), streamed end to end.**
The STT connection opens and the screenshot is taken the moment the hotkey goes
down, so both are hidden behind your own speech. `ForceEndpoint` on key release
returns the final transcript immediately. Gemini streams; the first utterance is
cut at the first clause and synthesized while the model is still writing. TTS
requests are pipelined, three at a time. Gemini Live (native audio) would be
lower latency still. Voice is an isolated stage, so it can be added as a "fast
mode" without touching grounding or the overlay.

**Inline visual tags instead of function calls.** The model writes
`<box id="db" box="…" label="Database"/>` immediately before the words about that
element. One streamed response carries speech and geometry in the right order,
with no extra round trips. The parser is incremental, tolerates chunk splits at
any byte, and treats unknown `<…>` as text so code and maths are never swallowed.

**Explicit coordinate spaces** (`luma-core/src/geometry.rs`). The model speaks in
0–1000 coordinates relative to *the image it saw*. The resolver maps
image → capture pixels → display view points. Overlays are one per display and
draw in view points, so mixed-DPI and negative-origin multi-monitor layouts need
no global math. Input space (points on macOS, physical pixels on Windows) is
used only for the pointer, and later for synthetic input. Scale is derived from
the capture's actual size rather than the reported scale factor, which survives
OS-side downscaling.

**Two images, never a dump.** Each turn sends the full display (long edge ≤1920)
plus a native-resolution close-up of about 480 points around the pointer. The
close-up is what makes small icons and "this" reliable. Context is a short text
block: app, title, pointer coordinates, level, and previously marked ids.

**Annotations are synchronized by the audio queue itself.** Speech clips and
annotation callbacks share one ordered rodio queue, so a box appears exactly
when its sentence begins, regardless of network ordering
(`luma-core/src/sequencer.rs`).

**Barge-in by epoch.** Every turn has an epoch. Pressing the hotkey increments
it, clears the playback queue (including pending annotation callbacks), and
every task checks the epoch before acting.

**Memory is text only.** The session keeps user and assistant text plus marked
item ids, labels and geometry, scoped to the app and window they were made in.
That is enough for "the one you just explained". Screenshots and audio are
dropped after each turn.

## Privacy model

- LUMA captures only while the hotkey is held or a typed question is sent, and
  only the display under the pointer.
- The on-screen HUD always shows listening, looking, thinking or talking. The
  tray offers Pause and "Forget this session".
- Excluded apps (password managers by default) are checked before capture. If
  one is in front, nothing is captured.
- Overlays are content-protected (`NSWindowSharingNone` /
  `WDA_EXCLUDEFROMCAPTURE`), so LUMA's own drawings never enter the screenshots.
- No screenshots or audio are written to disk. Logs never contain keys,
  transcripts or images.
