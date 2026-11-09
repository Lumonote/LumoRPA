use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use tokio::sync::watch;

use crate::{AgentPlan, ApprovalSnapshot, CapabilityDescriptor, LoopStatus, RiskLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentControlState {
    Running,
    WaitingApproval,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Default)]
struct ControlData {
    paused: bool,
    terminal: Option<AgentControlState>,
    required: BTreeMap<String, RiskLevel>,
    approval: Option<ApprovalSnapshot>,
}

/// Shared execution controls. Pausing stops new node dispatch; already-started
/// operations may finish. Approval is bound to the validated, immutable plan.
#[derive(Clone)]
pub struct AgentRunControl {
    data: Arc<Mutex<ControlData>>,
    changes: watch::Sender<u64>,
}

impl Default for AgentRunControl {
    fn default() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            data: Arc::new(Mutex::new(ControlData::default())),
            changes,
        }
    }
}

impl AgentRunControl {
    pub fn prepare(
        &self,
        plan: &AgentPlan,
        capabilities: &[CapabilityDescriptor],
        required: BTreeMap<String, RiskLevel>,
    ) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        data.required = required;
        data.approval = Some(ApprovalSnapshot::capture(
            plan,
            capabilities,
            BTreeSet::new(),
        ));
        drop(data);
        self.changed();
    }

    pub fn state(&self) -> AgentControlState {
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = data.terminal {
            return state;
        }
        if data.paused {
            return AgentControlState::Paused;
        }
        if data.required.keys().any(|id| {
            !data
                .approval
                .as_ref()
                .is_some_and(|approval| approval.approved_nodes.contains(id))
        }) {
            return AgentControlState::WaitingApproval;
        }
        AgentControlState::Running
    }

    pub fn pause(&self) -> Result<(), String> {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if data.terminal.is_some() || data.paused {
            return Err("agent run is not running".into());
        }
        data.paused = true;
        drop(data);
        self.changed();
        Ok(())
    }

    pub fn resume(&self) -> Result<(), String> {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if data.terminal.is_some() || !data.paused {
            return Err("agent run is not paused".into());
        }
        data.paused = false;
        drop(data);
        self.changed();
        Ok(())
    }

    pub fn validate_approval(
        &self,
        plan_hash: &str,
        node_ids: &[String],
        approved: bool,
        strengthened: bool,
    ) -> Result<(), String> {
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        validate_approval(&data, plan_hash, node_ids, approved, strengthened)
    }

    pub fn resolve_approval(
        &self,
        plan_hash: &str,
        node_ids: &[String],
        approved: bool,
        strengthened: bool,
    ) -> Result<(), String> {
        self.resolve_approval_persisted(plan_hash, node_ids, approved, strengthened, |_| Ok(()))
            .map(|_| ())
    }

    /// Persist the decision before waking execution. The callback is synchronous
    /// and runs under the control lock so terminal transitions and concurrent
    /// decisions cannot invalidate an already-recorded approval.
    pub fn resolve_approval_persisted<F>(
        &self,
        plan_hash: &str,
        node_ids: &[String],
        approved: bool,
        strengthened: bool,
        persist: F,
    ) -> Result<ApprovalSnapshot, String>
    where
        F: FnOnce(&ApprovalSnapshot) -> Result<(), String>,
    {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        validate_approval(&data, plan_hash, node_ids, approved, strengthened)?;
        let mut snapshot = data.approval.clone().expect("validated snapshot");
        if approved {
            snapshot.approved_nodes.extend(node_ids.iter().cloned());
            snapshot.approved_at = chrono::Utc::now();
        }
        persist(&snapshot)?;
        data.approval = Some(snapshot.clone());
        if !approved {
            data.terminal = Some(AgentControlState::Cancelled);
        }
        drop(data);
        self.changed();
        Ok(snapshot)
    }

    pub fn approval_snapshot(&self) -> Option<ApprovalSnapshot> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .approval
            .clone()
    }

    pub fn finish(&self, status: LoopStatus) {
        let terminal = match status {
            LoopStatus::Completed => AgentControlState::Completed,
            LoopStatus::Failed | LoopStatus::BudgetExceeded => AgentControlState::Failed,
            LoopStatus::Cancelled => AgentControlState::Cancelled,
            LoopStatus::WaitingApproval => return,
        };
        self.data.lock().unwrap_or_else(|e| e.into_inner()).terminal = Some(terminal);
        self.changed();
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    fn changed(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
}

fn validate_approval(
    data: &ControlData,
    plan_hash: &str,
    node_ids: &[String],
    approved: bool,
    strengthened: bool,
) -> Result<(), String> {
    if data.terminal.is_some() {
        return Err("agent run has already ended".into());
    }
    let snapshot = data
        .approval
        .as_ref()
        .ok_or("agent run is not waiting for approval")?;
    if snapshot.plan_hash != plan_hash {
        return Err("stale approval: plan hash changed".into());
    }
    if node_ids.is_empty() {
        return Err("approval requires at least one pending node".into());
    }
    for id in node_ids {
        let risk = data
            .required
            .get(id)
            .ok_or_else(|| format!("node `{id}` does not require approval"))?;
        if snapshot.approved_nodes.contains(id) {
            return Err(format!("node `{id}` was already approved"));
        }
        if approved && *risk == RiskLevel::L3 && !strengthened {
            return Err("L3 approval requires strengthened confirmation".into());
        }
    }
    Ok(())
}
