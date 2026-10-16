# LumoRPA Desktop

LumoRPA Desktop is a Tauri-based desktop workbench that calls the Rust runtime directly.

## Desktop Voice Agent

The desktop control plane combines four surfaces:

- A privacy-first voice edge with global shortcut/local wake support, hybrid STT, system TTS, cancellation, and a transparent floating capsule.
- A unified capability catalog for Flow, versioned Skill, and MCP tools. Imported MCP JSON, JSONC, YAML, and TOML configurations are previewed with secrets redacted; secret values are stored in the encrypted Vault and only `VaultRef` values are persisted.
- A bounded Agent Harness using validated DAG plans, risk levels L0–L3, immutable approval snapshots, cancellation, timeouts, retry limits, tool/token budgets, and append-before-broadcast events.
- Mission Control, which reconstructs serial and parallel execution exclusively from persisted/live events and shows current nodes, retries, replans, approvals, controls, and redacted diagnostics.

Raw microphone audio is not retained by default. The local pre-roll buffer is bounded and cleared on cancel/mute. Cloud STT is permitted only by the selected profile and receives post-wake audio. L2 external effects require confirmation; L3 operations use strengthened confirmation and cannot be approved by model output.

### Summoning and moving the voice capsule

The capsule is a separate borderless always-on-top window, so it can be summoned three ways and never becomes unreachable after it is dismissed:

- The global shortcut (`CommandOrControl+Shift+Space` by default) toggles it from any application.
- The `语音` button in the workbench titlebar toggles it and permanently displays the currently bound shortcut.
- The tray menu entry `唤起 / 收起 Lumo 语音` works even while the main window is closed.

Dragging anywhere on the capsule except its two buttons moves it; the position is remembered in `capsule-window.json` under the app data directory and is restored on the next summon. A remembered position that no longer overlaps any connected display falls back to bottom-center of the active screen. The capsule's `×` stops listening and hides the window — a toast then repeats the shortcut. Bindings are edited in 设置 → 桌面语音与隐私, whose first card lists all three summon paths.

Supervised self-improvement is versioned and approval-gated: redacted completed traces may produce structured proposals, but proposals must pass replay evaluation and explicit human approval before a new version becomes active. Applied versions retain rollback metadata.

### MCP command surface

The desktop host exposes commands for preview/apply import, list/test/delete/enable servers, discover/enable tools, and call tools. Supported runtime transports are stdio and MCP Streamable HTTP; legacy SSE profiles are reported as unsupported instead of being silently reinterpreted.

### Frontend verification

```bash
cd apps/desktop/frontend
npm test
npm run lint
```

### Production release gates

Tagged releases are blocked until Agent/Voice/Desktop tests, strict Clippy, Mission Control tests, Sherpa native-boundary tests and frontend lint all pass. macOS tags additionally require certificate, signing identity and notarization credentials. Voice model manifests are versioned resources: the native runtime checksum and every model asset checksum must match before a local wake/STT session is created.

Long-running agent jobs use durable leases and heartbeats. After a crash, idempotent work can resume; uncertain L2/L3 external effects become `unknown` and require an operator decision. Self-improvement candidates run in shadow mode without duplicating external effects and cannot activate until sample, regression and permission-expansion gates pass.

## Local Build

```bash
scripts/build-desktop.sh "" app,dmg
```

macOS outputs are written under the workspace `target/release/bundle/`.

Prefer the script over a bare `cargo tauri build`: `createUpdaterArtifacts` is
enabled but the updater pubkey lives in `LUMO_UPDATER_PUBKEY` rather than
`tauri.conf.json`, and the script feeds it to the bundler. Export it to produce
signed `.tar.gz`/`.sig` updater artifacts (alongside
`TAURI_SIGNING_PRIVATE_KEY`); leave it unset for a plain local installer.

### Universal (Intel + Apple Silicon)

Tauri builds a single universal `.app`/`.dmg` natively from the universal target
(it `lipo`-merges both arches internally — no manual merge step):

```bash
# from the repo root
rustup target add x86_64-apple-darwin aarch64-apple-darwin
scripts/build-desktop.sh universal-apple-darwin app,dmg
```

Output: `target/universal-apple-darwin/release/bundle/dmg/LumoRPA_<version>_universal.dmg`.
This is what the `release` CI workflow ships for macOS, so one download runs on
both Intel and Apple Silicon Macs.

## Windows

Build on a Windows runner with MSVC, WebView2 and the NSIS/MSI toolchain installed:

```powershell
cd apps\desktop
cargo tauri build --target x86_64-pc-windows-msvc --bundles nsis,msi
```

Expected outputs:

- `*.exe` NSIS installer
- `*.msi` MSI installer

## Linux

Build on the target Linux distribution:

```bash
cd apps/desktop
cargo tauri build --target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage
```

Expected outputs:

- `*.deb`
- `*.rpm`
- `*.AppImage`

## Kylin / Xinchuang Linux

For Kylin and other Xinchuang Linux distributions, build on the matching CPU and OS image whenever possible:

```bash
cd apps/desktop
cargo tauri build --target x86_64-unknown-linux-gnu --bundles deb,rpm,appimage
cargo tauri build --target aarch64-unknown-linux-gnu --bundles deb,rpm,appimage
cargo tauri build --target loongarch64-unknown-linux-gnu --bundles deb,rpm
```

AppImage support depends on the target distribution and CPU architecture. For loongarch64, prefer native `.deb` or `.rpm` builds on a loongarch64 Kylin builder.
