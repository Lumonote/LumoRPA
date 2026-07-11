use std::collections::BTreeSet;

use chrono::{Duration, TimeZone, Utc};
use lumo_agent::{
    assess_auto_rollback, ApplyError, ApprovalRecord, EffectPolicy, ImprovementProposal,
    ImprovementTarget, ReplayDataset, ReplaySample, ShadowApprovalGate, ShadowEvalError,
    ShadowEvaluation, ShadowExecutionMode, ShadowResult, ShadowThresholds, VersionedArtifact,
};
use serde_json::json;

fn permissions(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn result(
    quality: u32,
    success: bool,
    latency_ms: u64,
    cost: u64,
    permissions: &[&str],
) -> ShadowResult {
    ShadowResult {
        quality_score_milli: quality,
        success,
        latency_ms,
        cost_usd_micro: cost,
        permissions: self::permissions(permissions),
        external_effects_executed: 0,
    }
}

fn dataset() -> ReplayDataset {
    ReplayDataset::new(
        "orders-replay",
        vec![
            ReplaySample::new(
                "sample-1",
                json!({"utterance": "查订单"}),
                result(700, true, 100, 10, &["orders:read"]),
            ),
            ReplaySample::new(
                "sample-2",
                json!({"utterance": "订单状态"}),
                result(600, false, 120, 12, &["orders:read"]),
            ),
        ],
    )
    .unwrap()
}

fn proposal(id: &str, base: &str) -> ImprovementProposal {
    ImprovementProposal::trace_proposal(
        id,
        vec!["run-1".into()],
        ImprovementTarget::PromptTemplate {
            template_id: "planner".into(),
        },
        json!({"text": format!("candidate-{id}")}),
        base,
    )
    .unwrap()
}

#[test]
fn replay_dataset_rejects_empty_or_duplicate_samples() {
    assert!(matches!(
        ReplayDataset::new("empty", vec![]),
        Err(ShadowEvalError::EmptyDataset)
    ));
    let sample = ReplaySample::new("same", json!({}), result(500, true, 1, 1, &[]));
    assert!(matches!(
        ReplayDataset::new("duplicates", vec![sample.clone(), sample]),
        Err(ShadowEvalError::DuplicateSample(_))
    ));
}

#[test]
fn candidate_requests_are_shadow_only_and_suppress_external_effects() {
    let evaluation = ShadowEvaluation::new("proposal-1", "active-v1", dataset());
    let request = evaluation.candidate_request("sample-1").unwrap();

    assert_eq!(request.mode, ShadowExecutionMode::ShadowOnly);
    assert_eq!(request.effect_policy, EffectPolicy::SuppressExternal);
    assert_eq!(evaluation.active_routing_hash(), "active-v1");
    assert_eq!(request.input, json!({"utterance": "查订单"}));
}

#[test]
fn candidate_results_that_executed_external_effects_are_rejected() {
    let mut evaluation = ShadowEvaluation::new("proposal-1", "active-v1", dataset());
    let mut unsafe_result = result(800, true, 80, 8, &["orders:read"]);
    unsafe_result.external_effects_executed = 1;

    assert!(matches!(
        evaluation.record_candidate("sample-1", unsafe_result),
        Err(ShadowEvalError::ExternalEffectExecuted(_))
    ));
    assert_eq!(evaluation.recorded_samples(), 0);
}

#[test]
fn comparison_requires_minimum_samples_and_zero_permission_expansion() {
    let mut evaluation = ShadowEvaluation::new("proposal-1", "active-v1", dataset());
    evaluation
        .record_candidate("sample-1", result(800, true, 80, 8, &["orders:read"]))
        .unwrap();
    let thresholds = ShadowThresholds {
        minimum_samples: 2,
        ..ShadowThresholds::default()
    };
    let incomplete = evaluation.compare(&thresholds);
    assert!(!incomplete.approval_available);
    assert!(incomplete
        .failures
        .iter()
        .any(|reason| reason.contains("minimum sample")));

    evaluation
        .record_candidate(
            "sample-2",
            result(750, true, 90, 9, &["orders:read", "orders:write"]),
        )
        .unwrap();
    let expanded = evaluation.compare(&thresholds);
    assert!(!expanded.approval_available);
    assert_eq!(
        expanded.permission_delta.added,
        permissions(&["orders:write"])
    );
}

#[test]
fn control_candidate_metrics_and_regression_thresholds_gate_approval() {
    let mut evaluation = ShadowEvaluation::new("proposal-1", "active-v1", dataset());
    evaluation
        .record_candidate("sample-1", result(800, true, 80, 8, &["orders:read"]))
        .unwrap();
    evaluation
        .record_candidate("sample-2", result(750, true, 90, 9, &["orders:read"]))
        .unwrap();

    let report = evaluation.compare(&ShadowThresholds {
        minimum_samples: 2,
        max_quality_drop_milli: 0,
        max_success_rate_drop_milli: 0,
        max_latency_increase_ms: 10,
        max_cost_increase_usd_micro: 5,
    });
    assert!(report.approval_available);
    assert_eq!(report.control.attempts, 2);
    assert_eq!(report.candidate.successes, 2);
    assert!(report.quality_delta_milli > 0);

    let regressed = assess_auto_rollback(
        &report.control,
        &lumo_agent::EvaluationMetrics {
            quality_score_milli: 400,
            successes: 0,
            attempts: 2,
            latency_ms: 500,
            cost_usd_micro: 100,
            permissions: permissions(&["orders:read"]),
        },
        &ShadowThresholds {
            minimum_samples: 2,
            ..ShadowThresholds::default()
        },
    );
    assert!(regressed.triggered);
    assert!(!regressed.reasons.is_empty());
}

#[test]
fn conflicting_or_expired_proposals_cannot_be_applied() {
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let mut artifact = VersionedArtifact::new(json!({"text": "control"}));
    let first = proposal("proposal-1", artifact.active_hash());
    let second = proposal("proposal-2", artifact.active_hash());
    let approval = ApprovalRecord::matching(&first, "alice");

    let conflicted = ShadowApprovalGate::eligible(
        &first.id,
        now + Duration::hours(1),
        BTreeSet::from([second.id.clone()]),
    );
    assert!(matches!(
        artifact.apply_with_gate(&first, &approval, "alice", &conflicted, now),
        Err(ApplyError::ConflictingProposals(_))
    ));

    let expired =
        ShadowApprovalGate::eligible(&first.id, now - Duration::seconds(1), BTreeSet::new());
    assert!(matches!(
        artifact.apply_with_gate(&first, &approval, "alice", &expired, now),
        Err(ApplyError::ProposalExpired(_))
    ));
}

#[test]
fn automatic_rollback_records_reason_and_restores_control_version() {
    let now = Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap();
    let mut artifact = VersionedArtifact::new(json!({"text": "control"}));
    let control_hash = artifact.active_hash().to_string();
    let proposal = proposal("proposal-1", &control_hash);
    let approval = ApprovalRecord::matching(&proposal, "alice");
    let gate =
        ShadowApprovalGate::eligible(&proposal.id, now + Duration::hours(1), BTreeSet::new());
    artifact
        .apply_with_gate(&proposal, &approval, "alice", &gate, now)
        .unwrap();

    let trigger = lumo_agent::RollbackTrigger {
        triggered: true,
        reasons: vec!["success rate crossed rollback threshold".into()],
    };
    let restored = artifact
        .auto_rollback(&trigger, now + Duration::minutes(5))
        .unwrap();
    assert_eq!(restored.unwrap().version_hash, control_hash);
    let history = artifact.rollback_history();
    assert_eq!(history.len(), 1);
    assert!(history[0].automatic);
    assert!(history[0].reason.contains("success rate"));
}
