# Desktop Voice Agent Rollout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the approved desktop voice agent, unified Flow/Skill/MCP capability runtime, Mission Control, supervised self-improvement, and macOS safety/performance acceptance in independently shippable milestones.

**Architecture:** Build the durable capability and event foundation first, then the bounded Agent Harness, the event-projected desktop UI, the macOS voice edge, and finally supervised improvement plus adversarial verification. Every milestone leaves the application working and testable; later milestones depend only on stable public types from earlier ones.

**Tech Stack:** Rust 2021, Tokio, Tauri 2, rusqlite, serde/schemars, vanilla ESM/HTML/CSS, MCP stdio/Streamable HTTP, cpal, sherpa-onnx, AVSpeechSynthesizer.

---

## Plan suite and dependency order

1. `2026-07-11-capability-mcp-foundation.md`
   - Unified capability descriptors, persistent profiles/events, generic MCP import, backend commands, and a usable Capability Hub.
2. `2026-07-11-agent-harness-runtime.md`
   - Risk policy, immutable plans, bounded Plan–Act–Observe–Validate–Reflect loop, Flow/Skill/MCP adapters, cancellation and recovery.
3. `2026-07-11-mission-control-ui.md`
   - Event projection, execution DAG, live detail panel, confirmations, interruption controls, adaptive feedback surfaces.
4. `2026-07-11-voice-edge.md`
   - macOS permissions, global shortcut, audio capture, local wake word, hybrid STT, system TTS, floating capsule.
5. `2026-07-11-self-improvement-hardening.md`
   - Trace mining, structured proposals, sandbox evaluation, human approval/rollback, prompt-injection defenses, performance and release gates.

## Cross-plan rules

- Execute the plans in order; do not add compatibility shims for types that are not yet merged.
- Use TDD for every behavior change: red test, focused implementation, green test, commit.
- Preserve the existing vanilla frontend; do not introduce React or a bundler.
- Keep Flow VM as the only Flow/Skill execution engine.
- Persist append-only agent events before emitting them to Tauri listeners.
- Never store raw audio by default or plaintext MCP/STT credentials.
- Never let model output directly approve permissions or improvement proposals.
- Before each plan, use `superpowers:using-git-worktrees` because the current checkout contains unrelated user changes.

## Milestone acceptance gates

- Milestone 1: import a mixed MCP config, connect to a fixture server, browse tools, and persist redacted profiles.
- Milestone 2: execute a deterministic multi-node plan across Flow, Skill and MCP with risk gates, cancellation and persisted events.
- Milestone 3: reconstruct and animate serial/parallel execution from events without reading executor internals.
- Milestone 4: wake by shortcut or local keyword, stream transcript, route a command and speak a short result on macOS.
- Milestone 5: produce an evaluated improvement proposal that cannot take effect until explicit approval, then pass security/performance gates.

