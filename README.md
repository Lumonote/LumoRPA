# LumoRPA

LumoRPA is an early-stage, local-first RPA runtime built around flow-as-code YAML, a Rust execution core, deterministic actions, and optional BYOK AI routing.

## Current Scope

- Cargo workspace with DSL, VM, storage, actions, AI router, skills, recorder skeleton, CLI, and desktop crates.
- LumoFlow YAML parsing, templating, structural validation, and CLI validation.
- Built-in actions for control flow, file, HTTP, Excel, browser, AI chat, and skill invocation, with action schemas exposed through `lumo actions --show`.
- SQLite-backed run history for local execution, including nested paths for control-flow and loop iterations.
- Claude-style `SKILL.md` loading and `skill.invoke` sub-flows.
- Tauri desktop workbench for validating, running, inspecting, and packaging local automation flows.

This repository is still in the M1 stage. Recorder implementation, scheduler, MCP server, and multi-worker orchestration are planned but not production-ready yet.

## Requirements

- Rust toolchain from `rust-toolchain.toml`
- macOS, Linux, or Windows
- Chromium-compatible browser available on PATH for browser actions

## Quick Start

```bash
cargo build --workspace
cargo test --workspace --all-targets

cargo run -p lumo-cli -- validate examples/hello-world.lumoflow.yaml
cargo run -p lumo-cli -- run --no-store examples/hello-world.lumoflow.yaml
```

Run a flow that uses local example skills:

```bash
cargo run -p lumo-cli -- run --no-store examples/skill-driver.lumoflow.yaml
```

The CLI loads installed skills from `$LUMO_HOME/skills` and also loads a `skills/` directory next to the flow file when present.

## Packaging the CLI

Build a standalone `lumo` release archive under `dist/`:

```bash
# Host platform
scripts/package-cli.sh

# Cross-compile (run `rustup target add <triple>` first)
scripts/package-cli.sh x86_64-unknown-linux-gnu
scripts/package-cli.sh aarch64-unknown-linux-gnu
scripts/package-cli.sh x86_64-pc-windows-msvc

# Universal macOS archive — Intel + Apple Silicon in one binary (macOS only;
# needs both apple-darwin targets installed). The script builds each arch and
# `lipo`-merges them into a single fat `bin/lumo`.
scripts/package-cli.sh universal-apple-darwin
```

This produces `dist/lumorpa-<version>-<os>-<arch>.tar.gz` (a `.zip` for Windows
targets) containing `bin/lumo`, the bundled `examples/`, `README.md`, and `LICENSE`.

## Desktop App

Run or package the Tauri desktop workbench:

```bash
cd apps/desktop
cargo tauri dev
cargo tauri build --bundles app,dmg
```

Platform package commands:

```bash
# macOS
scripts/build-desktop.sh

# Windows, on a Windows builder
scripts/build-desktop.sh x86_64-pc-windows-msvc nsis,msi

# Linux / Kylin x86_64, on the target Linux builder
scripts/build-desktop.sh x86_64-unknown-linux-gnu deb,rpm,appimage

# Kylin ARM64, on an aarch64 Linux builder
scripts/build-desktop.sh aarch64-unknown-linux-gnu deb,rpm,appimage

# Kylin LoongArch, on a loongarch64 Linux builder
scripts/build-desktop.sh loongarch64-unknown-linux-gnu deb,rpm
```

Desktop details live in `apps/desktop/README.md`.

## Release (CI 出包)

Desktop installers for Windows, Linux, and 信创 (麒麟/龙芯) platforms cannot be
cross-built from macOS — each Tauri bundler needs its target OS's native
WebKit/GTK. Release packages are therefore produced by the
[`release`](.github/workflows/release.yml) GitHub Actions workflow, which builds
the **desktop installer + CLI archive** for every platform on its own runner.

Trigger it by pushing a `v*` tag (or via *Actions → release → Run workflow*):

```bash
git tag v0.1.0
git push origin v0.1.0
```

| Platform | Runner | Desktop | CLI archive |
| --- | --- | --- | --- |
| macOS universal (Intel + ARM) | `macos-14` | `.dmg` | `…-darwin-universal.tar.gz` |
| Windows x86_64 | `windows-latest` | `.exe` (NSIS) + `.msi` | `…-windows-x86_64.zip` |
| Linux x86_64 | `ubuntu-22.04` | `.deb` / `.rpm` / `.AppImage` | `…-linux-x86_64.tar.gz` |
| Linux aarch64 (麒麟 ARM) | `ubuntu-22.04-arm` | `.deb` / `.rpm` | `…-linux-aarch64.tar.gz` |
| loongarch64 (龙芯) | `ubuntu-22.04` (best-effort) | — | `…-linux-loongarch64.tar.gz` |

On a tag, every job's artifacts are also attached to the GitHub Release.

**信创 (Kylin / LoongArch) compatibility.** The Linux jobs build on Ubuntu, whose
glibc may be newer than a given 麒麟/统信 release, so an Ubuntu-built binary can
fail to load on an older 信创 target. For production 信创 packages, build natively
on a matching 麒麟/龙芯 image (or a self-hosted runner). GitHub has no
loongarch64 runner, so its CLI is cross-compiled best-effort and the desktop
bundle must be built natively. Treat the Ubuntu artifacts as a convenience
baseline, not a certified 信创 deliverable.

## AI Providers

AI actions are opt-in. Non-AI flows do not require provider configuration.

```bash
cargo run -p lumo-cli -- providers init
cargo run -p lumo-cli -- providers list
```

Network calls are disabled by default. Set `LUMO_ALLOW_LLM_NETWORK=1` before running flows that use `ai.chat`.

## Useful Commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p lumo-cli -- actions
cargo run -p lumo-cli -- actions --show ai.chat
cargo run -p lumo-cli -- skills list
cargo run -p lumo-cli -- runs list
cargo run -p lumo-cli -- runs show <run_id> --outputs
```

## Runtime Safety

Flows are capability-checked at runtime. Actions that touch files, HTTP/browser network, or LLMs require matching `spec.capabilities` entries such as:

```yaml
spec:
  capabilities:
    fs.read: ["./examples/data/**"]
    fs.write: ["/tmp/**"]
    network: ["example.com"]
    llm: ["*"]
```

Vault placeholders are preserved during template rendering and resolved just before action execution from environment variables. A declaration such as `vault: [smtp]` can use `{{ vault.smtp.password }}` with `LUMO_VAULT_SMTP_PASSWORD` set in the environment.

## Layout

```text
crates/lumo-dsl       LumoFlow AST, parser, template renderer, validation
crates/lumo-core      Action trait, registry, StepCtx, Flow VM
crates/lumo-actions   Built-in deterministic actions
crates/lumo-ai        Provider config, AI router, ai.chat action
crates/lumo-skills    SKILL.md loader and skill.invoke action
crates/lumo-storage   SQLite schema and repository
crates/lumo-recorder  Recorder trait and M2 placeholder
crates/lumo-cli       lumo command-line interface
apps/desktop          Tauri desktop workbench and package config
examples/             Runnable flow examples
docs/                 Product and architecture design notes
```

## Development Priorities

1. Keep `cargo fmt`, `cargo clippy -D warnings`, and workspace tests green.
2. Expand action-specific JSON schemas for richer Studio form generation.
3. Add encrypted vault storage and management commands.
4. Replace the recorder placeholder with a minimal browser CDP recorder.
5. Add scheduler/MCP entry points on top of the durable run store.
