use crate::trust::ContentOrigin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImprovementTarget {
    Alias { capability_id: String },
    RouterExample { capability_id: String },
    PromptTemplate { template_id: String },
    FlowPatch { flow_id: String },
    SkillPatch { skill_name: String },
}

impl ImprovementTarget {
    pub fn from_kind(kind: &str, id: impl Into<String>) -> Result<Self, ImprovementError> {
        let id = id.into();
        match kind {
            "alias" => Ok(Self::Alias { capability_id: id }),
            "router_example" => Ok(Self::RouterExample { capability_id: id }),
            "prompt_template" => Ok(Self::PromptTemplate { template_id: id }),
            "flow_patch" => Ok(Self::FlowPatch { flow_id: id }),
            "skill_patch" => Ok(Self::SkillPatch { skill_name: id }),
            forbidden => Err(ImprovementError::ForbiddenTarget(forbidden.into())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Evaluated,
    Approved,
    Rejected,
    Applied,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImprovementProposal {
    pub id: String,
    pub source_run_ids: Vec<String>,
    pub target: ImprovementTarget,
    pub patch: Value,
    pub patch_hash: String,
    pub rationale: String,
    pub status: ProposalStatus,
    pub base_version_hash: String,
    pub origin: ContentOrigin,
}

impl ImprovementProposal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        source_run_ids: Vec<String>,
        target: ImprovementTarget,
        patch: Value,
        rationale: impl Into<String>,
        base_version_hash: impl Into<String>,
        origin: ContentOrigin,
    ) -> Result<Self, ImprovementError> {
        validate_patch(&patch)?;
        let id = id.into();
        let base_version_hash = base_version_hash.into();
        if id.trim().is_empty() || base_version_hash.trim().is_empty() || source_run_ids.is_empty() {
            return Err(ImprovementError::InvalidProposal(
                "id, source runs and base version are required".into(),
            ));
        }
        let patch_hash = hash_json(&patch);
        Ok(Self {
            id,
            source_run_ids,
            target,
            patch,
            patch_hash,
            rationale: rationale.into(),
            status: ProposalStatus::Pending,
            base_version_hash,
            origin,
        })
    }

    pub fn trace_proposal(
        id: impl Into<String>,
        source_run_ids: Vec<String>,
        target: ImprovementTarget,
        patch: Value,
        base_version_hash: impl Into<String>,
    ) -> Result<Self, ImprovementError> {
        Self::new(
            id,
            source_run_ids,
            target,
            patch,
            "derived from completed redacted traces",
            base_version_hash,
            ContentOrigin::Trace,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImprovementError {
    #[error("improvement target `{0}` is forbidden")]
    ForbiddenTarget(String),
    #[error("improvement patch key `{0}` is forbidden")]
    ForbiddenPatchKey(String),
    #[error("invalid improvement proposal: {0}")]
    InvalidProposal(String),
}

fn validate_patch(value: &Value) -> Result<(), ImprovementError> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if [
                    "vault",
                    "approval",
                    "approvalstate",
                    "risk",
                    "riskfloor",
                    "systempolicy",
                    "policy",
                    "budget",
                    "visibletools",
                    "toolvisibility",
                    "permission",
                    "permissions",
                ]
                .iter()
                .any(|forbidden| normalized == *forbidden || normalized.starts_with(forbidden))
                {
                    return Err(ImprovementError::ForbiddenPatchKey(key.clone()));
                }
                validate_patch(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_patch(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn hash_json(value: &Value) -> String {
    let canonical = canonicalize(value).to_string();
    format!("{:x}", Sha256::digest(canonical.as_bytes()))
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
        value => value.clone(),
    }
}
