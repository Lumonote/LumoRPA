use lumo_agent::{
    ApprovalRecord, ApplyError, ImprovementProposal, ImprovementTarget, VersionedArtifact,
};
use serde_json::json;

fn proposal(base: &str) -> ImprovementProposal {
    ImprovementProposal::trace_proposal(
        "proposal-1",
        vec!["run-1".into()],
        ImprovementTarget::PromptTemplate {
            template_id: "planner".into(),
        },
        json!({"text": "new prompt"}),
        base,
    )
    .unwrap()
}

#[test]
fn apply_requires_matching_proposal_hash_base_and_approver() {
    let mut artifact = VersionedArtifact::new(json!({"text": "old prompt"}));
    let proposal = proposal(artifact.active_hash());
    let approval = ApprovalRecord {
        proposal_id: proposal.id.clone(),
        patch_hash: proposal.patch_hash.clone(),
        base_version_hash: proposal.base_version_hash.clone(),
        approver: "alice".into(),
    };

    for bad in [
        ApprovalRecord {
            proposal_id: "other".into(),
            ..approval.clone()
        },
        ApprovalRecord {
            patch_hash: "bad".into(),
            ..approval.clone()
        },
        ApprovalRecord {
            base_version_hash: "stale".into(),
            ..approval.clone()
        },
        ApprovalRecord {
            approver: "mallory".into(),
            ..approval.clone()
        },
    ] {
        assert!(artifact
            .apply(&proposal, &bad, "alice")
            .is_err());
        assert_eq!(artifact.active_content(), &json!({"text": "old prompt"}));
    }
}

#[test]
fn apply_creates_new_version_and_rollback_restores_previous_active_version() {
    let mut artifact = VersionedArtifact::new(json!({"text": "old prompt"}));
    let old_hash = artifact.active_hash().to_string();
    let proposal = proposal(&old_hash);
    let approval = ApprovalRecord::matching(&proposal, "alice");

    let applied = artifact.apply(&proposal, &approval, "alice").unwrap();
    assert_ne!(applied.version_hash, old_hash);
    assert_eq!(applied.previous_version_hash.as_deref(), Some(old_hash.as_str()));
    assert_eq!(artifact.active_content(), &json!({"text": "new prompt"}));

    let restored = artifact.rollback(&applied.version_hash).unwrap();
    assert_eq!(restored.version_hash, old_hash);
    assert_eq!(artifact.active_content(), &json!({"text": "old prompt"}));

    assert!(matches!(
        artifact.apply(&proposal, &approval, "alice"),
        Err(ApplyError::StaleBase { .. })
    ));
}
