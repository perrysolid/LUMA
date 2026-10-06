# Research notes (October 2026)

What we looked at and what we took from it. Kept short on purpose.

## Starting point: Clicky (farzaa/clicky)
- Swift and macOS only. Push-to-talk → AssemblyAI → Claude (SSE) → ElevenLabs.
  It points by embedding `[POINT:x,y:label:screenN]` in the text, and keys sit
  behind a Cloudflare Worker.
- **Kept:** the push-to-talk feel; inline markup for pointing (cheap, in-order,
  streamable); the companion pointer that flies to targets.
- **Replaced:** single-point output became a full annotation vocabulary with
  ids. Raw pixel guesses became normalized boxes on a known image plus explicit
  coordinate transforms. Mac-only Swift became a cross-platform Rust/Tauri app.
  The Worker became per-user keychain keys (no server to run).
- **Forks:** openclicky (jasonkneen) adds local agent work and a gallery. It
  confirmed the demand for acting, not just pointing (Phase 4).

## Models and APIs (checked against official docs on 2026-10-06)
- **Gemini 3.8 Flash** (`gemini-3.8-flash`): current fast multimodal model.
  Spatial output is `box_2d [ymin, xmin, ymax, xmax]` normalized to 0–1000; we
  use the same convention in tags. `thinkingLevel` is minimal/low/medium/high,
  and we default to *low* (spatial tasks degrade at high latency for little
  gain). `mediaResolution` HIGH improves small-text and icon grounding. The docs
  recommend leaving temperature at its default.
- **Gemini Live** (`gemini-3.8-live`): stateful WebSocket for native audio.
  Lowest latency, but video frames are low-resolution and around 1 fps, too
  coarse for precise boxes. Planned as an optional voice "fast mode" (Phase 5),
  with grounding still done on full-resolution stills.
- **Computer use**: now a tool on Gemini 3.5+ Flash. The 2.5 computer-use
  preview model is shut down. This is the planner for Phase 4.
- **AssemblyAI Universal-3.5 Pro streaming** (`wss://streaming.assemblyai.com/v3/ws`):
  about 300 ms to final transcript, `ForceEndpoint` for push-to-talk, header auth.
- **Sarvam Bulbul v3**: 30+ voices, Indian English and 10 Indic languages, REST
  and WebSocket. REST per utterance gives exact sentence-to-audio alignment,
  which our sync relies on.

## Ideas adopted from GUI-grounding and computer-use work
- **ScreenSpot / ScreenSpot-Pro** score grounding as "click accuracy": is the
  predicted point inside the target? Our eval's primary metric is the same.
  ScreenSpot-Pro shows that small targets on high-resolution professional
  screens are the main failure mode. That motivates the pointer close-up and
  the zoom-and-refine pass (Phase 2).
- **OSWorld / Windows Agent Arena**: long-horizon success rates for agents
  remain modest. So acting must be verify-after-every-step with cheap recovery
  and an easy "ask the user"; pretending success is the worst outcome.
- **Set-of-Mark style prompting** (marking candidate elements with ids) improves
  reference resolution. Our ids on marked items and `step` candidates for
  ambiguity serve the same purpose conversationally.
- **Accessibility-tree agents** (e.g. UFO on Windows, AX-based macOS agents)
  get exact frames for native controls but miss canvas content such as slides,
  diagrams and charts. Hence a hybrid: vision for semantics and canvases, the
  accessibility tree to snap native controls (Phase 2).

## Things we deliberately did not do
- Continuous screen streaming: costly, a privacy risk, and unnecessary for
  turn-based help. We capture on intent.
- Hard-coded app integrations. The only planned one is AppleScript/COM for
  PowerPoint/Keynote shape geometry, where it is strictly more reliable than
  vision.
- A server holding user keys.

## Measured on LUMA's eval (2026-10-06, 18 cases, 1920 px + pointer close-up)

| Config | Pass | Grounding first-hit | Mean IoU | TTFT p50 |
|---|---|---|---|---|
| gemini-3.8-flash, thinking low | **100%** | **100%** | 0.81 | 3.4 s |
| gemini-3.5-flash, minimal | 78% | 80% | 0.79 | 2.2 s |
| gemini-3.6-flash, minimal* | 67% | 67% | 0.57 | 2.1 s |
| gemini-3.1-flash-lite, minimal* | 67% | 67% | 0.56 | 1.7 s |

\* measured before the pointer ring was added.

- gemini-3.8-flash rejects `thinkingLevel: minimal` and ignores `thinkingBudget: 0`.
  It always thinks, so the client falls back from minimal to low.
- **Drawing the pointer as a ring on the images** (screenshots never include the
  cursor) was the biggest single gain: "this" questions went from frequent
  wrong-neighbour picks to 100% on 3.8-flash. Coordinates alone are a weak cue.
- Shrinking the full image from 1920 to 1440 px did not lower latency, because
  thinking dominates, so 1920 px stays.
- Default: Accurate (3.8-flash). Fast mode (3.5-flash) is available in Settings.
