# Supervised Self-Improvement and Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add evaluated, human-approved improvement proposals and close the security, recovery, observability and release gates for the full voice agent platform.

**Architecture:** Mine only completed redacted traces, produce structured allowlisted diffs, evaluate them in an isolated replay harness, and require a durable approval record before versioned application. Treat all external/model content as untrusted data.

**Tech Stack:** Rust 2021, serde_json, lumo-agent, lumo-storage, lumo-ai, Tauri 2, Node tests.

---

### Task 1: Define structured improvement proposals

**Files:**
- Create: `crates/lumo-agent/src/improvement.rs`
- Create: `crates/lumo-agent/tests/improvement.rs`

- [ ] Test allowed proposal targets (`alias`, `router_example`, `prompt_template`, `flow_patch`, `skill_patch`) and hard rejection of Vault, approval, risk-floor and system-policy targets.
- [ ] Implement:

```rust
pub struct ImprovementProposal {
    pub id: String,
    pub source_run_ids: Vec<String>,
    pub target: ImprovementTarget,
    pub patch: Value,
    pub rationale: String,
    pub status: ProposalStatus,
    pub base_version_hash: String,
}
```

- [ ] Run tests and commit `feat(agent): define supervised improvement proposals`.

### Task 2: Mine redacted traces and build proposals

**Files:**
- Create: `crates/lumo-agent/src/trace_miner.rs`
- Create: `crates/lumo-agent/src/proposal_builder.rs`
- Create: `crates/lumo-agent/tests/trace_miner.rs`

- [ ] Test aggregation of repeated retries, manual corrections, replacement tools, latency and success rate without retaining secret-bearing payload fields.
- [ ] Implement deterministic candidates first; pass only redacted summaries to the model proposal builder and require JSON Schema validation.
- [ ] Run tests and commit `feat(agent): derive proposals from redacted traces`.

### Task 3: Add replay evaluation and approval-gated apply/rollback

**Files:**
- Create: `crates/lumo-agent/src/evaluation.rs`
- Create: `crates/lumo-agent/src/proposal_apply.rs`
- Create: `crates/lumo-agent/tests/evaluation.rs`
- Create: `crates/lumo-agent/tests/proposal_apply.rs`

- [ ] Test quality, success, latency, cost and permission deltas; fail evaluation on any risk expansion not declared by the proposal.
- [ ] Test that apply requires an approval row matching proposal ID, patch hash, base version and approver; stale base versions fail.
- [ ] Apply into a new version, never in-place; store rollback metadata and prove rollback restores the previous active version.
- [ ] Run tests and commit `feat(agent): evaluate approve and rollback improvements`.

### Task 4: Add improvement review UI

**Files:**
- Create: `apps/desktop/src-tauri/src/improvement_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Create: `apps/desktop/frontend/src/js/improvements.js`
- Create: `apps/desktop/frontend/src/styles/improvements.css`
- Create: `apps/desktop/frontend/test/improvements.test.js`

- [ ] Test diff rendering, metric comparison, approve/reject, stale proposal, rollback and secret redaction.
- [ ] Add commands `list_improvement_proposals`, `evaluate_improvement`, `approve_improvement`, `reject_improvement`, `rollback_improvement`.
- [ ] Add Capability Hub “改进提案” view with source traces, structured diff, evaluation matrix and immutable approval summary.
- [ ] Run desktop/frontend tests and commit `feat(desktop): review supervised improvement proposals`.

### Task 5: Harden prompt, tool and data trust boundaries

**Files:**
- Create: `crates/lumo-agent/src/trust.rs`
- Create: `crates/lumo-agent/tests/adversarial.rs`
- Create: `crates/lumo-agent/tests/fixtures/adversarial_cases.json`

- [ ] Add cases for MCP tool-description injection, tool-result injection, webpage/email instruction injection, secret exfiltration, risk downgrade, hidden tool expansion and approval forgery.
- [ ] Tag content origins and ensure untrusted text is quoted as data; only code-owned policy can change visible tools, budgets, approvals or system instructions.
- [ ] Run `cargo test -p lumo-agent --test adversarial` and commit `security(agent): enforce untrusted data boundaries`.

### Task 6: Verify crash recovery and duplicate-side-effect safety

**Files:**
- Create: `crates/lumo-agent/tests/recovery.rs`
- Modify: `crates/lumo-agent/src/loop_engine.rs`
- Modify: `apps/desktop/src-tauri/src/agent_commands.rs`

- [ ] Simulate crashes before call, during call, after remote success/before event persistence and after event persistence.
- [ ] Prove read-only idempotent nodes may resume, while uncertain L2/L3 nodes become `unknown` and require user resolution.
- [ ] Run recovery tests and commit `fix(agent): prevent duplicate uncertain side effects`.

### Task 7: Full verification and documentation

**Files:**
- Modify: `README.md`
- Modify: `apps/desktop/README.md`
- Modify: `docs/03-Subsystems-Deep-Dive.md`
- Modify: `docs/05-LumoFlow-Instruction-Set.md`

- [ ] Document voice privacy, supported MCP import formats, risk levels, Agent budgets, event protocol, recovery semantics and supervised improvement.
- [ ] Run core packages separately from desktop to avoid unintended feature unification:

```bash
cargo test -p lumo-storage -p lumo-core -p lumo-actions -p lumo-skills -p lumo-agent -p lumo-voice
cargo test -p lumorpa-desktop
cd apps/desktop/frontend && npm test && npm run lint
cargo clippy -p lumo-agent -p lumo-voice -p lumorpa-desktop --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] Run the adversarial suite, macOS voice acceptance, MCP stdio/HTTP fixtures, Mission Control visual checks and improvement approval/rollback scenario.
- [ ] Confirm every L2/L3 call has an approval event, raw audio is absent by default, secrets are redacted, budgets terminate loops, and unapproved proposals have no runtime effect.
- [ ] Commit `docs: document desktop voice agent platform`.
