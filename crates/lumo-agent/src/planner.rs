use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AgentPlan, AgentProfile, CapabilityCatalog, PlanValidator};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RankedCandidate {
    pub capability_id: String,
    pub score: f64,
}

#[async_trait]
pub trait AiPlanModel: Send + Sync {
    async fn generate(
        &self,
        utterance: &str,
        candidates: &[RankedCandidate],
    ) -> Result<String, String>;
}

#[async_trait]
pub trait PlannerBackend: Send + Sync {
    async fn plan(
        &self,
        utterance: &str,
        candidates: Vec<RankedCandidate>,
    ) -> Result<AgentPlan, String>;
}

pub struct Planner {
    catalog: CapabilityCatalog,
    profile: AgentProfile,
    model: Arc<dyn AiPlanModel>,
}

impl Planner {
    pub fn new(
        catalog: CapabilityCatalog,
        profile: AgentProfile,
        model: Arc<dyn AiPlanModel>,
    ) -> Self {
        Self { catalog, profile, model }
    }

    pub async fn plan(
        &self,
        utterance: &str,
        candidates: Vec<RankedCandidate>,
    ) -> Result<AgentPlan, PlannerError> {
        let raw = self
            .model
            .generate(utterance, &candidates)
            .await
            .map_err(PlannerError::Model)?;
        let plan: AgentPlan =
            serde_json::from_str(&raw).map_err(|error| PlannerError::MalformedPlan(error.to_string()))?;
        if !candidates.is_empty()
            && plan.nodes.iter().any(|node| {
                !candidates
                    .iter()
                    .any(|candidate| candidate.capability_id == node.capability_id)
            })
        {
            return Err(PlannerError::InvalidPlan(
                "model selected a capability outside the filtered candidates".into(),
            ));
        }
        PlanValidator::new(&self.catalog, &self.profile)
            .validate(plan.clone())
            .map_err(|error| PlannerError::InvalidPlan(error.to_string()))?;
        Ok(plan)
    }
}

#[async_trait]
impl PlannerBackend for Planner {
    async fn plan(
        &self,
        utterance: &str,
        candidates: Vec<RankedCandidate>,
    ) -> Result<AgentPlan, String> {
        Planner::plan(self, utterance, candidates)
            .await
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlannerError {
    #[error("planner model failed: {0}")]
    Model(String),
    #[error("planner returned malformed JSON: {0}")]
    MalformedPlan(String),
    #[error("planner returned an invalid plan: {0}")]
    InvalidPlan(String),
}
