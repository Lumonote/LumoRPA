mod flow;
mod mcp;
mod skill;

use std::{collections::BTreeMap, sync::Arc, time::Instant};

use async_trait::async_trait;
use lumo_core::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{CapabilityDescriptor, CapabilitySource};

pub use flow::{FlowAdapter, FlowVmFactory};
pub use mcp::{
    McpAdapter, McpClientInvoker, McpConnectionProfile, McpProfileResolver, McpToolInvoker,
};
pub use skill::SkillAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityKind {
    Flow,
    Skill,
    Mcp,
}

impl CapabilityKind {
    pub fn from_source(source: &CapabilitySource) -> Self {
        match source {
            CapabilitySource::Flow { .. } => Self::Flow,
            CapabilitySource::Skill { .. } => Self::Skill,
            CapabilitySource::Mcp { .. } => Self::Mcp,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InvocationRequest {
    pub capability: CapabilityDescriptor,
    pub arguments: Value,
    pub attempt: u32,
    pub timeout_ms: u64,
}

#[derive(Clone)]
pub struct InvocationContext {
    pub run_id: String,
    pub node_id: String,
    pub cancel: CancelToken,
    pub metadata: BTreeMap<String, Value>,
}

impl InvocationContext {
    pub fn new(run_id: impl Into<String>, node_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            node_id: node_id.into(),
            cancel: CancelToken::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvocationResult {
    pub output: Value,
    pub duration_ms: u64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub cost_usd_micro: u64,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl InvocationResult {
    pub fn new(output: Value, started: Instant) -> Self {
        Self {
            output: normalize_empty_output(output),
            duration_ms: started.elapsed().as_millis() as u64,
            tokens_used: 0,
            cost_usd_micro: 0,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvocationError {
    #[error("adapter received an incompatible capability source")]
    InvalidSource,
    #[error("capability is unavailable: {0}")]
    Unavailable(String),
    #[error("invocation was cancelled")]
    Cancelled,
    #[error("invocation timed out after {duration_ms}ms")]
    Timeout { duration_ms: u64 },
    #[error("invocation failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait InvocationAdapter: Send + Sync {
    fn source_kind(&self) -> CapabilityKind;

    async fn invoke(
        &self,
        request: InvocationRequest,
        context: InvocationContext,
    ) -> Result<InvocationResult, InvocationError>;
}

#[derive(Default, Clone)]
pub struct AdapterRegistry {
    adapters: BTreeMap<CapabilityKind, Arc<dyn InvocationAdapter>>,
}

impl AdapterRegistry {
    pub fn register(&mut self, adapter: Arc<dyn InvocationAdapter>) {
        self.adapters.insert(adapter.source_kind(), adapter);
    }

    pub fn get(&self, source: &CapabilitySource) -> Option<Arc<dyn InvocationAdapter>> {
        self.adapters.get(&CapabilityKind::from_source(source)).cloned()
    }
}

fn normalize_empty_output(output: Value) -> Value {
    match output {
        Value::Object(values) if values.is_empty() => Value::Null,
        value => value,
    }
}
