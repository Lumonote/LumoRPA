use std::collections::BTreeSet;

use lumo_agent::{
    evaluate_node, validate_replan, AgentPlan, AgentProfile, AgentProfileDraft, ApprovalSnapshot,
    ApprovalStrength, CapabilityCatalog, CapabilityDescriptor, PermissionDecision, PermissionRule,
    PlanNode, PolicyDecision, ReplanDecision, RiskLevel,
};
use serde_json::json;

fn capability(id: &str, risk: RiskLevel) -> CapabilityDescriptor {
    let mut descriptor = CapabilityDescriptor::mcp("srv", id, json!({"type": "object"}));
    descriptor.id = id.into();
    descriptor.name = id.into();
    descriptor.risk = risk;
    descriptor.refresh_version_hash();
    descriptor
}

fn node(id: &str, capability_id: &str, risk: RiskLevel) -> PlanNode {
    PlanNode {
        id: id.into(),
        depends_on: vec![],
        capability_id: capability_id.into(),
        arguments: json!({}),
        risk,
        timeout_ms: 1_000,
        retry_limit: 0,
        expected_output_schema: None,
    }
}

fn profile(capabilities: &[CapabilityDescriptor]) -> AgentProfile {
    let catalog = CapabilityCatalog::new(capabilities.to_vec()).unwrap();
    AgentProfile::validate(AgentProfileDraft::default(), &catalog).unwrap()
}

#[test]
fn l0_and_l1_follow_profile_rules() {
    let l0 = capability("read", RiskLevel::L0);
    let l1 = capability("local-read", RiskLevel::L1);
    let mut safe = profile(&[l0.clone(), l1.clone()]);

    assert_eq!(evaluate_node(&node("a", "read", RiskLevel::L0), &l0, &safe), PolicyDecision::Allow);
    assert_eq!(evaluate_node(&node("b", "local-read", RiskLevel::L1), &l1, &safe), PolicyDecision::Allow);

    safe.permission_rules.push(PermissionRule {
        capability_pattern: "local-*".into(),
        max_risk: RiskLevel::L1,
        decision: PermissionDecision::Deny,
    });
    assert!(matches!(
        evaluate_node(&node("b", "local-read", RiskLevel::L1), &l1, &safe),
        PolicyDecision::Deny { .. }
    ));
}

#[test]
fn l2_and_l3_always_require_the_right_confirmation_strength() {
    let l2 = capability("write", RiskLevel::L2);
    let l3 = capability("desktop", RiskLevel::L3);
    let mut permissive = profile(&[l2.clone(), l3.clone()]);
    permissive.max_auto_risk = RiskLevel::L3;

    assert_eq!(
        evaluate_node(&node("a", "write", RiskLevel::L2), &l2, &permissive),
        PolicyDecision::RequireApproval {
            strength: ApprovalStrength::Standard,
            reason: "L2 capabilities require confirmation".into(),
        }
    );
    assert_eq!(
        evaluate_node(&node("b", "desktop", RiskLevel::L3), &l3, &permissive),
        PolicyDecision::RequireApproval {
            strength: ApprovalStrength::Strengthened,
            reason: "L3 capabilities require strengthened confirmation".into(),
        }
    );
}

#[test]
fn risk_cannot_be_understated_or_lowered_during_replan() {
    let dangerous = capability("delete", RiskLevel::L3);
    let safe = profile(std::slice::from_ref(&dangerous));
    assert!(matches!(
        evaluate_node(&node("a", "delete", RiskLevel::L1), &dangerous, &safe),
        PolicyDecision::Deny { .. }
    ));

    let old = AgentPlan::new("p1", "delete", vec![node("a", "delete", RiskLevel::L3)]);
    let mut approved = BTreeSet::new();
    approved.insert("a".into());
    let snapshot = ApprovalSnapshot::capture(&old, &[dangerous], approved);
    let new = AgentPlan::new("p2", "delete", vec![node("a", "replacement", RiskLevel::L1)]);

    assert!(matches!(
        validate_replan(&old, &new, &snapshot),
        ReplanDecision::Reject { .. }
    ));
}

#[test]
fn changed_arguments_require_fresh_approval() {
    let write = capability("write", RiskLevel::L2);
    let old = AgentPlan::new("p1", "write", vec![node("a", "write", RiskLevel::L2)]);
    let mut approved = BTreeSet::new();
    approved.insert("a".into());
    let snapshot = ApprovalSnapshot::capture(&old, &[write], approved);
    let mut changed = node("a", "write", RiskLevel::L2);
    changed.arguments = json!({"path": "/different"});
    let new = AgentPlan::new("p2", "write", vec![changed]);

    assert_eq!(
        validate_replan(&old, &new, &snapshot),
        ReplanDecision::RequiresApproval {
            node_ids: BTreeSet::from(["a".into()])
        }
    );
}
