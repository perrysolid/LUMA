# LUMA

A visual AI companion for macOS and Windows. Hold a shortcut, point at anything
and ask, for example "what is this?". LUMA answers out loud while drawing
boxes, arrows, labels and numbered steps directly over your screen, timed to
what it is saying.

- **Sees** the display you are working on, plus a sharp close-up around your pointer
- **Talks** naturally, and you can interrupt it any time by pressing the shortcut again
- **Points** with boxes, circles, highlights, arrows, step badges, spotlight and a flying pointer
- **Remembers** the conversation ("explain the one you just showed me", "go deeper")
- **Private by design:** captures only while you hold the shortcut, keys live in your
  OS keychain, password managers are excluded, and nothing is saved to disk

Built with Tauri 2 (Rust + TypeScript), Gemini, AssemblyAI and Sarvam.
See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), [docs/ROADMAP.md](docs/ROADMAP.md)
and [docs/RESEARCH.md](docs/RESEARCH.md).

## Use it

1. Install LUMA (`.dmg` on macOS, `.msi`/`.exe` on Windows) from Releases, or build it yourself (below).
2. Open LUMA from the menu bar / system tray → **Settings** → add your keys:
   - **Gemini** (required): https://aistudio.google.com/apikey. Use a billing-enabled
     project so your screen content is not used for training.
   - **AssemblyAI** (required, speech-to-text)
   - **Sarvam** (optional, voice). Without it, answers appear as captions.
3. Hold **⌘⇧Space** (macOS) or **Ctrl+Alt+Space** (Windows), ask, release.

macOS asks once for Microphone and Screen Recording permission.

## Develop

Requirements: Node 20+, Rust (stable), and on macOS the Xcode Command Line Tools
(`xcode-select --install`). The full Xcode app is not needed.

```bash
npm install
npm run tauri dev        # run the app
npm test                 # TypeScript + Rust unit tests
npm run demo             # overlay demo page: http://localhost:1420/dev/demo.html
```

### Eval

```bash
npm run eval:render                              # render fixtures with headless Chrome
LUMA_GEMINI_API_KEY=... npm run eval             # score grounding, flows, ambiguity, latency
LUMA_GEMINI_API_KEY=... npm run eval -- --filter editor --repeat 3
```

### Release builds

Push a tag such as `v0.1.0`. GitHub Actions builds macOS (Apple Silicon and
Intel) and Windows installers into a draft release
(`.github/workflows/release.yml`). Unsigned builds work, but show a first-launch
warning; see the roadmap for signing.
