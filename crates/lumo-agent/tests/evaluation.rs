use lumo_agent::{evaluate_improvement, EvaluationMetrics, ImprovementProposal, ImprovementTarget};
use serde_json::json;
use std::collections::BTreeSet;

fn proposal() -> ImprovementProposal {
    ImprovementProposal::trace_proposal(
        "p1",
        vec!["r1".into()],
        ImprovementTarget::RouterExample {
            capability_id: "orders".into(),
        },
        json!({"example": "orders"}),
        "base-v1",
    )
    .unwrap()
}

fn metrics(
    quality: u32,
    successes: u32,
    attempts: u32,
    latency_ms: u64,
    cost: u64,
    permissions: &[&str],
) -> EvaluationMetrics {
    EvaluationMetrics {
        quality_score_milli: quality,
        successes,
        attempts,
        latency_ms,
        cost_usd_micro: cost,
        permissions: permissions.iter().map(|value| (*value).into()).collect(),
    }
}

#[test]
fn evaluation_reports_quality_success_latency_cost_and_permission_deltas() {
    let baseline = metrics(700, 7, 10, 1_000, 100, &["orders:read"]);
    let candidate = metrics(800, 9, 10, 700, 80, &["orders:read"]);
    let report = evaluate_improvement(&proposal(), &baseline, &candidate);

    assert!(report.passed);
    assert_eq!(report.quality_delta_milli, 100);
    assert_eq!(report.success_rate_delta_milli, 200);
    assert_eq!(report.latency_delta_ms, -300);
    assert_eq!(report.cost_delta_usd_micro, -20);
    assert!(report.permission_delta.added.is_empty());
    assert!(report.permission_delta.removed.is_empty());
}

#[test]
fn permission_expansion_or_metric_regression_fails_evaluation() {
    let baseline = metrics(700, 8, 10, 500, 50, &["orders:read"]);
    let candidate = metrics(
        650,
        7,
        10,
        700,
        90,
        &["orders:read", "vault:read"],
    );
    let report = evaluate_improvement(&proposal(), &baseline, &candidate);

    assert!(!report.passed);
    assert_eq!(
        report.permission_delta.added,
        BTreeSet::from(["vault:read".into()])
    );
    assert!(report.failures.iter().any(|failure| failure.contains("permission")));
}
