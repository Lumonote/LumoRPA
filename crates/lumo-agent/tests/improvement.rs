use lumo_agent::{
    ContentOrigin, ImprovementError, ImprovementProposal, ImprovementTarget, ProposalStatus,
};
use serde_json::json;

#[test]
fn only_explicit_improvement_targets_are_allowed() {
    let allowed = [
        "alias",
        "router_example",
        "prompt_template",
        "flow_patch",
        "skill_patch",
    ];
    for kind in allowed {
        assert!(ImprovementTarget::from_kind(kind, "target-1").is_ok(), "{kind}");
    }

    for forbidden in [
        "vault",
        "approval",
        "risk_floor",
        "system_policy",
        "budget",
        "tool_visibility",
    ] {
        assert!(matches!(
            ImprovementTarget::from_kind(forbidden, "target-1"),
            Err(ImprovementError::ForbiddenTarget(_))
        ));
    }
}

#[test]
fn proposal_rejects_control_plane_patch_keys() {
    for forbidden_key in [
        "vault",
        "approvalState",
        "risk_floor",
        "systemPolicy",
        "budget",
        "visibleTools",
    ] {
        let result = ImprovementProposal::new(
            "proposal-1",
            vec!["run-1".into()],
            ImprovementTarget::Alias {
                capability_id: "orders".into(),
            },
            json!({forbidden_key: "forged"}),
            "safe rationale",
            "base-v1",
            ContentOrigin::Model,
        );
        assert!(matches!(result, Err(ImprovementError::ForbiddenPatchKey(_))));
    }
}

#[test]
fn safe_proposal_is_structured_and_pending() {
    let proposal = ImprovementProposal::new(
        "proposal-1",
        vec!["run-1".into(), "run-2".into()],
        ImprovementTarget::Alias {
            capability_id: "orders".into(),
        },
        json!({"add": ["查订单"]}),
        "Users corrected the route repeatedly",
        "base-v1",
        ContentOrigin::Trace,
    )
    .unwrap();

    assert_eq!(proposal.status, ProposalStatus::Pending);
    assert_eq!(proposal.source_run_ids, ["run-1", "run-2"]);
    assert_eq!(proposal.origin, ContentOrigin::Trace);
    assert_eq!(proposal.patch_hash.len(), 64);
}
