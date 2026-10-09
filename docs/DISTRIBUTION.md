# Distributing LUMA

Push a tag such as `v0.2.0` and `.github/workflows/release.yml` builds draft
releases for macOS (Apple silicon and Intel) and Windows. With no secrets set
the installers are unsigned, which is fine for testing. Each of the sections
below switches on one more piece by adding GitHub repository secrets
(Settings → Secrets and variables → Actions).

## Auto-update

LUMA checks `https://github.com/perrysolid/LUMA/releases/latest/download/latest.json`
at launch and every 6 hours. When there is a newer signed build, the tray menu
offers **Install LUMA x.y.z and restart**. Nothing installs silently.

The update signing key was generated on the development Mac:

- private key: `~/.tauri/luma-updater.key` (no password). Back it up; if it is
  lost, existing installs cannot verify new updates
- public key: already in `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`

Secrets:

| Secret | Value |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | the contents of `~/.tauri/luma-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | empty (or the password, if you regenerate the key with one) |

With the key set, the workflow turns on `createUpdaterArtifacts` and uploads
`latest.json`. Publish the draft release for clients to see it.

## macOS: Developer ID signing and notarization

Needs an Apple Developer Program membership.

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | base64 of your "Developer ID Application" `.p12` (`base64 -i cert.p12 \| pbcopy`) |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | your Apple ID email |
| `APPLE_PASSWORD` | an app-specific password (appleid.apple.com) |
| `APPLE_TEAM_ID` | your 10-character team id |

The app is built with the hardened runtime and `src-tauri/Entitlements.plist`
(microphone input; without it a notarized build records silence).
`src-tauri/Info.plist` carries the Microphone and Speech Recognition usage strings.

## Windows: Azure Trusted Signing

| Secret | Value |
|---|---|
| `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID` | an app registration with the "Trusted Signing Certificate Profile Signer" role |
| `TRUSTED_SIGNING_ENDPOINT` | e.g. `https://eus.codesigning.azure.net/` |
| `TRUSTED_SIGNING_ACCOUNT` | the Trusted Signing account name |
| `TRUSTED_SIGNING_PROFILE` | the certificate profile name |

When `AZURE_CLIENT_ID` is set, the workflow installs `trusted-signing-cli` and
signs the installers.

## Key proxy (optional)

For teams that don't want every user to bring their own API keys. The keys
stay on a server you run; each device gets its own token.

```bash
GEMINI_API_KEY=… SARVAM_API_KEY=… ASSEMBLYAI_API_KEY=… \
LUMA_PROXY_TOKENS=device-token-for-alice-xxxxxxxx,device-token-for-bob-yyyyyyyy \
LUMA_PROXY_RPM=60 PORT=8787 \
cargo run -p luma-proxy --release
```

Run it behind HTTPS (any reverse proxy or a platform that terminates TLS). In
LUMA → Settings, set **LUMA proxy URL** and paste the device's token into
**LUMA proxy token**. Provider keys then show "✓ via proxy".

What it allows, and nothing else:

| Route | Does |
|---|---|
| `POST /gemini/v1beta/models/{model}:streamGenerateContent?alt=sse`, `:generateContent` | adds the Gemini key, streams the answer back |
| `POST /sarvam/text-to-speech` | adds the Sarvam key |
| `GET /assemblyai/token` | returns a 60-second AssemblyAI streaming token; the device connects to AssemblyAI directly |
| `GET /health` | `ok` |

Every request needs `Authorization: Bearer <device token>` (tokens are compared in
constant time), and each token is limited to `LUMA_PROXY_RPM` requests a
minute. Through the proxy, voice uses Sarvam REST instead of the streaming
socket, and fast mode (Gemini Live) is off, because neither is proxied.

Verified end to end: `SMOKE_PROXY_URL=… LUMA_PROXY_TOKEN=… npm run smoke` runs
the full Gemini → Sarvam → AssemblyAI loop with only the device token on the client.
