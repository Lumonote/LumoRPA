# Agent Harness Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Execute bounded, auditable plans across Flow, Skill and MCP through a unified Harness with immutable permissions, cancellation, retries, validation and recovery.

**Architecture:** Model plans and events as serializable data. Route exact aliases deterministically before using an AI planner. The executor only runs validated ready nodes through adapters and appends every state transition to storage before broadcasting it.

**Tech Stack:** Rust 2021, Tokio, lumo-core FlowVm, lumo-skills, lumo-actions MCP client, lumo-storage, lumo-ai.

---

### Task 1: Define plan, event and budget contracts

**Files:**
- Create: `crates/lumo-agent/src/plan.rs`
- Create: `crates/lumo-agent/src/event.rs`
- Create: `crates/lumo-agent/src/budget.rs`
- Create: `crates/lumo-agent/tests/plan_contract.rs`

- [ ] Write failing tests for cycle rejection, monotonic event sequence and hard budget exhaustion.
- [ ] Run `cargo test -p lumo-agent --test plan_contract` and confirm missing types fail.
- [ ] Implement `AgentPlan`, `PlanNode`, `AgentEvent`, `AgentBudget` and `validate_dag()` with this node contract:

```rust
pub struct PlanNode {
    pub id: String,
    pub depends_on: Vec<String>,
    pub capability_id: String,
    pub arguments: Value,
    pub risk: RiskLevel,
    pub timeout_ms: u64,
    pub retry_limit: u32,
    pub expected_output_schema: Option<Value>,
}
```

- [ ] Run the test and commit with `git commit -m "feat(agent): define plan event and budget contracts"`.

### Task 2: Implement risk policy and immutable approval snapshots

**Files:**
- Create: `crates/lumo-agent/src/policy.rs`
- Create: `crates/lumo-agent/tests/policy.rs`

- [ ] Write table-driven tests proving L0/L1 policy behavior, mandatory L2 confirmation, strengthened L3 confirmation, and rejection when a replacement tool lowers the declared risk.
- [ ] Implement:

```rust
pub struct ApprovalSnapshot {
    pub plan_hash: String,
    pub capability_versions: BTreeMap<String, String>,
    pub schema_hashes: BTreeMap<String, String>,
    pub approved_nodes: BTreeSet<String>,
    pub approved_at: DateTime<Utc>,
}

pub fn evaluate_node(node: &PlanNode, capability: &CapabilityDescriptor, profile: &AgentProfile) -> PolicyDecision;
pub fn validate_replan(old: &AgentPlan, new: &AgentPlan, approval: &ApprovalSnapshot) -> ReplanDecision;
```

- [ ] Run `cargo test -p lumo-agent --test policy` and commit `feat(agent): enforce risk and approval snapshots`.

### Task 3: Add append-before-broadcast event sink

**Files:**
- Create: `crates/lumo-agent/src/event_sink.rs`
- Create: `crates/lumo-agent/tests/event_sink.rs`

- [ ] Write a test with a failing repository proving no broadcast occurs when persistence fails.
- [ ] Implement `EventSink::publish` so it obtains the next sequence under a per-run lock, writes through `Repo::append_agent_event`, then sends over `tokio::sync::broadcast`.
- [ ] Run `cargo test -p lumo-agent --test event_sink` and commit `feat(agent): persist events before broadcast`.

### Task 4: Implement Flow, Skill and MCP invocation adapters

**Files:**
- Create: `crates/lumo-agent/src/adapters/mod.rs`
- Create: `crates/lumo-agent/src/adapters/flow.rs`
- Create: `crates/lumo-agent/src/adapters/skill.rs`
- Create: `crates/lumo-agent/src/adapters/mcp.rs`
- Create: `crates/lumo-agent/tests/adapters.rs`

- [ ] Write fixture tests that invoke one Flow, one Skill and one MCP echo tool and normalize all results to `InvocationResult`.
- [ ] Implement:

```rust
#[async_trait]
pub trait InvocationAdapter: Send + Sync {
    fn source_kind(&self) -> CapabilityKind;
    async fn invoke(&self, request: InvocationRequest, ctx: InvocationContext) -> Result<InvocationResult, InvocationError>;
}
```

- [ ] Ensure Flow and Skill reuse `FlowVm`, capability clamping and shared cancellation; MCP uses the reusable client and a profile with Vault-resolved secrets.
- [ ] Run `cargo test -p lumo-agent --test adapters` and commit `feat(agent): invoke flow skill and MCP capabilities`.

### Task 5: Build the bounded Agent Loop

**Files:**
- Create: `crates/lumo-agent/src/harness.rs`
- Create: `crates/lumo-agent/src/loop_engine.rs`
- Create: `crates/lumo-agent/src/validator.rs`
- Create: `crates/lumo-agent/tests/loop_engine.rs`

- [ ] Write tests for serial execution, four-way bounded parallelism, retry, cancellation, timeout, budget exhaustion and deterministic output ordering.
- [ ] Implement the finite transition enum:

```rust
pub enum LoopDecision {
    Complete,
    Retry { node_id: String },
    Replace { node_id: String, capability_id: String },
    Replan { reason: String },
    AskUser { question: String },
    Fail { reason: String },
}
```

- [ ] Use `JoinSet` plus a semaphore for ready nodes; emit queued/started/progress/completed/failed events; check cancellation and budget before every transition.
- [ ] Run `cargo test -p lumo-agent --test loop_engine` and commit `feat(agent): add bounded plan act observe loop`.

### Task 6: Add deterministic routing and AI planner boundary

**Files:**
- Create: `crates/lumo-agent/src/router.rs`
- Create: `crates/lumo-agent/src/planner.rs`
- Create: `crates/lumo-agent/tests/router.rs`
- Create: `crates/lumo-agent/tests/planner.rs`

- [ ] Test system control intents, exact aliases, ambiguous aliases, filtered AI candidates and malformed model plans.
- [ ] Implement `Router::route()` order: control intent → exact alias → local ranked candidates → `Planner::plan()` → one clarification.
- [ ] Require the AI planner to return JSON matching `AgentPlan`; run `validate_dag`, catalog membership, Schema validation and policy evaluation before acceptance.
- [ ] Run router/planner tests and commit `feat(agent): route commands into validated plans`.

### Task 7: Add desktop session commands and recovery

**Files:**
- Create: `apps/desktop/src-tauri/src/agent_commands.rs`
- Modify: `apps/desktop/src-tauri/src/state.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Create: `apps/desktop/src-tauri/tests/agent_commands.rs`

- [ ] Add tests for start, pause, resume, cancel, approve, list events and restore running/unknown sessions.
- [ ] Implement commands `agent_start`, `agent_pause`, `agent_resume`, `agent_cancel`, `agent_approve`, `agent_events`, `agent_restore`.
- [ ] Mark unfinished external-effect nodes `unknown` after restart and require user resolution before continuing.
- [ ] Run `cargo test -p lumorpa-desktop agent` and commit `feat(desktop): host agent sessions and recovery`.

