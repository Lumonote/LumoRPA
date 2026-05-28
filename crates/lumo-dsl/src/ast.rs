//! LumoFlow AST: the in-memory representation produced by `parse_str`.
//!
//! YAML schema (excerpt):
//! ```yaml
//! apiVersion: lumorpa.io/v1
//! kind: Flow
//! metadata:
//!   id: hello
//!   version: 0.1.0
//!   name: Hello
//! spec:
//!   inputs:  [{ name: x, type: string, default: world }]
//!   outputs: [{ name: greeting, type: string }]
//!   steps:
//!     - id: greet
//!       action: control.log
//!       with: { message: "hello {{ inputs.x }}" }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flow {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: Metadata,
    pub spec: FlowSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub id: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub ai: Option<FlowAi>,
}

fn default_version() -> String {
    "0.1.0".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowSpec {
    #[serde(default)]
    pub inputs: Vec<IoDecl>,
    #[serde(default)]
    pub outputs: Vec<IoDecl>,
    #[serde(default)]
    pub vault: Vec<String>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub resources: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoDecl {
    pub name: String,
    #[serde(rename = "type", default = "default_type")]
    pub ty: String,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
}

fn default_type() -> String {
    "string".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default, rename = "fs.read")]
    pub fs_read: Vec<String>,
    #[serde(default, rename = "fs.write")]
    pub fs_write: Vec<String>,
    #[serde(default)]
    pub llm: Vec<String>,
    #[serde(default)]
    pub mcp: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    pub kind: String,
    #[serde(default)]
    pub with: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    pub action: String,
    #[serde(default)]
    pub with: serde_yaml::Value,
    #[serde(default)]
    pub retry: Option<RetrySpec>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub ai: Option<StepAi>,
    /// Nested steps for control-flow actions (for / for_each / if / try / parallel).
    #[serde(default, rename = "do")]
    pub do_: Option<Vec<Step>>,
    #[serde(default, rename = "else")]
    pub else_: Option<Vec<Step>>,
    #[serde(default, rename = "catch")]
    pub catch_: Option<Vec<Step>>,
    #[serde(default, rename = "finally")]
    pub finally_: Option<Vec<Step>>,
    /// Multi-step branches for `control.parallel`. Each entry is a sequence of
    /// steps run concurrently with its siblings; entries within one branch
    /// still execute in declaration order. Mutually exclusive with `do_` for
    /// that action but `do_` is also accepted (each top-level step becomes a
    /// one-step branch).
    #[serde(default)]
    pub branches: Option<Vec<Vec<Step>>>,
}

impl Step {
    /// All children in execution order (do / else / catch / finally / branches).
    pub fn children(&self) -> Vec<&[Step]> {
        let mut v = Vec::new();
        if let Some(d) = &self.do_ {
            v.push(d.as_slice());
        }
        if let Some(e) = &self.else_ {
            v.push(e.as_slice());
        }
        if let Some(c) = &self.catch_ {
            v.push(c.as_slice());
        }
        if let Some(f) = &self.finally_ {
            v.push(f.as_slice());
        }
        if let Some(bs) = &self.branches {
            for b in bs {
                v.push(b.as_slice());
            }
        }
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrySpec {
    #[serde(default = "default_retry_times")]
    pub times: u32,
    #[serde(default = "default_backoff")]
    pub backoff: String,
    #[serde(default = "default_initial_delay")]
    pub initial_ms: u64,
    #[serde(default)]
    pub on: Vec<String>,
}

fn default_retry_times() -> u32 {
    0
}
fn default_backoff() -> String {
    "fixed".into()
}
fn default_initial_delay() -> u64 {
    500
}

/// Per-step AI augmentation policy. Absent ⇒ `mode: off`, fully back-compat.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepAi {
    #[serde(default)]
    pub mode: AiMode,
    /// Override `metadata.ai.model`. Empty ⇒ inherit.
    #[serde(default)]
    pub model: Option<String>,
    /// Natural-language goal. Empty ⇒ VM auto-constructs from action + with.
    #[serde(default)]
    pub prompt: Option<String>,
}

impl StepAi {
    pub fn is_enabled(&self) -> bool {
        !matches!(self.mode, AiMode::Off)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    #[default]
    Off,
    Fallback,
    Primary,
}

/// Flow-level AI policy. Absent ⇒ all defaults; nodes with `ai: { mode: off }` still skip AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowAi {
    /// Master switch. `false` ⇒ ignore all step-level `ai.mode`.
    #[serde(default = "default_flow_ai_enabled")]
    pub enabled: bool,
    /// Default model for AI-enabled steps. Empty ⇒ use router active provider.
    #[serde(default)]
    pub model: Option<String>,
    /// On any step failure, attach LLM diagnostic.
    #[serde(default)]
    pub diagnose_on_failure: bool,
    #[serde(default)]
    pub budget: AiBudget,
}

impl Default for FlowAi {
    fn default() -> Self {
        Self {
            enabled: true,
            model: None,
            diagnose_on_failure: false,
            budget: AiBudget::default(),
        }
    }
}

fn default_flow_ai_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiBudget {
    #[serde(default = "default_max_calls_per_run")]
    pub max_calls_per_run: u32,
}

impl Default for AiBudget {
    fn default() -> Self {
        Self {
            max_calls_per_run: default_max_calls_per_run(),
        }
    }
}

fn default_max_calls_per_run() -> u32 {
    100
}

/// Hand-written schema to be returned by `lumo actions --show <id>` etc.
/// Avoids pulling `schemars` derive across all action crates.
pub fn flow_json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "LumoFlow",
        "type": "object",
        "required": ["apiVersion", "kind", "metadata", "spec"],
        "properties": {
            "apiVersion": { "type": "string", "const": "lumorpa.io/v1" },
            "kind": { "type": "string", "const": "Flow" },
            "metadata": {
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id":      { "type": "string" },
                    "version": { "type": "string" },
                    "name":    { "type": "string" }
                }
            },
            "spec": { "type": "object" }
        }
    })
}
