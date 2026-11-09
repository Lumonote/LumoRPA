use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use lumo_core::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::Mutex, task::JoinSet};

use crate::{
    validate_invocation_result, AdapterRegistry, AgentBudget, AgentControlState, AgentEventDraft,
    AgentEventKind, AgentPlan, AgentProfile, AgentRunControl, ApprovalSnapshot, BudgetExceeded,
    CapabilityCatalog, EventSink, InvocationContext, InvocationError, InvocationRequest,
    InvocationResult, JobCheckpointStore, PlanValidator, RiskLevel,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum LoopDecision {
    Complete,
    Retry {
        node_id: String,
    },
    Replace {
        node_id: String,
        capability_id: String,
    },
    Replan {
        reason: String,
    },
    AskUser {
        question: String,
    },
    Fail {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopStatus {
    Completed,
    Failed,
    Cancelled,
    BudgetExceeded,
    WaitingApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopReport {
    pub run_id: String,
    pub status: LoopStatus,
    pub outputs: BTreeMap<String, InvocationResult>,
    pub attempts: BTreeMap<String, u32>,
    pub decisions: Vec<LoopDecision>,
    pub error: Option<String>,
}

impl LoopReport {
    fn terminal(
        run_id: String,
        status: LoopStatus,
        outputs: BTreeMap<String, InvocationResult>,
        attempts: BTreeMap<String, u32>,
        decisions: Vec<LoopDecision>,
        error: impl Into<Option<String>>,
    ) -> Self {
        Self {
            run_id,
            status,
            outputs,
            attempts,
            decisions,
            error: error.into(),
        }
    }
}

#[async_trait]
pub trait AgentEventPublisher: Send + Sync {
    async fn publish(&self, draft: AgentEventDraft) -> Result<(), String>;
}

#[async_trait]
impl AgentEventPublisher for EventSink {
    async fn publish(&self, draft: AgentEventDraft) -> Result<(), String> {
        EventSink::publish(self, draft)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
pub struct LoopEngine {
    catalog: CapabilityCatalog,
    profile: AgentProfile,
    adapters: AdapterRegistry,
    events: Option<Arc<dyn AgentEventPublisher>>,
    control: Option<AgentRunControl>,
    checkpoint: Option<JobCheckpointStore>,
    approval: Option<ApprovalSnapshot>,
}

impl LoopEngine {
    pub fn new(
        catalog: CapabilityCatalog,
        profile: AgentProfile,
        adapters: AdapterRegistry,
    ) -> Self {
        Self {
            catalog,
            profile,
            adapters,
            events: None,
            control: None,
            checkpoint: None,
            approval: None,
        }
    }

    pub fn with_event_publisher(mut self, events: Arc<dyn AgentEventPublisher>) -> Self {
        self.events = Some(events);
        self
    }

    pub fn with_run_control(mut self, control: AgentRunControl) -> Self {
        self.control = Some(control);
        self
    }

    pub fn with_job_checkpoint(mut self, checkpoint: JobCheckpointStore) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    pub fn with_approval_snapshot(mut self, approval: ApprovalSnapshot) -> Self {
        self.approval = Some(approval);
        self
    }

    pub async fn execute(
        &self,
        plan: AgentPlan,
        budget: AgentBudget,
        cancel: CancelToken,
    ) -> LoopReport {
        let report = self.execute_inner(plan, budget, cancel).await;
        if let Some(control) = &self.control {
            control.finish(report.status);
        }
        let kind = match report.status {
            LoopStatus::Completed => Some(AgentEventKind::RunCompleted),
            LoopStatus::Failed | LoopStatus::BudgetExceeded if self.checkpoint.is_none() => {
                Some(AgentEventKind::RunFailed)
            }
            LoopStatus::Cancelled if self.checkpoint.is_none() => {
                Some(AgentEventKind::RunCancelled)
            }
            LoopStatus::Failed | LoopStatus::BudgetExceeded | LoopStatus::Cancelled => None,
            LoopStatus::WaitingApproval => None,
        };
        if let Some(kind) = kind {
            self.emit(
                AgentEventDraft::new(&report.run_id, kind).payload(json!({"error": report.error})),
            )
            .await;
        }
        report
    }

    async fn execute_inner(
        &self,
        plan: AgentPlan,
        mut budget: AgentBudget,
        cancel: CancelToken,
    ) -> LoopReport {
        let run_id = plan.id.clone();
        let mut validated = match PlanValidator::new(&self.catalog, &self.profile).validate(plan) {
            Ok(validated) => validated,
            Err(error) => {
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Failed,
                    BTreeMap::new(),
                    BTreeMap::new(),
                    vec![LoopDecision::Fail {
                        reason: error.to_string(),
                    }],
                    Some(error.to_string()),
                );
            }
        };
        let (mut outputs, mut attempts) = match &self.checkpoint {
            Some(checkpoint) => match checkpoint.restore(&validated.plan, &self.catalog) {
                Ok(restored) => (restored.outputs, restored.attempts),
                Err(error) => {
                    return LoopReport::terminal(
                        run_id,
                        LoopStatus::Failed,
                        BTreeMap::new(),
                        BTreeMap::new(),
                        vec![],
                        Some(error),
                    )
                }
            },
            None => (BTreeMap::new(), BTreeMap::new()),
        };
        let steps = attempts.values().copied().fold(0_u32, u32::saturating_add);
        let (tokens, cost, runtime) =
            outputs
                .values()
                .fold((0_u64, 0_u64, 0_u64), |usage, result| {
                    (
                        usage.0.saturating_add(result.tokens_used),
                        usage.1.saturating_add(result.cost_usd_micro),
                        usage.2.saturating_add(result.duration_ms),
                    )
                });
        if let Err(error) = budget.restore_recorded_usage(steps, tokens, cost, runtime) {
            return budget_report(run_id, outputs, attempts, Vec::new(), error);
        }
        validated
            .approval_required
            .retain(|id| !outputs.contains_key(id));
        if let Some(approval) = &self.approval {
            if approval.plan_hash == validated.plan.stable_hash()
                && validated.plan.nodes.iter().all(|node| {
                    self.catalog
                        .get(&node.capability_id)
                        .is_some_and(|capability| {
                            approval.capability_versions.get(&node.capability_id)
                                == Some(&capability.version_hash)
                                && approval.schema_hashes.get(&node.capability_id)
                                    == Some(&crate::plan::stable_json_hash(
                                        &capability.input_schema,
                                    ))
                        })
                })
            {
                validated
                    .approval_required
                    .retain(|id| !approval.approved_nodes.contains(id));
            }
        }
        if let Some(control) = &self.control {
            let capabilities = validated
                .plan
                .nodes
                .iter()
                .filter_map(|node| {
                    self.catalog
                        .get(&node.capability_id)
                        .map(|capability| (*capability).clone())
                })
                .collect::<Vec<_>>();
            let required = validated
                .plan
                .nodes
                .iter()
                .filter(|node| validated.approval_required.contains(&node.id))
                .map(|node| (node.id.clone(), node.risk))
                .collect();
            control.prepare(&validated.plan, &capabilities, required);
        }
        for node in &validated.plan.nodes {
            if validated.approval_required.contains(&node.id) {
                self.emit(AgentEventDraft::new(&run_id, AgentEventKind::PermissionRequested)
                    .node_id(&node.id).payload(json!({
                        "planHash": validated.plan.stable_hash(), "nodeIds": [&node.id],
                        "capabilityId": node.capability_id, "arguments": node.arguments,
                        "risk": node.risk, "strength": if node.risk == RiskLevel::L3 { "strengthened" } else { "standard" },
                        "reason": "Agent policy requires confirmation before execution",
                    }))).await;
            }
        }
        if !validated.approval_required.is_empty() && self.control.is_none() {
            return LoopReport::terminal(
                run_id,
                LoopStatus::WaitingApproval,
                outputs,
                attempts,
                vec![LoopDecision::AskUser {
                    question: format!("Approve nodes {:?}?", validated.approval_required),
                }],
                None,
            );
        }
        if let Err(status) = self.wait_until_running(&budget, &cancel).await {
            return stopped_report(run_id, status, outputs, attempts, Vec::new());
        }
        let plan = validated.plan;
        let nodes = plan
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut completed = outputs.keys().cloned().collect::<BTreeSet<_>>();
        let mut pending = nodes
            .keys()
            .filter(|id| !completed.contains(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut decisions = Vec::new();
        let budget = Arc::new(Mutex::new(budget));
        let max_parallel = self.profile.max_parallel.clamp(1, 4) as usize;
        let mut running = JoinSet::new();
        let mut control_updates = self.control.as_ref().map(AgentRunControl::subscribe);

        self.emit(AgentEventDraft::new(&run_id, AgentEventKind::RunStarted))
            .await;
        for node in nodes.values() {
            if completed.contains(&node.id) {
                continue;
            }
            self.emit(
                AgentEventDraft::new(&run_id, AgentEventKind::NodeQueued)
                    .node_id(&node.id)
                    .payload(json!({"capabilityId": node.capability_id})),
            )
            .await;
        }

        loop {
            if cancel.is_cancelled() {
                running.abort_all();
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Cancelled,
                    outputs,
                    attempts,
                    decisions,
                    Some("cancelled".into()),
                );
            }
            if completed.len() == nodes.len() {
                decisions.push(LoopDecision::Complete);
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Completed,
                    outputs,
                    attempts,
                    decisions,
                    None,
                );
            }

            if running.is_empty() {
                if let Err(status) = self
                    .wait_until_running(&*budget.lock().await, &cancel)
                    .await
                {
                    return stopped_report(run_id, status, outputs, attempts, decisions);
                }
            }
            while running.len() < max_parallel {
                if self
                    .control
                    .as_ref()
                    .is_some_and(|control| control.state() != AgentControlState::Running)
                {
                    break;
                }
                let ready = pending
                    .iter()
                    .find(|id| {
                        nodes[*id]
                            .depends_on
                            .iter()
                            .all(|dependency| completed.contains(dependency))
                    })
                    .cloned();
                let Some(node_id) = ready else { break };
                if let Err(error) = budget.lock().await.reserve_step() {
                    running.abort_all();
                    return budget_report(run_id, outputs, attempts, decisions, error);
                }
                pending.remove(&node_id);
                let attempt = attempts.entry(node_id.clone()).or_insert(0);
                *attempt += 1;
                let attempt_number = *attempt;
                let node = nodes[&node_id].clone();
                let Some(capability) = self.catalog.get(&node.capability_id) else {
                    return LoopReport::terminal(
                        run_id,
                        LoopStatus::Failed,
                        outputs,
                        attempts,
                        decisions,
                        Some("capability disappeared".into()),
                    );
                };
                let Some(adapter) = self.adapters.get(&capability.source) else {
                    return LoopReport::terminal(
                        run_id,
                        LoopStatus::Failed,
                        outputs,
                        attempts,
                        decisions,
                        Some(format!("no adapter for {}", capability.id)),
                    );
                };
                if let Some(checkpoint) = &self.checkpoint {
                    if let Err(error) =
                        checkpoint.save(&plan, &node, &self.catalog, attempt_number, None)
                    {
                        running.abort_all();
                        return LoopReport::terminal(
                            run_id,
                            LoopStatus::Failed,
                            outputs,
                            attempts,
                            decisions,
                            Some(error),
                        );
                    }
                }
                self.emit(
                    AgentEventDraft::new(&run_id, AgentEventKind::NodeStarted)
                        .node_id(&node_id)
                        .payload(json!({"attempt": attempt_number})),
                )
                .await;
                let context = InvocationContext::new(&run_id, &node_id).with_cancel(cancel.clone());
                running.spawn(async move {
                    let request = InvocationRequest {
                        capability: (*capability).clone(),
                        arguments: node.arguments.clone(),
                        attempt: attempt_number,
                        timeout_ms: node.timeout_ms,
                    };
                    let result = tokio::time::timeout(
                        Duration::from_millis(node.timeout_ms),
                        adapter.invoke(request, context),
                    )
                    .await
                    .map_err(|_| InvocationError::Timeout {
                        duration_ms: node.timeout_ms,
                    })
                    .and_then(|result| result);
                    (node, attempt_number, result)
                });
            }

            if running.is_empty() {
                if self
                    .control
                    .as_ref()
                    .is_some_and(|control| control.state() != AgentControlState::Running)
                {
                    continue;
                }
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Failed,
                    outputs,
                    attempts,
                    decisions,
                    Some("plan made no progress".into()),
                );
            }

            let remaining = match budget.lock().await.remaining_runtime() {
                Ok(remaining) => remaining,
                Err(error) => {
                    running.abort_all();
                    return budget_report(run_id, outputs, attempts, decisions, error);
                }
            };
            let joined = tokio::select! {
                _ = async {
                    match control_updates.as_mut() {
                        Some(changes) => { let _ = changes.changed().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => continue,
                _ = cancel.cancelled() => {
                    running.abort_all();
                    continue;
                }
                joined = tokio::time::timeout(remaining, running.join_next()) => joined,
            };
            let Some(joined) = (match joined {
                Ok(joined) => joined,
                Err(_) => {
                    running.abort_all();
                    return budget_report(
                        run_id,
                        outputs,
                        attempts,
                        decisions,
                        BudgetExceeded::Runtime { limit_ms: 0 },
                    );
                }
            }) else {
                continue;
            };
            let (node, attempt, result) = match joined {
                Ok(result) => result,
                Err(error) => {
                    return LoopReport::terminal(
                        run_id,
                        LoopStatus::Failed,
                        outputs,
                        attempts,
                        decisions,
                        Some(error.to_string()),
                    );
                }
            };
            match result.and_then(|result| {
                validate_invocation_result(&node, &result)
                    .map_err(|error| InvocationError::Failed(error.to_string()))?;
                Ok(result)
            }) {
                Ok(result) => {
                    if let Some(checkpoint) = &self.checkpoint {
                        if let Err(error) =
                            checkpoint.save(&plan, &node, &self.catalog, attempt, Some(&result))
                        {
                            running.abort_all();
                            return LoopReport::terminal(
                                run_id,
                                LoopStatus::Failed,
                                outputs,
                                attempts,
                                decisions,
                                Some(error),
                            );
                        }
                    }
                    if let Err(error) = budget
                        .lock()
                        .await
                        .charge_usage(result.tokens_used, result.cost_usd_micro)
                    {
                        running.abort_all();
                        return budget_report(run_id, outputs, attempts, decisions, error);
                    }
                    completed.insert(node.id.clone());
                    outputs.insert(node.id.clone(), result.clone());
                    self.emit(
                        AgentEventDraft::new(&run_id, AgentEventKind::NodeCompleted)
                            .node_id(&node.id)
                            .payload(json!({"attempt": attempt, "result": result.output})),
                    )
                    .await;
                }
                Err(InvocationError::Cancelled) if cancel.is_cancelled() => {
                    running.abort_all();
                }
                Err(error)
                    if attempt <= node.retry_limit
                        && (self.checkpoint.is_none() || node.risk == RiskLevel::L0) =>
                {
                    decisions.push(LoopDecision::Retry {
                        node_id: node.id.clone(),
                    });
                    pending.insert(node.id.clone());
                    self.emit(
                        AgentEventDraft::new(&run_id, AgentEventKind::NodeFailed)
                            .node_id(&node.id)
                            .payload(json!({"attempt": attempt, "error": error.to_string(), "retrying": true})),
                    )
                    .await;
                }
                Err(error) => {
                    running.abort_all();
                    decisions.push(LoopDecision::Fail {
                        reason: error.to_string(),
                    });
                    self.emit(
                        AgentEventDraft::new(&run_id, AgentEventKind::NodeFailed)
                            .node_id(&node.id)
                            .payload(json!({"attempt": attempt, "error": error.to_string()})),
                    )
                    .await;
                    return LoopReport::terminal(
                        run_id,
                        LoopStatus::Failed,
                        outputs,
                        attempts,
                        decisions,
                        Some(error.to_string()),
                    );
                }
            }
        }
    }

    async fn wait_until_running(
        &self,
        budget: &AgentBudget,
        cancel: &CancelToken,
    ) -> Result<(), LoopStatus> {
        let Some(control) = &self.control else {
            return Ok(());
        };
        let mut changes = control.subscribe();
        loop {
            if cancel.is_cancelled() {
                return Err(LoopStatus::Cancelled);
            }
            match control.state() {
                AgentControlState::Running => return Ok(()),
                AgentControlState::Cancelled => return Err(LoopStatus::Cancelled),
                AgentControlState::Completed | AgentControlState::Failed => {
                    return Err(LoopStatus::Failed)
                }
                AgentControlState::Paused | AgentControlState::WaitingApproval => {}
            }
            let remaining = budget
                .remaining_runtime()
                .map_err(|_| LoopStatus::BudgetExceeded)?;
            tokio::select! {
                _ = cancel.cancelled() => return Err(LoopStatus::Cancelled),
                _ = tokio::time::sleep(remaining) => return Err(LoopStatus::BudgetExceeded),
                _ = changes.changed() => {}
            }
        }
    }

    async fn emit(&self, draft: AgentEventDraft) {
        if let Some(events) = &self.events {
            let _ = events.publish(draft).await;
        }
    }
}

fn budget_report(
    run_id: String,
    outputs: BTreeMap<String, InvocationResult>,
    attempts: BTreeMap<String, u32>,
    mut decisions: Vec<LoopDecision>,
    error: BudgetExceeded,
) -> LoopReport {
    decisions.push(LoopDecision::Fail {
        reason: error.to_string(),
    });
    LoopReport::terminal(
        run_id,
        LoopStatus::BudgetExceeded,
        outputs,
        attempts,
        decisions,
        Some(error.to_string()),
    )
}

fn stopped_report(
    run_id: String,
    status: LoopStatus,
    outputs: BTreeMap<String, InvocationResult>,
    attempts: BTreeMap<String, u32>,
    decisions: Vec<LoopDecision>,
) -> LoopReport {
    let error = match status {
        LoopStatus::BudgetExceeded => "runtime budget exhausted while awaiting approval or resume",
        LoopStatus::Cancelled => "cancelled",
        _ => "execution control ended",
    };
    LoopReport::terminal(
        run_id,
        status,
        outputs,
        attempts,
        decisions,
        Some(error.into()),
    )
}
