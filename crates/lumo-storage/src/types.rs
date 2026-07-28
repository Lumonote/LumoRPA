use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRunRow {
    pub id: String,
    pub profile_id: Option<String>,
    pub utterance: Option<String>,
    pub plan_json: Option<Value>,
    pub approval_json: Option<Value>,
    pub state: String,
    /// Run start time, stored as Unix milliseconds.
    pub started_at: DateTime<Utc>,
    /// Run finish time, stored as Unix milliseconds when present.
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventRow {
    pub run_id: String,
    pub seq: i64,
    pub kind: String,
    pub node_id: Option<String>,
    pub parent_node_id: Option<String>,
    pub payload: Value,
    /// Event creation time, stored as Unix milliseconds.
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentEventInsert<'a> {
    pub run_id: &'a str,
    pub seq: i64,
    pub kind: &'a str,
    pub node_id: Option<&'a str>,
    pub parent_node_id: Option<&'a str>,
    pub payload: &'a Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAgentJob {
    pub id: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub schedule_kind: String,
    pub schedule_spec: Value,
    pub priority: i64,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentJobRow {
    pub id: String,
    pub idempotency_key: String,
    pub payload: Value,
    pub schedule_kind: String,
    pub schedule_spec: Value,
    pub state: String,
    pub priority: i64,
    pub available_at: DateTime<Utc>,
    pub attempts: i64,
    pub max_attempts: i64,
    pub worker_id: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueJobResult {
    pub job: AgentJobRow,
    pub inserted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobNodeCheckpoint {
    pub job_id: String,
    pub node_id: String,
    pub state: String,
    pub risk: String,
    pub idempotent: bool,
    pub attempt: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveredJob {
    pub job_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRow {
    pub id: String,
    pub name: String,
    pub transport: String,
    pub config: Value,
    pub enabled: bool,
    pub health: String,
    /// Server creation time, stored as Unix milliseconds.
    pub created_at: DateTime<Utc>,
    /// Last server update time, stored as Unix milliseconds.
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolRow {
    pub server_id: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub risk: String,
    pub enabled: bool,
    pub version_hash: String,
    /// Tool discovery time, stored as Unix milliseconds.
    pub discovered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowRunRow {
    pub id: String,
    pub flow_id: String,
    pub flow_version: String,
    pub trigger_kind: String,
    pub inputs: serde_json::Value,
    pub outputs: Option<serde_json::Value>,
    pub state: String, // queued | running | ok | failed | cancelled
    pub worker_id: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub cost_token: i64,
    pub cost_usd_micro: i64,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRunRow {
    pub flow_run_id: String,
    pub seq: i64,
    pub path: String,
    pub parent_path: Option<String>,
    pub depth: i64,
    pub step_id: String,
    pub idx: i64,
    pub state: String,
    pub attempt: i64,
    pub input_hash: Vec<u8>,
    pub output_json: Option<serde_json::Value>,
    /// F-19 variable watch: snapshot of the `vars` (set_var) environment as of
    /// after this step.
    ///
    /// P1-3(vars_json 治理)后的取值语义:
    ///   * `Some(map)` —— 全量快照;
    ///   * `Some({"__truncated__": true, "bytes": N})` —— 快照序列化超过引擎
    ///     侧上限(lumo-core `VARS_JSON_MAX_BYTES`),只存截断标记;
    ///   * `None`(库内 NULL)—— vars 与同 run 前一条 seq 行相同(写入侧去
    ///     重)。[`crate::Repo::list_steps`] 读取时已向前回溯补齐,调用方通常
    ///     看不到这种 `None`;仅 v3 之前的老行(整跑无快照)保持 `None`。
    pub vars_json: Option<serde_json::Value>,
    pub error: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRow {
    pub id: String,
    pub flow_run_id: String,
    pub step_id: Option<String>,
    pub kind: String, // screenshot | dom | har | video | table | file
    pub mime: String,
    pub size: i64,
    pub blob_path: String,
    pub sha256: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

/// X-10 cost accounting row. One row per LLM/vision call inside a flow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCallRow {
    pub id: i64,
    pub flow_run_id: String,
    pub step_id: Option<String>,
    pub helper: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub latency_ms: i64,
    pub cost_usd_micro: i64,
    pub created_at: DateTime<Utc>,
}

/// New-row payload (no `id` / `created_at` — repo fills them in).
#[derive(Debug, Clone)]
pub struct AiCallInsert<'a> {
    pub flow_run_id: &'a str,
    pub step_id: Option<&'a str>,
    pub helper: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub latency_ms: i64,
    pub cost_usd_micro: i64,
}

/// A row of the `vault_items` table (P1-3). `age_ciphertext` is opaque age
/// binary; `metadata` is non-sensitive JSON (field names only).
#[derive(Debug, Clone)]
pub struct VaultRow {
    pub name: String,
    pub age_ciphertext: Vec<u8>,
    pub metadata: String,
    pub updated_at: i64,
}

/// 架构 P2(runs retention):[`crate::Repo::prune_runs`] 的保留策略。
/// running / queued / paused 状态的 run 在任一策略下都不删(还在写行 /
/// 可续跑)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrunePolicy {
    /// 保留最近 N 天内启动的 run(按 `started_at`),更早的删除。
    KeepDays(u32),
    /// 按 `started_at` 倒序保留最新 N 条 run,其余删除。
    KeepCount(u32),
}

/// 架构 P2:一次 [`crate::Repo::prune_runs`] 删除了什么。行删除(flow_runs
/// 及级联的 step_runs / artifacts / ai_calls)在同一事务内完成;artifacts 的
/// blob **文件**无法参与 SQLite 事务,其路径经 `blob_paths` 返回,由调用方在
/// 提交后自行 best-effort 清理。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// 删除的 flow_runs 行数。
    pub runs: usize,
    /// 级联删除的 step_runs 行数。
    pub steps: usize,
    /// 级联删除的 artifacts 行数(= `blob_paths.len()`)。
    pub artifacts: usize,
    /// 级联删除的 ai_calls 行数。
    pub ai_calls: usize,
    /// 被删 artifacts 记录指向的 blob 文件路径,供调用方清理磁盘。
    pub blob_paths: Vec<String>,
}
