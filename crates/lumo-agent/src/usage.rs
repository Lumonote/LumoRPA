use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use chrono::Utc;
use lumo_storage::{InvocationAttemptRow, InvocationFlowRunRow, Repo};

use crate::{CapabilityKind, InvocationError, InvocationResult};

#[derive(Clone)]
pub struct InvocationUsageReporter {
    repo: Repo,
    row: InvocationAttemptRow,
    observer_failed: Arc<AtomicBool>,
}

pub(crate) struct InvocationAttemptGuard {
    reporter: InvocationUsageReporter,
    started: Instant,
    finished: bool,
}

impl InvocationUsageReporter {
    pub(crate) fn begin(
        repo: Repo,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        capability_id: &str,
        source: CapabilityKind,
        budget_sensitive: bool,
    ) -> Result<InvocationAttemptGuard, String> {
        let flow_run_id = if source == CapabilityKind::Mcp {
            None
        } else {
            Some(format!(
                "agent-{}",
                crate::plan::stable_json_hash(&serde_json::json!([run_id, node_id, attempt]))
            ))
        };
        let row = InvocationAttemptRow {
            run_id: run_id.into(),
            node_id: node_id.into(),
            attempt,
            capability_id: capability_id.into(),
            source_kind: match source {
                CapabilityKind::Flow => "flow",
                CapabilityKind::Skill => "skill",
                CapabilityKind::Mcp => "mcp",
            }
            .into(),
            flow_run_id,
            state: "running".into(),
            usage_state: "pending".into(),
            budget_sensitive,
            tokens_used: None,
            cost_usd_micro: None,
            duration_ms: 0,
            started_at: Utc::now(),
            finished_at: None,
            error: None,
            revision: 0,
            resolved_by: None,
            resolution_reason: None,
            resolved_at: None,
        };
        repo.begin_invocation_attempt(&row)
            .map_err(|e| e.to_string())?;
        Ok(InvocationAttemptGuard {
            reporter: Self {
                repo,
                row,
                observer_failed: Arc::new(AtomicBool::new(false)),
            },
            started: Instant::now(),
            finished: false,
        })
    }

    pub fn flow_run_id(&self) -> Option<String> {
        self.row.flow_run_id.clone()
    }

    pub fn set_budget_sensitive(&self, sensitive: bool) -> Result<(), String> {
        if let Err(error) = self.repo.set_invocation_budget_sensitive(
            &self.row.run_id,
            &self.row.node_id,
            self.row.attempt,
            sensitive,
        ) {
            self.observer_failed.store(true, Ordering::SeqCst);
            return Err(error.to_string());
        }
        Ok(())
    }

    pub fn observer(&self) -> lumo_core::RunUsageObserver {
        let reporter = self.clone();
        Arc::new(move |event| {
            use lumo_core::RunUsageEvent;
            let row = match event {
                RunUsageEvent::Started {
                    run_id,
                    parent_run_id,
                } => InvocationFlowRunRow {
                    flow_run_id: run_id.clone(),
                    run_id: reporter.row.run_id.clone(),
                    node_id: reporter.row.node_id.clone(),
                    attempt: reporter.row.attempt,
                    parent_flow_run_id: parent_run_id.clone(),
                    state: "running".into(),
                    usage_unknown: true,
                    tokens_used: None,
                    cost_usd_micro: None,
                    duration_ms: 0,
                },
                RunUsageEvent::Finished {
                    run_id,
                    parent_run_id,
                    tokens_used,
                    cost_usd_micro,
                    usage_unknown,
                    state,
                    duration_ms,
                } => InvocationFlowRunRow {
                    flow_run_id: run_id.clone(),
                    run_id: reporter.row.run_id.clone(),
                    node_id: reporter.row.node_id.clone(),
                    attempt: reporter.row.attempt,
                    parent_flow_run_id: parent_run_id.clone(),
                    state: state.clone(),
                    usage_unknown: *usage_unknown,
                    tokens_used: Some(*tokens_used),
                    cost_usd_micro: Some(*cost_usd_micro),
                    duration_ms: *duration_ms,
                },
            };
            if reporter.repo.record_invocation_flow_run(&row).is_err() {
                reporter.observer_failed.store(true, Ordering::SeqCst);
            }
        })
    }
}

impl InvocationAttemptGuard {
    pub(crate) fn reporter(&self) -> InvocationUsageReporter {
        self.reporter.clone()
    }

    pub(crate) fn finish(
        mut self,
        result: &Result<InvocationResult, InvocationError>,
    ) -> Result<InvocationAttemptRow, String> {
        let mut row = self.reporter.row.clone();
        row.duration_ms = self
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        row.finished_at = Some(Utc::now());
        match result {
            Ok(result) => {
                row.duration_ms = row.duration_ms.max(result.duration_ms);
                row.state = "completed".into();
                row.tokens_used = Some(result.tokens_used);
                row.cost_usd_micro = Some(result.cost_usd_micro);
                row.usage_state = if result.metadata.get("usageState").and_then(|v| v.as_str())
                    == Some("unknown")
                {
                    "unknown"
                } else {
                    "recorded"
                }
                .into();
            }
            Err(error) => {
                row.state = match error {
                    InvocationError::Cancelled => "cancelled",
                    InvocationError::Timeout { .. } => "timed_out",
                    _ => "failed",
                }
                .into();
                row.usage_state = "unknown".into();
                row.error = Some(error.to_string());
            }
        }
        if self.reporter.observer_failed.load(Ordering::SeqCst) {
            row.usage_state = "unknown".into();
            row.error = Some("usage persistence failed; manual reconciliation required".into());
        }
        self.reporter
            .repo
            .finish_invocation_attempt(&row)
            .map_err(|e| e.to_string())?;
        self.finished = true;
        let saved = self
            .reporter
            .repo
            .get_invocation_attempt(&row.run_id, &row.node_id, row.attempt)
            .map_err(|e| e.to_string())?
            .ok_or("invocation usage disappeared")?;
        if self.reporter.observer_failed.load(Ordering::SeqCst) {
            return Err("usage persistence failed; manual reconciliation required".into());
        }
        Ok(saved)
    }
}

impl Drop for InvocationAttemptGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut row = self.reporter.row.clone();
        row.state = "dropped".into();
        row.usage_state = "unknown".into();
        row.error = Some("invocation stopped before reporting usage".into());
        row.duration_ms = self
            .started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);
        row.finished_at = Some(Utc::now());
        let _ = self.reporter.repo.finish_invocation_attempt(&row);
    }
}
