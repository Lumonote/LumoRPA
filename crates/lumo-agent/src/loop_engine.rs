use std::{collections::{BTreeMap, BTreeSet}, sync::Arc, time::Duration};

use async_trait::async_trait;
use lumo_core::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::{sync::Mutex, task::JoinSet};

use crate::{
    validate_invocation_result, AdapterRegistry, AgentBudget, AgentEventDraft, AgentEventKind,
    AgentPlan, AgentProfile, BudgetExceeded, CapabilityCatalog, EventSink, InvocationContext,
    InvocationError, InvocationRequest, InvocationResult, PlanValidator,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum LoopDecision {
    Complete,
    Retry { node_id: String },
    Replace { node_id: String, capability_id: String },
    Replan { reason: String },
    AskUser { question: String },
    Fail { reason: String },
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
        Self { run_id, status, outputs, attempts, decisions, error: error.into() }
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
}

impl LoopEngine {
    pub fn new(
        catalog: CapabilityCatalog,
        profile: AgentProfile,
        adapters: AdapterRegistry,
    ) -> Self {
        Self { catalog, profile, adapters, events: None }
    }

    pub fn with_event_publisher(mut self, events: Arc<dyn AgentEventPublisher>) -> Self {
        self.events = Some(events);
        self
    }

    pub async fn execute(
        &self,
        plan: AgentPlan,
        budget: AgentBudget,
        cancel: CancelToken,
    ) -> LoopReport {
        let run_id = plan.id.clone();
        let validated = match PlanValidator::new(&self.catalog, &self.profile).validate(plan) {
            Ok(validated) => validated,
            Err(error) => {
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Failed,
                    BTreeMap::new(),
                    BTreeMap::new(),
                    vec![LoopDecision::Fail { reason: error.to_string() }],
                    Some(error.to_string()),
                );
            }
        };
        if !validated.approval_required.is_empty() {
            return LoopReport::terminal(
                run_id,
                LoopStatus::WaitingApproval,
                BTreeMap::new(),
                BTreeMap::new(),
                vec![LoopDecision::AskUser {
                    question: format!("Approve nodes {:?}?", validated.approval_required),
                }],
                None,
            );
        }
        let plan = validated.plan;
        let nodes = plan
            .nodes
            .iter()
            .map(|node| (node.id.clone(), node.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut pending = nodes.keys().cloned().collect::<BTreeSet<_>>();
        let mut completed = BTreeSet::new();
        let mut outputs = BTreeMap::new();
        let mut attempts = BTreeMap::new();
        let mut decisions = Vec::new();
        let budget = Arc::new(Mutex::new(budget));
        let max_parallel = self.profile.max_parallel.clamp(1, 4) as usize;
        let mut running = JoinSet::new();

        self.emit(AgentEventDraft::new(&run_id, AgentEventKind::RunStarted)).await;
        for node in nodes.values() {
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
                self.emit(AgentEventDraft::new(&run_id, AgentEventKind::RunCancelled)).await;
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
                self.emit(AgentEventDraft::new(&run_id, AgentEventKind::RunCompleted)).await;
                return LoopReport::terminal(
                    run_id,
                    LoopStatus::Completed,
                    outputs,
                    attempts,
                    decisions,
                    None,
                );
            }

            while running.len() < max_parallel {
                let ready = pending.iter().find(|id| {
                    nodes[*id]
                        .depends_on
                        .iter()
                        .all(|dependency| completed.contains(dependency))
                }).cloned();
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
                    .map_err(|_| InvocationError::Timeout { duration_ms: node.timeout_ms })
                    .and_then(|result| result);
                    (node, attempt_number, result)
                });
            }

            if running.is_empty() {
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
            }) else { continue };
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
                Err(error) if attempt <= node.retry_limit => {
                    decisions.push(LoopDecision::Retry { node_id: node.id.clone() });
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
                    decisions.push(LoopDecision::Fail { reason: error.to_string() });
                    self.emit(
                        AgentEventDraft::new(&run_id, AgentEventKind::NodeFailed)
                            .node_id(&node.id)
                            .payload(json!({"attempt": attempt, "error": error.to_string()})),
                    )
                    .await;
                    self.emit(AgentEventDraft::new(&run_id, AgentEventKind::RunFailed)).await;
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
    decisions.push(LoopDecision::Fail { reason: error.to_string() });
    LoopReport::terminal(
        run_id,
        LoopStatus::BudgetExceeded,
        outputs,
        attempts,
        decisions,
        Some(error.to_string()),
    )
}
