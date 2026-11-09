use crate::{
    plan::stable_json_hash, AgentPlan, CapabilityCatalog, InvocationResult, PlanNode, RiskLevel,
};
use chrono::Utc;
use lumo_storage::{DurableJobNode, JobNodeCheckpoint, Repo};
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct JobCheckpointStore {
    repo: Repo,
    job_id: String,
    worker_id: String,
}

pub(crate) struct RestoredCheckpoint {
    pub outputs: BTreeMap<String, InvocationResult>,
    pub attempts: BTreeMap<String, u32>,
}

impl JobCheckpointStore {
    pub fn new(repo: Repo, job_id: impl Into<String>, worker_id: impl Into<String>) -> Self {
        Self {
            repo,
            job_id: job_id.into(),
            worker_id: worker_id.into(),
        }
    }

    pub(crate) fn restore(
        &self,
        plan: &AgentPlan,
        catalog: &CapabilityCatalog,
    ) -> Result<RestoredCheckpoint, String> {
        let mut outputs = BTreeMap::new();
        let mut attempts = BTreeMap::new();
        for saved in self
            .repo
            .list_durable_job_nodes(&self.job_id)
            .map_err(|error| error.to_string())?
        {
            let node = plan
                .node(&saved.checkpoint.node_id)
                .ok_or("checkpoint references a removed plan node")?;
            if saved.node_hash != node_hash(plan, node, catalog)? {
                return Err(format!(
                    "checkpoint for `{}` no longer matches its plan or capability",
                    node.id
                ));
            }
            match saved.checkpoint.state.as_str() {
                "completed" => {
                    let result = serde_json::from_value(
                        saved
                            .result
                            .ok_or("completed checkpoint has no durable output")?,
                    )
                    .map_err(|error| format!("invalid durable output: {error}"))?;
                    crate::validate_invocation_result(node, &result)
                        .map_err(|error| format!("invalid durable output: {error}"))?;
                    outputs.insert(node.id.clone(), result);
                }
                "unknown" | "running" if !saved.checkpoint.idempotent => {
                    return Err(format!(
                        "node `{}` requires manual verification before retry",
                        node.id
                    ))
                }
                _ => {}
            }
            attempts.insert(
                node.id.clone(),
                saved
                    .checkpoint
                    .attempt
                    .try_into()
                    .map_err(|_| "checkpoint attempt out of range")?,
            );
        }
        Ok(RestoredCheckpoint { outputs, attempts })
    }

    pub(crate) fn save(
        &self,
        plan: &AgentPlan,
        node: &PlanNode,
        catalog: &CapabilityCatalog,
        attempt: u32,
        result: Option<&InvocationResult>,
    ) -> Result<(), String> {
        self.repo
            .save_durable_job_node(
                &self.worker_id,
                &DurableJobNode {
                    checkpoint: JobNodeCheckpoint {
                        job_id: self.job_id.clone(),
                        node_id: node.id.clone(),
                        state: if result.is_some() {
                            "completed"
                        } else {
                            "running"
                        }
                        .into(),
                        risk: format!("{:?}", node.risk),
                        idempotent: node.risk == RiskLevel::L0,
                        attempt: i64::from(attempt),
                        updated_at: Utc::now(),
                    },
                    node_hash: node_hash(plan, node, catalog)?,
                    result: result
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(|error| error.to_string())?,
                },
            )
            .map_err(|error| error.to_string())
    }
}

fn node_hash(
    plan: &AgentPlan,
    node: &PlanNode,
    catalog: &CapabilityCatalog,
) -> Result<String, String> {
    let capability = catalog
        .get(&node.capability_id)
        .ok_or("checkpoint capability disappeared")?;
    Ok(stable_json_hash(
        &serde_json::json!({"plan":plan,"node":node,"capabilityVersion":capability.version_hash}),
    ))
}
