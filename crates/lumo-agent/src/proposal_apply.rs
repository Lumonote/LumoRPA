use crate::improvement::{hash_json, ImprovementProposal};
use crate::{RollbackTrigger, ShadowApprovalGate};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub proposal_id: String,
    pub patch_hash: String,
    pub base_version_hash: String,
    pub approver: String,
}

impl ApprovalRecord {
    pub fn matching(proposal: &ImprovementProposal, approver: impl Into<String>) -> Self {
        Self {
            proposal_id: proposal.id.clone(),
            patch_hash: proposal.patch_hash.clone(),
            base_version_hash: proposal.base_version_hash.clone(),
            approver: approver.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    pub version_hash: String,
    pub content: Value,
    pub previous_version_hash: Option<String>,
    pub proposal_id: Option<String>,
    pub approver: Option<String>,
}

pub struct VersionedArtifact {
    versions: BTreeMap<String, ArtifactVersion>,
    active: String,
    applied_proposals: BTreeSet<String>,
    rollback_history: Vec<RollbackRecord>,
}

impl VersionedArtifact {
    pub fn new(content: Value) -> Self {
        let version_hash = hash_json(&content);
        let version = ArtifactVersion {
            version_hash: version_hash.clone(),
            content,
            previous_version_hash: None,
            proposal_id: None,
            approver: None,
        };
        Self {
            versions: BTreeMap::from([(version_hash.clone(), version)]),
            active: version_hash,
            applied_proposals: BTreeSet::new(),
            rollback_history: Vec::new(),
        }
    }

    pub fn active_hash(&self) -> &str {
        &self.active
    }

    pub fn active_content(&self) -> &Value {
        &self.versions[&self.active].content
    }

    pub fn apply(
        &mut self,
        proposal: &ImprovementProposal,
        approval: &ApprovalRecord,
        approver: &str,
    ) -> Result<ArtifactVersion, ApplyError> {
        if self.applied_proposals.contains(&proposal.id) {
            return Err(ApplyError::StaleBase {
                expected: proposal.base_version_hash.clone(),
                actual: self.active.clone(),
            });
        }
        validate_approval(proposal, approval, approver)?;
        if self.active != proposal.base_version_hash {
            return Err(ApplyError::StaleBase {
                expected: proposal.base_version_hash.clone(),
                actual: self.active.clone(),
            });
        }
        let mut content = self.active_content().clone();
        merge_patch(&mut content, &proposal.patch);
        let seed = serde_json::json!([self.active, proposal.id, proposal.patch_hash, content]);
        let version_hash = format!("{:x}", Sha256::digest(seed.to_string().as_bytes()));
        let version = ArtifactVersion {
            version_hash: version_hash.clone(),
            content,
            previous_version_hash: Some(self.active.clone()),
            proposal_id: Some(proposal.id.clone()),
            approver: Some(approver.into()),
        };
        self.versions.insert(version_hash.clone(), version.clone());
        self.active = version_hash;
        self.applied_proposals.insert(proposal.id.clone());
        Ok(version)
    }

    pub fn apply_with_gate(
        &mut self,
        proposal: &ImprovementProposal,
        approval: &ApprovalRecord,
        approver: &str,
        gate: &ShadowApprovalGate,
        now: DateTime<Utc>,
    ) -> Result<ArtifactVersion, ApplyError> {
        if gate.proposal_id != proposal.id {
            return Err(ApplyError::ApprovalMismatch(
                "shadow evaluation proposal id".into(),
            ));
        }
        if !gate.approval_available {
            return Err(ApplyError::EvaluationNotEligible(proposal.id.clone()));
        }
        if now >= gate.expires_at {
            return Err(ApplyError::ProposalExpired(proposal.id.clone()));
        }
        if !gate.conflicting_proposal_ids.is_empty() {
            return Err(ApplyError::ConflictingProposals(
                gate.conflicting_proposal_ids.clone(),
            ));
        }
        self.apply(proposal, approval, approver)
    }

    pub fn rollback(&mut self, version_hash: &str) -> Result<ArtifactVersion, ApplyError> {
        self.rollback_with_reason(version_hash, "manual rollback", false, Utc::now())
    }

    pub fn auto_rollback(
        &mut self,
        trigger: &RollbackTrigger,
        at: DateTime<Utc>,
    ) -> Result<Option<ArtifactVersion>, ApplyError> {
        if !trigger.triggered {
            return Ok(None);
        }
        let active = self.active.clone();
        let reason = if trigger.reasons.is_empty() {
            "automatic rollback threshold crossed".into()
        } else {
            trigger.reasons.join("; ")
        };
        self.rollback_with_reason(&active, &reason, true, at)
            .map(Some)
    }

    pub fn rollback_history(&self) -> &[RollbackRecord] {
        &self.rollback_history
    }

    fn rollback_with_reason(
        &mut self,
        version_hash: &str,
        reason: &str,
        automatic: bool,
        at: DateTime<Utc>,
    ) -> Result<ArtifactVersion, ApplyError> {
        if self.active != version_hash {
            return Err(ApplyError::NotActive(version_hash.into()));
        }
        let previous = self.versions[version_hash]
            .previous_version_hash
            .clone()
            .ok_or_else(|| ApplyError::NoRollback(version_hash.into()))?;
        self.active = previous.clone();
        self.rollback_history.push(RollbackRecord {
            from_version_hash: version_hash.into(),
            restored_version_hash: previous.clone(),
            reason: reason.into(),
            automatic,
            created_at: at,
        });
        Ok(self.versions[&previous].clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackRecord {
    pub from_version_hash: String,
    pub restored_version_hash: String,
    pub reason: String,
    pub automatic: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("approval does not match proposal: {0}")]
    ApprovalMismatch(String),
    #[error("stale base version: expected {expected}, active {actual}")]
    StaleBase { expected: String, actual: String },
    #[error("version `{0}` is not active")]
    NotActive(String),
    #[error("version `{0}` has no rollback target")]
    NoRollback(String),
    #[error("proposal `{0}` has not passed shadow evaluation")]
    EvaluationNotEligible(String),
    #[error("proposal `{0}` has expired")]
    ProposalExpired(String),
    #[error("proposal conflicts with active proposals: {0:?}")]
    ConflictingProposals(BTreeSet<String>),
}

fn validate_approval(
    proposal: &ImprovementProposal,
    approval: &ApprovalRecord,
    approver: &str,
) -> Result<(), ApplyError> {
    if approval.proposal_id != proposal.id {
        return Err(ApplyError::ApprovalMismatch("proposal id".into()));
    }
    if approval.patch_hash != proposal.patch_hash {
        return Err(ApplyError::ApprovalMismatch("patch hash".into()));
    }
    if approval.base_version_hash != proposal.base_version_hash {
        return Err(ApplyError::ApprovalMismatch("base version".into()));
    }
    if approver.is_empty() || approval.approver != approver {
        return Err(ApplyError::ApprovalMismatch("approver".into()));
    }
    Ok(())
}

fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Default::default());
    }
    let target = target
        .as_object_mut()
        .expect("target was replaced with object");
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}
