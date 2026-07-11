# Desktop Agent Production Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the existing desktop voice-agent foundation into an end-to-end executable, always-on, enterprise-governed and releasable desktop agent.

**Architecture:** A single `DesktopAgentService` owns routing, planning and Harness execution for typed and voice requests. Voice capture/providers remain in `lumo-voice`; desktop lifecycle and windows remain in Tauri. Durable policy, jobs, OAuth credentials, approvals, audit and improvement versions remain in SQLite/Vault and are projected into the vanilla frontend through append-before-broadcast events.

**Tech Stack:** Rust 2021, Tokio, Tauri 2, CPAL, sherpa-onnx backend boundary, AVSpeechSynthesizer, rusqlite, MCP stdio/Streamable HTTP/OAuth, vanilla ESM/CSS, Node test runner.

---

### Task 1: Build the end-to-end DesktopAgentService

**Files:**
- Create: `apps/desktop/src-tauri/src/agent_service.rs`
- Modify: `apps/desktop/src-tauri/src/agent_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Test: `apps/desktop/src-tauri/src/agent_service.rs`

- [ ] Write a failing test that submits `"运行日报"`, resolves an exact capability alias, executes a two-node plan through fake adapters and receives persisted `run.completed` after node events.
- [ ] Run `cargo test -p lumorpa-desktop --lib agent_service::tests` and confirm the service type is missing.
- [ ] Implement this boundary:

```rust
pub struct AgentStartInput {
    pub utterance: String,
    pub profile_id: Option<String>,
    pub supplied_plan: Option<AgentPlan>,
}

#[async_trait]
pub trait DesktopAgentFactory: Send + Sync {
    async fn catalog(&self, profile_id: Option<&str>) -> Result<CapabilityCatalog, String>;
    async fn profile(&self, profile_id: Option<&str>) -> Result<AgentProfile, String>;
    async fn adapters(&self, profile: &AgentProfile) -> Result<AdapterRegistry, String>;
    async fn plan(&self, utterance: &str, catalog: &CapabilityCatalog, profile: &AgentProfile) -> Result<AgentPlan, String>;
}
```

- [ ] Make `agent_start` call the service, spawn `AgentHarness::execute`, persist terminal state and emit the same events used by Mission Control.
- [ ] Subscribe `lumo://agent-start-request` to this service so voice and typed requests share the same route.
- [ ] Run the focused tests and `cargo clippy -p lumorpa-desktop --lib --tests -- -D warnings`.

### Task 2: Complete native wake-word and local STT loading

**Files:**
- Create: `crates/lumo-voice/src/sherpa_native.rs`
- Modify: `crates/lumo-voice/src/sherpa.rs`
- Modify: `crates/lumo-voice/Cargo.toml`
- Test: `crates/lumo-voice/tests/sherpa_native.rs`

- [ ] Write failing tests for model-manifest mapping, missing dynamic library, invalid model assets, wake hit, partial/final ASR events and cancellation.
- [ ] Run `cargo test -p lumo-voice --test sherpa_native` and confirm native constructors are missing.
- [ ] Add a feature-gated `SherpaNativeBackend` that loads the native runtime/model assets from app data and implements the existing wake/STT backend traits.
- [ ] Keep builds without the native runtime deterministic: constructors return `NativeUnavailable`, while missing assets return `ModelMissing`.
- [ ] Add runtime version and checksum compatibility checks before creating sessions.
- [ ] Run all `lumo-voice` tests and strict Clippy.

### Task 3: Add real cloud STT providers and privacy policy

**Files:**
- Create: `crates/lumo-voice/src/cloud_stt.rs`
- Modify: `crates/lumo-voice/src/stt_router.rs`
- Modify: `apps/desktop/src-tauri/src/voice_commands.rs`
- Test: `crates/lumo-voice/tests/cloud_stt.rs`

- [ ] Write failing tests for OpenAI-compatible streaming requests, Vault credential resolution, post-wake-only frames, timeout, cancellation, cost budget and enterprise cloud denial.
- [ ] Implement `CloudSttProvider` behind an injected transport so tests never use the network.
- [ ] Add `VoicePrivacyPolicy { cloud_allowed, retain_transcript, retain_audio, max_cloud_seconds, max_cost_usd_micro }` and enforce it before sending frames.
- [ ] Wire the selected voice profile into `DefaultVoicePipelineFactory` instead of hard-coding `cloud_allowed: false`.
- [ ] Run focused tests and strict Clippy.

### Task 4: Implement always-on voice lifecycle and device hot-plug

**Files:**
- Create: `apps/desktop/src-tauri/src/voice_daemon.rs`
- Modify: `apps/desktop/src-tauri/src/voice_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/tauri.conf.json`
- Test: `apps/desktop/src-tauri/src/voice_daemon.rs`

- [ ] Write failing tests for login start, sleep/lock suspension, wake resume, device removal, default-device change, hard mute and duplicate daemon start.
- [ ] Implement `VoiceDaemon` with one cancellation root and explicit `Running/Suspended/Muted/Stopped` states.
- [ ] Restart capture only after a debounced device change and never while the screen is locked.
- [ ] Add tray/menu commands for wake enablement, mute, device and quit.
- [ ] Run desktop tests and strict Clippy.

### Task 5: Add durable jobs, scheduling and long-task recovery

**Files:**
- Create: `crates/lumo-agent/src/job.rs`
- Create: `crates/lumo-storage/src/job_store.rs`
- Create: `apps/desktop/src-tauri/src/job_commands.rs`
- Test: `crates/lumo-agent/tests/job.rs`
- Test: `crates/lumo-storage/tests/job_store.rs`

- [ ] Write failing tests for enqueue, cron/one-shot schedule, lease acquisition, heartbeat, retry time, cancellation and crash recovery.
- [ ] Implement `AgentJob` with `queued/running/waiting/paused/completed/failed/unknown` states and an idempotency key.
- [ ] Add transactional lease/heartbeat methods so only one desktop worker owns a job.
- [ ] Resume idempotent nodes; mark uncertain L2/L3 side effects `unknown` and require user resolution.
- [ ] Add Tauri commands `job_list`, `job_schedule`, `job_pause`, `job_resume`, `job_cancel`.
- [ ] Run focused tests and strict Clippy.

### Task 6: Add MCP OAuth, registry and health supervision

**Files:**
- Create: `crates/lumo-actions/src/mcp/oauth.rs`
- Create: `crates/lumo-agent/src/mcp_registry.rs`
- Create: `apps/desktop/src-tauri/src/mcp_supervisor.rs`
- Modify: `apps/desktop/src-tauri/src/mcp_commands.rs`
- Test: `crates/lumo-actions/tests/mcp_oauth.rs`

- [ ] Write failing tests for PKCE state validation, token refresh, Vault storage, registry signature metadata, schema-version drift, rate limiting and circuit breaking.
- [ ] Implement OAuth authorization metadata and token refresh through an injected browser/HTTP boundary.
- [ ] Store tokens only as Vault references; persist expiry and scopes separately.
- [ ] Add a health supervisor with exponential backoff, failure threshold and half-open recovery.
- [ ] Emit schema-change events that require operator approval before newly discovered tools become visible.
- [ ] Run MCP fixture tests and strict Clippy.

### Task 7: Build the Security Center and biometric L3 approval boundary

**Files:**
- Create: `crates/lumo-agent/src/security_center.rs`
- Create: `apps/desktop/src-tauri/src/security_commands.rs`
- Create: `apps/desktop/frontend/src/js/security-center.js`
- Create: `apps/desktop/frontend/src/styles/security-center.css`
- Test: `crates/lumo-agent/tests/security_center.rs`
- Test: `apps/desktop/frontend/test/security-center.test.js`

- [ ] Write failing tests for permission revoke, approval expiry, L3 biometric challenge, argument redaction, injection findings and audit export.
- [ ] Implement immutable approval/revocation records; model output cannot create either record.
- [ ] Add a platform authenticator trait; macOS production uses LocalAuthentication and tests use a fake authenticator.
- [ ] Render permission history, active grants, revoke controls, risk events and redacted audit export.
- [ ] Run Rust/frontend tests and linters.

### Task 8: Add Self-Improvement shadow/A-B lifecycle

**Files:**
- Create: `crates/lumo-agent/src/shadow_eval.rs`
- Modify: `crates/lumo-agent/src/proposal_apply.rs`
- Modify: `apps/desktop/frontend/src/js/improvements.js`
- Test: `crates/lumo-agent/tests/shadow_eval.rs`
- Test: `apps/desktop/frontend/test/improvements.test.js`

- [ ] Write failing tests for replay datasets, shadow-only execution, control/candidate comparison, regression thresholds, conflicting proposals, proposal expiry and automatic rollback trigger.
- [ ] Implement `ShadowEvaluation` that records candidate results without changing active routing or executing duplicate external effects.
- [ ] Require minimum sample count and zero undeclared permission expansion before approval becomes available.
- [ ] Add UI comparison charts and explicit rollback reason/history.
- [ ] Run focused tests and strict Clippy/lint.

### Task 9: Scale and visually verify Mission Control

**Files:**
- Modify: `apps/desktop/frontend/src/js/mission-control.js`
- Modify: `apps/desktop/frontend/src/styles/mission-control.css`
- Create: `apps/desktop/frontend/test/mission-control-scale.test.js`
- Create: `apps/desktop/frontend/test/fixtures/large-agent-events.json`

- [ ] Write failing tests for 1,000 nodes, capped logs, viewport virtualization, multi-monitor capsule placement, keyboard navigation and reduced motion.
- [ ] Virtualize offscreen topology lanes and logs while preserving active/failed/approval nodes.
- [ ] Add zoom, minimap, focus-active-node and accessible serial/parallel summaries.
- [ ] Verify 1280×720, 2560×1440 and reduced-motion layouts with fixture events.
- [ ] Run frontend tests and lint.

### Task 10: Add observability, packaging and release gates

**Files:**
- Create: `crates/lumo-agent/src/telemetry.rs`
- Create: `apps/desktop/src-tauri/src/update_commands.rs`
- Modify: `apps/desktop/README.md`
- Modify: `.github/workflows/release.yml`
- Test: `crates/lumo-agent/tests/telemetry.rs`

- [ ] Write failing tests proving telemetry redacts secrets/audio and can be disabled by policy.
- [ ] Add local structured diagnostics with run/event correlation and bounded retention.
- [ ] Add signed update metadata, rollback channel and model-resource version compatibility checks.
- [ ] Add release gates for macOS signing/notarization inputs, migrations, crash recovery, idle CPU, shortcut latency, MCP fixtures and security adversarial tests.
- [ ] Run:

```bash
cargo test -p lumo-storage -p lumo-actions -p lumo-agent -p lumo-voice -p lumorpa-desktop
cargo clippy -p lumo-agent -p lumo-voice -p lumorpa-desktop --all-targets -- -D warnings
cd apps/desktop/frontend && npm test && npm run lint
```

Expected: all commands exit 0 with no warnings or failed tests.
