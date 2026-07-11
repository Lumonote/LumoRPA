# Capability and MCP Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the unified capability catalog, durable agent/MCP storage, generic MCP import pipeline, desktop command surface, and first usable Capability Hub.

**Architecture:** Introduce `lumo-agent` with dependency-light descriptor/event/profile modules. Extend `lumo-storage` through focused modules and schema migration v4. Reuse the MCP transport in `lumo-actions`, while keeping import normalization and profile policy in `lumo-agent`.

**Tech Stack:** Rust 2021, serde/schemars, rusqlite, Tokio, reqwest/rustls, Tauri 2, vanilla ESM and Node test runner.

---

### Task 1: Scaffold `lumo-agent` and unified capability types

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/lumo-agent/Cargo.toml`
- Create: `crates/lumo-agent/src/lib.rs`
- Create: `crates/lumo-agent/src/capability.rs`
- Create: `crates/lumo-agent/tests/capability.rs`

- [ ] **Step 1: Write the failing serialization and stable-ID tests**

```rust
use lumo_agent::{CapabilityDescriptor, CapabilitySource, RiskLevel};

#[test]
fn mcp_capability_id_is_stable() {
    let c = CapabilityDescriptor::mcp("erp", "query_orders", serde_json::json!({"type":"object"}));
    assert_eq!(c.id, "mcp:erp/query_orders");
    assert_eq!(c.source, CapabilitySource::Mcp { server: "erp".into(), tool: "query_orders".into() });
    assert_eq!(c.risk, RiskLevel::L0);
    assert_eq!(serde_json::to_value(&c).unwrap()["versionHash"].as_str().unwrap().len(), 64);
}
```

- [ ] **Step 2: Run the focused test and verify the crate is missing**

Run: `cargo test -p lumo-agent --test capability`
Expected: FAIL because package `lumo-agent` does not exist.

- [ ] **Step 3: Add the workspace member and minimal public model**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CapabilitySource {
    Flow { path: String },
    Skill { name: String, source: String },
    Mcp { server: String, tool: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel { L0, L1, L2, L3 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub id: String,
    pub source: CapabilitySource,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: Option<serde_json::Value>,
    pub aliases: Vec<String>,
    pub examples: Vec<String>,
    pub risk: RiskLevel,
    pub enabled: bool,
    pub version_hash: String,
}
```

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p lumo-agent --test capability`
Expected: PASS.

```bash
git add Cargo.toml crates/lumo-agent
git commit -m "feat(agent): add unified capability descriptors"
```

### Task 2: Add storage migration v4 and repository APIs

**Files:**
- Modify: `crates/lumo-storage/src/repo.rs`
- Modify: `crates/lumo-storage/src/schema.rs`
- Modify: `crates/lumo-storage/src/types.rs`
- Modify: `crates/lumo-storage/src/lib.rs`
- Create: `crates/lumo-storage/src/agent_store.rs`
- Modify: `crates/lumo-storage/tests/migrations.rs`
- Create: `crates/lumo-storage/tests/agent_store.rs`

- [ ] **Step 1: Write migration and round-trip tests**

```rust
#[test]
fn v4_creates_agent_and_mcp_tables() {
    let repo = Repo::open_in_memory().unwrap();
    for table in ["voice_profiles", "mcp_servers", "mcp_tools", "capability_aliases",
                  "agent_profiles", "agent_runs", "agent_events",
                  "improvement_proposals", "improvement_approvals"] {
        let count: i64 = repo.with_raw(|c| c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table], |r| r.get(0))).unwrap();
        assert_eq!(count, 1, "missing {table}");
    }
}
```

- [ ] **Step 2: Verify the tests fail at schema version 3**

Run: `cargo test -p lumo-storage --test migrations --test agent_store`
Expected: FAIL with expected user version 4 and missing tables.

- [ ] **Step 3: Add guarded v3→v4 migration and focused repository methods**

```rust
pub struct AgentEventInsert<'a> {
    pub run_id: &'a str,
    pub seq: i64,
    pub kind: &'a str,
    pub node_id: Option<&'a str>,
    pub parent_node_id: Option<&'a str>,
    pub payload: &'a serde_json::Value,
}

impl Repo {
    pub fn append_agent_event(&self, row: AgentEventInsert<'_>) -> Result<(), StorageError>;
    pub fn list_agent_events(&self, run_id: &str, after_seq: i64) -> Result<Vec<AgentEventRow>, StorageError>;
    pub fn upsert_mcp_server(&self, profile: &McpServerRow) -> Result<(), StorageError>;
    pub fn replace_mcp_tools(&self, server_id: &str, tools: &[McpToolRow]) -> Result<(), StorageError>;
}
```

Store JSON as text, use foreign keys with cascade, and enforce `UNIQUE(run_id, seq)` plus `UNIQUE(server_id, name)`.

- [ ] **Step 4: Run storage tests and commit**

Run: `cargo test -p lumo-storage`
Expected: PASS with `EXPECTED_USER_VERSION = 4`.

```bash
git add crates/lumo-storage
git commit -m "feat(storage): persist agent events and MCP profiles"
```

### Task 3: Extract a reusable MCP client call

**Files:**
- Modify: `crates/lumo-actions/src/mcp.rs`
- Create: `crates/lumo-actions/tests/mcp_client.rs`

- [ ] **Step 1: Add a fixture-server contract test**

```rust
#[tokio::test]
async fn reusable_client_lists_and_calls_tools() {
    let fixture = McpFixture::stdio(vec![tool("echo", object_schema())]).await;
    let client = McpClient::connect(fixture.profile(), Duration::from_secs(3)).await.unwrap();
    assert_eq!(client.list_tools().await.unwrap()[0].name, "echo");
    let value = client.call_tool("echo", json!({"text":"hi"})).await.unwrap();
    assert!(value.to_string().contains("hi"));
}
```

- [ ] **Step 2: Run and confirm `McpClient` is not public**

Run: `cargo test -p lumo-actions --test mcp_client`
Expected: FAIL to resolve `McpClient`.

- [ ] **Step 3: Move transport logic behind a reusable API and keep the action as an adapter**

```rust
pub enum McpTransportConfig {
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    StreamableHttp { url: String, headers: BTreeMap<String, String> },
}

pub struct McpClient;
impl McpClient {
    pub async fn connect(config: McpTransportConfig, timeout: Duration) -> Result<Self, McpError>;
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError>;
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError>;
}
```

`McpCallAction::execute` must construct this client and preserve its existing JSON result contract.

- [ ] **Step 4: Run old and new MCP tests and commit**

Run: `cargo test -p lumo-actions mcp`
Expected: PASS, including the existing Lumo round-trip when its binary is present.

```bash
git add crates/lumo-actions/src/mcp.rs crates/lumo-actions/tests/mcp_client.rs
git commit -m "refactor(mcp): expose reusable MCP client"
```

### Task 4: Implement generic MCP import adapters

**Files:**
- Modify: `crates/lumo-agent/Cargo.toml`
- Create: `crates/lumo-agent/src/mcp_profile.rs`
- Create: `crates/lumo-agent/src/mcp_import.rs`
- Create: `crates/lumo-agent/tests/mcp_import.rs`
- Create: `crates/lumo-agent/tests/fixtures/mcp/claude.json`
- Create: `crates/lumo-agent/tests/fixtures/mcp/cursor.jsonc`
- Create: `crates/lumo-agent/tests/fixtures/mcp/codex.toml`
- Create: `crates/lumo-agent/tests/fixtures/mcp/servers.yaml`

- [ ] **Step 1: Add table-driven import tests**

```rust
#[test]
fn imports_common_formats_and_extracts_secrets() {
    for fixture in ["claude.json", "cursor.jsonc", "codex.toml", "servers.yaml"] {
        let batch = import_bytes(fixture, fixture_bytes(fixture)).unwrap();
        assert!(!batch.servers.is_empty(), "{fixture}");
        assert!(batch.servers.iter().all(|s| !s.redacted_json().to_string().contains("sk-test")));
    }
}
```

- [ ] **Step 2: Verify unsupported formats and secret leakage fail**

Run: `cargo test -p lumo-agent --test mcp_import`
Expected: FAIL because import adapters do not exist.

- [ ] **Step 3: Implement normalization, conflict keys and secret candidates**

```rust
pub struct McpImportBatch {
    pub servers: Vec<McpServerDraft>,
    pub warnings: Vec<ImportWarning>,
}

pub struct SecretCandidate {
    pub server_id: String,
    pub field_path: String,
    pub suggested_vault_key: String,
    pub value: String,
}

pub fn import_bytes(source_name: &str, bytes: &[u8]) -> Result<McpImportBatch, ImportError>;
pub fn discover_macos_configs(home: &Path) -> Vec<DiscoveredConfig>;
```

Recognize `mcpServers`, `[mcp_servers.<name>]`, single-server objects, stdio, Streamable HTTP and legacy SSE. Treat env/header names containing `TOKEN`, `KEY`, `SECRET`, `PASSWORD`, or `AUTHORIZATION` as secret candidates.

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p lumo-agent --test mcp_import`
Expected: PASS for all fixtures and redaction assertions.

```bash
git add crates/lumo-agent
git commit -m "feat(mcp): import common MCP configurations"
```

### Task 5: Build capability catalog aggregators

**Files:**
- Create: `crates/lumo-agent/src/catalog.rs`
- Create: `crates/lumo-agent/tests/catalog.rs`
- Modify: `crates/lumo-agent/src/lib.rs`

- [ ] **Step 1: Test Flow, Skill and MCP aggregation plus filtering**

```rust
#[test]
fn catalog_filters_disabled_and_unhealthy_capabilities() {
    let catalog = CapabilityCatalog::build(flow_source(), skill_source(), mcp_source()).unwrap();
    assert!(catalog.get("skill:greet").is_some());
    assert!(catalog.visible_for(&profile("safe")).iter().all(|c| c.enabled));
    assert!(catalog.visible_for(&profile("safe")).iter().all(|c| c.risk <= RiskLevel::L1));
}
```

- [ ] **Step 2: Run red test**

Run: `cargo test -p lumo-agent --test catalog`
Expected: FAIL because `CapabilityCatalog` is missing.

- [ ] **Step 3: Implement immutable catalog snapshots**

```rust
pub struct CapabilityCatalog {
    by_id: BTreeMap<String, Arc<CapabilityDescriptor>>,
    alias_index: BTreeMap<String, Vec<String>>,
}

impl CapabilityCatalog {
    pub fn get(&self, id: &str) -> Option<Arc<CapabilityDescriptor>>;
    pub fn exact_alias(&self, utterance: &str) -> Vec<Arc<CapabilityDescriptor>>;
    pub fn visible_for(&self, profile: &AgentProfile) -> Vec<Arc<CapabilityDescriptor>>;
}
```

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p lumo-agent --test catalog`
Expected: PASS.

```bash
git add crates/lumo-agent
git commit -m "feat(agent): aggregate unified capability catalog"
```

### Task 6: Add desktop MCP commands and Capability Hub

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Create: `apps/desktop/src-tauri/src/mcp_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/frontend/src/index.html`
- Create: `apps/desktop/frontend/src/js/capability-hub.js`
- Modify: `apps/desktop/frontend/src/js/main.js`
- Modify: `apps/desktop/frontend/src/js/state.js`
- Create: `apps/desktop/frontend/src/styles/capability-hub.css`
- Create: `apps/desktop/frontend/test/capability-hub.test.js`

- [ ] **Step 1: Write frontend projection and import-preview tests**

```javascript
test("renders mixed import preview without exposing secrets", () => {
  const html = renderImportPreview({ servers: [{ id: "erp", transport: "stdio", secretCount: 1 }] });
  assert.match(html, /ERP|erp/);
  assert.match(html, /1 个敏感字段/);
  assert.doesNotMatch(html, /sk-test/);
});
```

- [ ] **Step 2: Run red frontend test**

Run: `cd apps/desktop/frontend && npm test -- --test-name-pattern='import preview'`
Expected: FAIL because the module is missing.

- [ ] **Step 3: Add focused Tauri commands**

```rust
#[tauri::command]
async fn preview_mcp_import(source_name: String, content: Vec<u8>) -> Result<McpImportPreviewDto, String>;
#[tauri::command]
async fn apply_mcp_import(selection: McpImportSelectionDto, app: AppHandle) -> Result<Vec<McpServerDto>, String>;
#[tauri::command]
async fn test_mcp_server(id: String, app: AppHandle) -> Result<McpHealthDto, String>;
#[tauri::command]
async fn discover_mcp_tools(id: String, app: AppHandle) -> Result<Vec<McpToolDto>, String>;
#[tauri::command]
async fn call_mcp_tool(id: String, tool: String, arguments: Value, app: AppHandle) -> Result<Value, String>;
```

- [ ] **Step 4: Implement the Capability Hub tab and state**

Export pure functions `renderServerRows`, `renderImportPreview`, and `renderToolSchema` for Node tests. Keep DOM binding in `mountCapabilityHub()` and load data with the existing `call()` wrapper.

- [ ] **Step 5: Run desktop checks and commit**

Run: `cd apps/desktop/frontend && npm test && npm run lint`
Expected: PASS.

Run: `cargo test -p lumorpa-desktop`
Expected: PASS.

```bash
git add apps/desktop crates/lumo-agent Cargo.toml
git commit -m "feat(desktop): add MCP capability hub"
```

### Task 7: Add complete Skill lifecycle management

**Files:**
- Create: `crates/lumo-agent/src/skill_manager.rs`
- Create: `crates/lumo-agent/tests/skill_manager.rs`
- Create: `apps/desktop/src-tauri/src/skill_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/frontend/src/js/capability-hub.js`
- Create: `apps/desktop/frontend/test/skill-management.test.js`

- [ ] **Step 1: Write failing import, update and rollback tests**

```rust
#[test]
fn skill_update_is_versioned_and_rollback_restores_previous() {
    let root = tempfile::tempdir().unwrap();
    let manager = SkillManager::new(root.path()).unwrap();
    let v1 = manager.import_local(fixture("greet-v1")).unwrap();
    let v2 = manager.import_local(fixture("greet-v2")).unwrap();
    assert_ne!(v1.version_hash, v2.version_hash);
    manager.activate("greet", &v2.version_hash).unwrap();
    manager.rollback("greet").unwrap();
    assert_eq!(manager.active("greet").unwrap().version_hash, v1.version_hash);
}
```

- [ ] **Step 2: Run the red test**

Run: `cargo test -p lumo-agent --test skill_manager`
Expected: FAIL because `SkillManager` is missing.

- [ ] **Step 3: Implement staged import and immutable versions**

```rust
impl SkillManager {
    pub fn import_local(&self, source: &Path) -> Result<SkillVersion, SkillManagerError>;
    pub async fn import_git(&self, url: &str, revision: Option<&str>) -> Result<SkillVersion, SkillManagerError>;
    pub fn validate(&self, version: &SkillVersion) -> Result<SkillValidationReport, SkillManagerError>;
    pub fn activate(&self, name: &str, version_hash: &str) -> Result<(), SkillManagerError>;
    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<(), SkillManagerError>;
    pub fn rollback(&self, name: &str) -> Result<SkillVersion, SkillManagerError>;
}
```

Copy imported Skills into an app-data staging directory, validate with `load_skill_file`, then atomically activate a version. Never execute directly from a Git checkout or temporary directory.

- [ ] **Step 4: Add Tauri commands and tested UI actions**

Add `import_skill_local`, `import_skill_git`, `validate_skill`, `activate_skill_version`, `set_skill_enabled`, `rollback_skill`, and `test_skill`. Render version history, Flow inputs, declared capabilities, aliases and validation errors through pure frontend renderers.

- [ ] **Step 5: Run checks and commit**

Run: `cargo test -p lumo-agent --test skill_manager && cargo test -p lumorpa-desktop`
Expected: PASS.

Run: `cd apps/desktop/frontend && npm test && npm run lint`
Expected: PASS.

```bash
git add crates/lumo-agent apps/desktop
git commit -m "feat(desktop): manage versioned Skills"
```

### Task 8: Add Agent Profile and permission-policy management

**Files:**
- Create: `crates/lumo-agent/src/profile.rs`
- Create: `crates/lumo-agent/tests/profile.rs`
- Create: `apps/desktop/src-tauri/src/agent_profile_commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/frontend/src/js/capability-hub.js`
- Create: `apps/desktop/frontend/test/agent-profiles.test.js`

- [ ] **Step 1: Test profile validation and visible capability filtering**

```rust
#[test]
fn profile_rejects_unbounded_loop_and_unknown_capability() {
    let err = AgentProfile::validate(AgentProfileDraft {
        max_steps: 0,
        max_parallel: 99,
        visible_capabilities: vec!["missing:tool".into()],
        ..safe_profile()
    }, &catalog()).unwrap_err();
    assert!(err.to_string().contains("max_steps"));
}
```

- [ ] **Step 2: Implement bounded profile model**

```rust
pub struct AgentProfile {
    pub id: String,
    pub planner_provider: String,
    pub validator_provider: String,
    pub reflector_provider: String,
    pub max_steps: u32,
    pub max_parallel: u32,
    pub max_runtime_ms: u64,
    pub max_tokens: u64,
    pub max_cost_usd_micro: u64,
    pub visible_capabilities: BTreeSet<String>,
    pub permission_rules: Vec<PermissionRule>,
}
```

Enforce `1..=100` steps and `1..=16` parallel nodes in validation; desktop defaults remain 20 and 4.

- [ ] **Step 3: Add CRUD commands and UI**

Add `list_agent_profiles`, `save_agent_profile`, `delete_agent_profile`, `set_default_agent_profile`, and render model selectors, budgets, visible capability filters and permission rules.

- [ ] **Step 4: Run checks and commit**

Run: `cargo test -p lumo-agent --test profile && cargo test -p lumorpa-desktop`
Expected: PASS.

Run: `cd apps/desktop/frontend && npm test && npm run lint`
Expected: PASS.

```bash
git add crates/lumo-agent apps/desktop
git commit -m "feat(desktop): configure agent profiles and policies"
```
