use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CapabilityCatalog, RiskLevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRule {
    pub capability_pattern: String,
    pub max_risk: RiskLevel,
    pub decision: PermissionDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub planner_provider: String,
    pub validator_provider: String,
    pub reflector_provider: String,
    pub max_steps: u32,
    pub max_parallel: u32,
    pub max_runtime_ms: u64,
    pub max_tokens: u64,
    pub max_cost_usd_micro: u64,
    pub visible_capabilities: BTreeSet<String>,
    pub permission_rules: Vec<PermissionRule>,
    pub max_auto_risk: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileDraft {
    pub id: String,
    pub name: String,
    pub planner_provider: String,
    pub validator_provider: String,
    pub reflector_provider: String,
    pub max_steps: u32,
    pub max_parallel: u32,
    pub max_runtime_ms: u64,
    pub max_tokens: u64,
    pub max_cost_usd_micro: u64,
    pub visible_capabilities: Vec<String>,
    pub permission_rules: Vec<PermissionRule>,
    pub max_auto_risk: RiskLevel,
}

impl Default for AgentProfileDraft {
    fn default() -> Self {
        Self {
            id: "safe".into(),
            name: "Safe".into(),
            planner_provider: "default".into(),
            validator_provider: "default".into(),
            reflector_provider: "default".into(),
            max_steps: 20,
            max_parallel: 4,
            max_runtime_ms: 300_000,
            max_tokens: 100_000,
            max_cost_usd_micro: 0,
            visible_capabilities: Vec::new(),
            permission_rules: Vec::new(),
            max_auto_risk: RiskLevel::L1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProfileError {
    #[error("max_steps must be in 1..=100, got {0}")]
    MaxStepsOutOfRange(u32),
    #[error("max_parallel must be in 1..=16, got {0}")]
    MaxParallelOutOfRange(u32),
    #[error("max_runtime_ms must be greater than zero")]
    MaxRuntimeMustBePositive,
    #[error("max_tokens must be greater than zero")]
    MaxTokensMustBePositive,
    #[error("visible capability `{0}` does not exist")]
    UnknownCapability(String),
}

impl AgentProfile {
    pub fn validate(
        draft: AgentProfileDraft,
        catalog: &CapabilityCatalog,
    ) -> Result<Self, ProfileError> {
        if !(1..=100).contains(&draft.max_steps) {
            return Err(ProfileError::MaxStepsOutOfRange(draft.max_steps));
        }
        if !(1..=16).contains(&draft.max_parallel) {
            return Err(ProfileError::MaxParallelOutOfRange(draft.max_parallel));
        }
        if draft.max_runtime_ms == 0 {
            return Err(ProfileError::MaxRuntimeMustBePositive);
        }
        if draft.max_tokens == 0 {
            return Err(ProfileError::MaxTokensMustBePositive);
        }
        if let Some(id) = draft
            .visible_capabilities
            .iter()
            .find(|id| catalog.get(id).is_none())
        {
            return Err(ProfileError::UnknownCapability(id.clone()));
        }

        Ok(Self {
            id: draft.id,
            name: draft.name,
            planner_provider: draft.planner_provider,
            validator_provider: draft.validator_provider,
            reflector_provider: draft.reflector_provider,
            max_steps: draft.max_steps,
            max_parallel: draft.max_parallel,
            max_runtime_ms: draft.max_runtime_ms,
            max_tokens: draft.max_tokens,
            max_cost_usd_micro: draft.max_cost_usd_micro,
            visible_capabilities: draft.visible_capabilities.into_iter().collect(),
            permission_rules: draft.permission_rules,
            max_auto_risk: draft.max_auto_risk,
        })
    }
}

pub fn validate(
    draft: AgentProfileDraft,
    catalog: &CapabilityCatalog,
) -> Result<AgentProfile, ProfileError> {
    AgentProfile::validate(draft, catalog)
}
