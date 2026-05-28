# LumoRPA

LumoRPA is an early-stage, local-first RPA runtime built around flow-as-code YAML, a Rust execution core, deterministic actions, and optional BYOK AI routing.

## Current Scope

- Cargo workspace with DSL, VM, storage, actions, AI router, skills, recorder skeleton, and CLI crates.
- LumoFlow YAML parsing, templating, structural validation, and CLI validation.
- Built-in actions for control flow, file, HTTP, Excel, browser, AI chat, and skill invocation, with action schemas exposed through `lumo actions --show`.
- SQLite-backed run history for local execution, including nested paths for control-flow and loop iterations.
- Claude-style `SKILL.md` loading and `skill.invoke` sub-flows.

This repository is still in the M1 stage. Studio UI, recorder implementation, scheduler, MCP server, and multi-worker orchestration are planned but not production-ready yet.

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
examples/             Runnable flow examples
docs/                 Product and architecture design notes
```

## Development Priorities

1. Keep `cargo fmt`, `cargo clippy -D warnings`, and workspace tests green.
2. Expand action-specific JSON schemas for richer Studio form generation.
3. Add encrypted vault storage and management commands.
4. Replace the recorder placeholder with a minimal browser CDP recorder.
5. Add scheduler/MCP entry points on top of the durable run store.
