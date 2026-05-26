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
    pub state: String,                  // queued | running | ok | failed | cancelled
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
    pub kind: String,                   // screenshot | dom | har | video | file
    pub mime: String,
    pub size: i64,
    pub blob_path: String,
    pub sha256: Vec<u8>,
    pub created_at: DateTime<Utc>,
}
