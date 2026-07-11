use crate::improvement::ImprovementProposal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationMetrics {
    pub quality_score_milli: u32,
    pub successes: u32,
    pub attempts: u32,
    pub latency_ms: u64,
    pub cost_usd_micro: u64,
    pub permissions: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionDelta {
    pub added: BTreeSet<String>,
    pub removed: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvaluationReport {
    pub passed: bool,
    pub quality_delta_milli: i64,
    pub success_rate_delta_milli: i64,
    pub latency_delta_ms: i64,
    pub cost_delta_usd_micro: i64,
    pub permission_delta: PermissionDelta,
    pub failures: Vec<String>,
}

pub fn evaluate_improvement(
    _proposal: &ImprovementProposal,
    baseline: &EvaluationMetrics,
    candidate: &EvaluationMetrics,
) -> EvaluationReport {
    let quality_delta = i64::from(candidate.quality_score_milli)
        - i64::from(baseline.quality_score_milli);
    let success_delta = success_rate_milli(candidate) - success_rate_milli(baseline);
    let latency_delta = signed_delta(candidate.latency_ms, baseline.latency_ms);
    let cost_delta = signed_delta(candidate.cost_usd_micro, baseline.cost_usd_micro);
    let permission_delta = PermissionDelta {
        added: candidate
            .permissions
            .difference(&baseline.permissions)
            .cloned()
            .collect(),
        removed: baseline
            .permissions
            .difference(&candidate.permissions)
            .cloned()
            .collect(),
    };
    let mut failures = Vec::new();
    if quality_delta < 0 {
        failures.push("quality regressed".into());
    }
    if success_delta < 0 {
        failures.push("success rate regressed".into());
    }
    if latency_delta > 0 {
        failures.push("latency regressed".into());
    }
    if cost_delta > 0 {
        failures.push("cost regressed".into());
    }
    if !permission_delta.added.is_empty() {
        failures.push("permission expansion is forbidden".into());
    }
    EvaluationReport {
        passed: failures.is_empty(),
        quality_delta_milli: quality_delta,
        success_rate_delta_milli: success_delta,
        latency_delta_ms: latency_delta,
        cost_delta_usd_micro: cost_delta,
        permission_delta,
        failures,
    }
}

fn success_rate_milli(metrics: &EvaluationMetrics) -> i64 {
    if metrics.attempts == 0 {
        0
    } else {
        i64::from(metrics.successes) * 1_000 / i64::from(metrics.attempts)
    }
}

fn signed_delta(candidate: u64, baseline: u64) -> i64 {
    i128::from(candidate)
        .saturating_sub(i128::from(baseline))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}
