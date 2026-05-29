use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    pub kind: String, // screenshot | dom | har | video | file
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
