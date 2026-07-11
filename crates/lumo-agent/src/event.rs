use std::{collections::BTreeMap, sync::Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEventKind {
    #[serde(rename = "session.started")]
    SessionStarted,
    #[serde(rename = "route.selected")]
    RouteSelected,
    #[serde(rename = "plan.created")]
    PlanCreated,
    #[serde(rename = "plan.revised")]
    PlanRevised,
    #[serde(rename = "permission.requested")]
    PermissionRequested,
    #[serde(rename = "permission.resolved")]
    PermissionResolved,
    #[serde(rename = "run.started")]
    RunStarted,
    #[serde(rename = "run.paused")]
    RunPaused,
    #[serde(rename = "run.resumed")]
    RunResumed,
    #[serde(rename = "run.completed")]
    RunCompleted,
    #[serde(rename = "run.failed")]
    RunFailed,
    #[serde(rename = "run.cancelled")]
    RunCancelled,
    #[serde(rename = "node.queued")]
    NodeQueued,
    #[serde(rename = "node.started")]
    NodeStarted,
    #[serde(rename = "node.progress")]
    NodeProgress,
    #[serde(rename = "tool.called")]
    ToolCalled,
    #[serde(rename = "tool.result")]
    ToolResult,
    #[serde(rename = "node.completed")]
    NodeCompleted,
    #[serde(rename = "node.failed")]
    NodeFailed,
    #[serde(rename = "node.cancelled")]
    NodeCancelled,
}

impl AgentEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session.started",
            Self::RouteSelected => "route.selected",
            Self::PlanCreated => "plan.created",
            Self::PlanRevised => "plan.revised",
            Self::PermissionRequested => "permission.requested",
            Self::PermissionResolved => "permission.resolved",
            Self::RunStarted => "run.started",
            Self::RunPaused => "run.paused",
            Self::RunResumed => "run.resumed",
            Self::RunCompleted => "run.completed",
            Self::RunFailed => "run.failed",
            Self::RunCancelled => "run.cancelled",
            Self::NodeQueued => "node.queued",
            Self::NodeStarted => "node.started",
            Self::NodeProgress => "node.progress",
            Self::ToolCalled => "tool.called",
            Self::ToolResult => "tool.result",
            Self::NodeCompleted => "node.completed",
            Self::NodeFailed => "node.failed",
            Self::NodeCancelled => "node.cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub session_id: String,
    pub run_id: String,
    pub seq: i64,
    pub timestamp: DateTime<Utc>,
    pub kind: AgentEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_node_id: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentEventDraft {
    pub session_id: String,
    pub run_id: String,
    pub kind: AgentEventKind,
    pub node_id: Option<String>,
    pub parent_node_id: Option<String>,
    pub payload: Value,
}

impl AgentEventDraft {
    pub fn new(run_id: impl Into<String>, kind: AgentEventKind) -> Self {
        let run_id = run_id.into();
        Self {
            session_id: run_id.clone(),
            run_id,
            kind,
            node_id: None,
            parent_node_id: None,
            payload: Value::Null,
        }
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = session_id.into();
        self
    }

    pub fn node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub(crate) fn stamp(self, seq: i64) -> AgentEvent {
        AgentEvent {
            session_id: self.session_id,
            run_id: self.run_id,
            seq,
            timestamp: Utc::now(),
            kind: self.kind,
            node_id: self.node_id,
            parent_node_id: self.parent_node_id,
            payload: self.payload,
        }
    }
}

#[derive(Debug, Default)]
pub struct EventSequence {
    by_run: Mutex<BTreeMap<String, i64>>,
}

impl EventSequence {
    pub fn stamp(&self, draft: AgentEventDraft) -> AgentEvent {
        let mut by_run = self.by_run.lock().expect("event sequence lock poisoned");
        let next = by_run.entry(draft.run_id.clone()).or_default();
        *next += 1;
        draft.stamp(*next)
    }
}
