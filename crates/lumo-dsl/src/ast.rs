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
    /// Nested steps for control-flow actions (for / for_each / if / try / parallel).
    #[serde(default, rename = "do")]
    pub do_: Option<Vec<Step>>,
    #[serde(default, rename = "else")]
    pub else_: Option<Vec<Step>>,
    #[serde(default, rename = "catch")]
    pub catch_: Option<Vec<Step>>,
    #[serde(default, rename = "finally")]
    pub finally_: Option<Vec<Step>>,
}

impl Step {
    /// All children in execution order (do / else / catch / finally).
    pub fn children(&self) -> Vec<&[Step]> {
        let mut v = Vec::new();
        if let Some(d) = &self.do_      { v.push(d.as_slice()); }
        if let Some(e) = &self.else_    { v.push(e.as_slice()); }
        if let Some(c) = &self.catch_   { v.push(c.as_slice()); }
        if let Some(f) = &self.finally_ { v.push(f.as_slice()); }
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

fn default_retry_times() -> u32 { 0 }
fn default_backoff() -> String { "fixed".into() }
fn default_initial_delay() -> u64 { 500 }

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
