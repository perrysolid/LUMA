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

## Phase 2 — Precision grounding ✅
- ✅ Accessibility snapping (macOS): one AX hit test at each box/circle/step/point
  plus its ancestors (a few ms); the frame with the best IoU and matching name
  replaces the model's box, so native controls get pixel-exact edges. Panes and
  windows never win; weak matches keep the model's box. Pure logic in
  `luma_core::snap`, unit tested. Needs Accessibility access, silently off without it.
- ✅ Same for Windows via UI Automation `ElementFromPoint` + control-view parents
  (150 ms connection/transaction timeouts). Type-checked; not yet run on a Windows machine.
- ✅ Chromium/Electron apps (Chrome, Slack, VS Code…) get their accessibility tree switched on (`AXManualAccessibility`)
- ✅ Local voice commands, no model call: "never mind" / "stop" cancels, "repeat that" /
  "say that again" / "phir se" replays the last answer with its drawings
- ✅ Keyboard focus and selected text in the context block (AX `AXFocusedUIElement` / `AXSelectedText`,
  UIA `GetFocusedElement` / `TextPattern`); "this" prefers the selection
- ✅ Password fields blacked out in every capture before it leaves the machine
  (AX `AXSecureTextField`, UIA `IsPassword`; bounded walk ≤ 600 elements / 80 ms)
- ✅ Zoom-and-refine: marks under 24 pt that AX did not snap are re-asked on a native-resolution
  crop (fast model) and swapped in when the answer is consistent (`luma_core::refine`)
- ✅ On-device OCR (Apple Vision / Windows.Media.Ocr) snaps highlights and underlines to real text lines
- ✅ True magnifier for `<zoom>`: the native-resolution crop is shown as an inset beside the target

## Phase 3 — Teaching mode ✅
- ✅ "Teach me how to…" → `<lesson goal>` → one step at a time: point + say → watch thumbnail diffs →
  check after the screen settles → next step, a gentle correction, or done (`luma_core::lesson`, `src-tauri/src/lesson.rs`)
- ✅ Pressing the shortcut pauses for a question ("why?", "go deeper"); the lesson resumes by itself,
  or on "continue" / "next" / "where were we"; "stop the lesson" ends it
- ✅ Opt-in: remember lesson progress (text only) for "continue from where we left off" after a restart
- ✅ Eval: routing (teach vs do vs explain) and first-step pointing, 100%

## Phase 4 — Acting on the computer ✅
- ✅ Executor: accessibility press first (`AXPress`, UIA `Invoke`) for buttons, links, menu items;
  synthetic click as the fallback; the cursor still glides there so the user sees it
- ✅ Verify after every action: screen diff + reading the focused field back after typing
  ("verified" / "did not land"), both fed to the next model step
- ✅ Risk tiers: security/access actions (2FA, passkeys, authorize, invite, make public…) need approval
- ✅ Action history: "what did you change?" reads back the last task's changes; "undo that" presses
  undo once in the same app, checks the screen changed and reports honestly when it did not
- ✅ Prompt-injection guard in code: emails, handles, long numbers and web addresses in a task goal
  must come from the user's words (or the app they are in), otherwise the task is refused
- Decided against: the Gemini Computer Use tool. It is browser-scoped, while LUMA acts on the whole
  desktop, and the tag protocol already passes every agent eval case.

## Phase 5 — Latency and presence ✅
- ✅ Fast mode (opt-in): Gemini Live native audio over one websocket per turn; push-to-talk via
  manual activity detection; drawing through a non-blocking `draw` function resolved like tags;
  first audio ~0.6 s after release (smoke test) vs ~3 s for the standard pipeline; 88% on the eval.
  Tasks, lessons and typed questions stay on the standard pipeline; falls back for 5 min if Live fails
- ✅ Sarvam WebSocket TTS: one stream per turn, ~200 ms to first audio, gapless PCM playback
- ✅ Warm STT: a session is opened after each answer and reused for a follow-up within 6 s (~1 s saved)
- ✅ On-device STT fallback when AssemblyAI fails or has no key (macOS Speech, on-device only; the
  turn's audio is kept in memory only). Windows: not yet (no recorded-audio API in Windows speech)
- ✅ Conversation mode (opt-in): after LUMA finishes speaking it listens hands-free for a few seconds,
  including for yes/no to a task's question. The mic opens only after LUMA stops talking.
- Not done: interrupting LUMA by voice *while it speaks*. That needs echo cancellation (macOS
  voice-processing I/O, Windows AEC) inside the audio stack; pressing the shortcut still interrupts.

## Freehand diagrams ✅
- ✅ LUMA draws its own diagrams on a whiteboard panel: `<board>`, `<node>` (box with text),
  `<arrow>` between nodes, `<sketch>` (smooth freehand stroke, open or closed), `<point>`;
  arrows that name a node not drawn yet wait for it. Also in fast mode via the draw function.
- ✅ Eval `sketch` (board, ≥3 readable non-overlapping nodes inside it, ≥2 arrows) and `draws`
  (arrows / pointer actually resolve): 100% in standard, quick and fast modes
- ✅ In-place annotation of drawings, videos and whiteboards: lines are traced with marker strokes in the
  drawing's own colours, never boxed or covered; `luma_core::ink` snaps traces onto the real ink
  (only segments that run along a drawn line; new shapes are left as drawn). Eval `trace`: 12/12
- ✅ The board is see-through, and a board that lands on busy content moves (with its diagram) to the
  emptiest area of the same size

## Responsiveness
- ✅ The companion cursor no longer depends on requestAnimationFrame, which WebKit can pause in
  transparent overlays (it then never moved). Overlays log their frame health to the app log
- ✅ Accessibility / OCR snapping and refine run in the background; they never delay speech
- ✅ Hedged answers: if the accurate model is silent for 6 s, the fast model starts too and the first
  to answer wins (p90 first token 18 s → 7 s on the eval); a spoken "one moment" after 4 s of silence

## Phase 6 — Distribution ✅ (needs your accounts to switch on)
- ✅ Release workflow: macOS (arm64 + x64) and Windows; Developer ID signing + notarization,
  Windows Trusted Signing and signed updater artifacts, each enabled by its GitHub secrets
- ✅ Hardened-runtime entitlements (microphone) and Info.plist usage strings
- ✅ Auto-update (Tauri updater, minisign-verified): checks at launch and every 6 h; offered in the tray, never silent
- ✅ Optional key proxy (`crates/luma-proxy`): keys stay on a server; devices get bearer tokens with
  access to exactly LUMA's calls, rate-limited; short-lived AssemblyAI tokens. See docs/DISTRIBUTION.md

## Eval targets
| Metric | Gate before a release |
|---|---|
| Grounding first-hit (locate + refer) | ≥ 85% |
| Flow coverage / order | ≥ 0.8 / ≥ 0.8 |
| Ambiguity handled | ≥ 90% |
| Time to first token p50 | < 1.5 s |
