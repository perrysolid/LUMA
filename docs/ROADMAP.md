# Roadmap

Each phase ends buildable, tested, and usable. ✅ done · 🔜 next · ⏳ later

## Phase 1 — Voice + see + point (✅ this slice)
- ✅ Tauri 2 app for Mac and Windows: tray, panel, hold-to-talk global shortcut
- ✅ AssemblyAI streaming STT with forced endpoint on release
- ✅ Gemini streaming with full-screen + pointer close-up images and compact context
- ✅ Sarvam Bulbul v3 TTS, pipelined per utterance; captions-only fallback
- ✅ Inline tag protocol: box, circle, highlight, underline, point, arrow, step, label, spotlight, zoom, focus, clear
- ✅ Coordinate engine covering Retina, mixed DPI, negative origins and portrait displays (unit tested)
- ✅ Per-display overlays: animated SVG scene, label placement, curved arrows, flying pointer, HUD
- ✅ Annotations synced to speech via the playback queue; barge-in
- ✅ Session memory: text, marked ids, explanation level, window scoping
- ✅ Privacy: keychain keys, excluded apps, pause, forget, content-protected overlays
- ✅ Eval harness: rendered fixtures, ScreenSpot-style hit rate, IoU, flow order, ambiguity, latency

## Phase 1.5 — Modes, tasks, hygiene (✅)
- ✅ Voice-only quick press (1280 px, medium media resolution, no drawing) vs annotate long press (full detail + close-up); "always/never" options
- ✅ Pointer drawn on the images as a ring: "this" questions 100% on the eval
- ✅ Agent: `<task>` routing, then observe → one action → show → policy → act → verify by screen diff, with up to 25 steps
- ✅ Actions: click, double/right click, type, keys, scroll, open URL, wait, ask, done, fail
- ✅ Safety: approval for committing actions (code-enforced keyword policy + model risk flag), never types secrets, http(s) URLs only, stop by pressing the shortcut, on-screen text treated as data
- ✅ Pausing tasks: spoken yes/no for approvals; answers to the agent's questions resume the task
- ✅ Stale overlays cleared on window switch or content change (thumbnail diff)
- ✅ Tray: "Check overlay alignment"
- ✅ Eval: routing and agent first-step cases; 24/24 on gemini-3.8-flash

## Phase 2 — Precision grounding 🔜
- Accessibility snapping. Read the element tree (AXUIElement on macOS, UI
  Automation on Windows) around the pointer and the predicted boxes, and snap a
  model box to the element frame with the best IoU and matching role/name.
  Native controls then get pixel-exact boxes.
- Selected text and focused element in context (AX `AXSelectedText`, UIA `TextPattern`)
- Secure-field detection: black out `AXSecureTextField` / `IsPassword` regions before upload
- Zoom-and-refine pass for targets under ~24 pt: re-ask on a tight crop
- On-device OCR (Apple Vision / Windows.Media.Ocr) to snap text highlights to real line boxes
- True magnifier for `<zoom>`: send the captured crop to the overlay

## Phase 3 — Teaching mode ⏳
- A "lesson" state machine on the session: goal → step → expected screen state
- "Let me try": after each step, poll cheap screen diffs and re-check only when the screen changes
- Correct with a pointer and a one-line reason; "Why?", "Go deeper", "Continue from where we left off" (session persisted locally, opt-in)

## Phase 4 — Acting on the computer (core shipped in 1.5; next:)
- Gemini Computer Use tool (Gemini 3.5+ Flash) for planning; actions executed by LUMA, never by the model directly
- Executor: AX actions first (`AXPress`, UIA `Invoke`), synthetic input fallback (CGEvent / SendInput)
- Verify after every action: screen diff plus AX state plus a model check of the expected change. Retry with another strategy, or ask.
- Risk tiers: observe/annotate (auto) · navigate (auto) · modify (confirm) · send, publish, pay, delete, credentials, security settings (explicit approval with a spoken and visual summary)
- Action history: "what did you change?", "undo that" via app undo where available, honest reporting where it is not
- Prompt-injection guard: on-screen text is data. Actions are planned only from the user's request.

## Phase 5 — Latency and presence ⏳
- Optional Gemini Live (native audio) "fast mode" with the same tag protocol over the transcript channel
- Sarvam WebSocket TTS for the first utterance
- Warm STT connection for a few seconds after a turn (follow-ups start instantly)
- On-device STT fallback when AssemblyAI credits run out (Apple SpeechAnalyzer / Windows speech)
- Open-mic mode with echo cancellation (macOS voice-processing IO, Windows AEC)

## Phase 6 — Distribution ⏳
- GitHub Actions release builds (workflow included); signing with an Apple Developer ID and notarization; Windows Trusted Signing
- Auto-update (Tauri updater)
- Optional key proxy with short-lived tokens, for users who should not bring their own keys

## Eval targets
| Metric | Gate before a release |
|---|---|
| Grounding first-hit (locate + refer) | ≥ 85% |
| Flow coverage / order | ≥ 0.8 / ≥ 0.8 |
| Ambiguity handled | ≥ 90% |
| Time to first token p50 | < 1.5 s |
