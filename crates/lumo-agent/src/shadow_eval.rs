use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{EvaluationMetrics, PermissionDelta};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadowExecutionMode {
    ShadowOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectPolicy {
    SuppressExternal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowResult {
    pub quality_score_milli: u32,
    pub success: bool,
    pub latency_ms: u64,
    pub cost_usd_micro: u64,
    pub permissions: BTreeSet<String>,
    pub external_effects_executed: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaySample {
    pub id: String,
    pub input: Value,
    pub control: ShadowResult,
}

impl ReplaySample {
    pub fn new(id: impl Into<String>, input: Value, control: ShadowResult) -> Self {
        Self {
            id: id.into(),
            input,
            control,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayDataset {
    pub id: String,
    pub samples: Vec<ReplaySample>,
}

impl ReplayDataset {
    pub fn new(id: impl Into<String>, samples: Vec<ReplaySample>) -> Result<Self, ShadowEvalError> {
        if samples.is_empty() {
            return Err(ShadowEvalError::EmptyDataset);
        }
        let mut ids = BTreeSet::new();
        for sample in &samples {
            if sample.id.trim().is_empty() {
                return Err(ShadowEvalError::EmptySampleId);
            }
            if !ids.insert(sample.id.clone()) {
                return Err(ShadowEvalError::DuplicateSample(sample.id.clone()));
            }
        }
        Ok(Self {
            id: id.into(),
            samples,
        })
    }

    fn sample(&self, id: &str) -> Option<&ReplaySample> {
        self.samples.iter().find(|sample| sample.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowExecutionRequest {
    pub sample_id: String,
    pub input: Value,
    pub mode: ShadowExecutionMode,
    pub effect_policy: EffectPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowThresholds {
    pub minimum_samples: u32,
    pub max_quality_drop_milli: u32,
    pub max_success_rate_drop_milli: u32,
    pub max_latency_increase_ms: u64,
    pub max_cost_increase_usd_micro: u64,
}

impl Default for ShadowThresholds {
    fn default() -> Self {
        Self {
            minimum_samples: 20,
            max_quality_drop_milli: 0,
            max_success_rate_drop_milli: 0,
            max_latency_increase_ms: 0,
            max_cost_increase_usd_micro: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowComparison {
    pub proposal_id: String,
    pub sampled: u32,
    pub approval_available: bool,
    pub control: EvaluationMetrics,
    pub candidate: EvaluationMetrics,
    pub quality_delta_milli: i64,
    pub success_rate_delta_milli: i64,
    pub latency_delta_ms: i64,
    pub cost_delta_usd_micro: i64,
    pub permission_delta: PermissionDelta,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ShadowEvaluation {
    proposal_id: String,
    active_routing_hash: String,
    dataset: ReplayDataset,
    candidate_results: BTreeMap<String, ShadowResult>,
}

impl ShadowEvaluation {
    pub fn new(
        proposal_id: impl Into<String>,
        active_routing_hash: impl Into<String>,
        dataset: ReplayDataset,
    ) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            active_routing_hash: active_routing_hash.into(),
            dataset,
            candidate_results: BTreeMap::new(),
        }
    }

    pub fn active_routing_hash(&self) -> &str {
        &self.active_routing_hash
    }

    pub fn recorded_samples(&self) -> usize {
        self.candidate_results.len()
    }

    pub fn candidate_request(
        &self,
        sample_id: &str,
    ) -> Result<ShadowExecutionRequest, ShadowEvalError> {
        let sample = self
            .dataset
            .sample(sample_id)
            .ok_or_else(|| ShadowEvalError::UnknownSample(sample_id.into()))?;
        Ok(ShadowExecutionRequest {
            sample_id: sample.id.clone(),
            input: sample.input.clone(),
            mode: ShadowExecutionMode::ShadowOnly,
            effect_policy: EffectPolicy::SuppressExternal,
        })
    }

    pub fn record_candidate(
        &mut self,
        sample_id: &str,
        result: ShadowResult,
    ) -> Result<(), ShadowEvalError> {
        if self.dataset.sample(sample_id).is_none() {
            return Err(ShadowEvalError::UnknownSample(sample_id.into()));
        }
        if result.external_effects_executed != 0 {
            return Err(ShadowEvalError::ExternalEffectExecuted(sample_id.into()));
        }
        if self.candidate_results.contains_key(sample_id) {
            return Err(ShadowEvalError::DuplicateResult(sample_id.into()));
        }
        self.candidate_results.insert(sample_id.into(), result);
        Ok(())
    }

    pub fn compare(&self, thresholds: &ShadowThresholds) -> ShadowComparison {
        let paired = self
            .dataset
            .samples
            .iter()
            .filter_map(|sample| {
                self.candidate_results
                    .get(&sample.id)
                    .map(|candidate| (&sample.control, candidate))
            })
            .collect::<Vec<_>>();
        let control = aggregate(paired.iter().map(|(control, _)| *control));
        let candidate = aggregate(paired.iter().map(|(_, candidate)| *candidate));
        compare_metrics(
            self.proposal_id.clone(),
            paired.len() as u32,
            control,
            candidate,
            thresholds,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowApprovalGate {
    pub proposal_id: String,
    pub approval_available: bool,
    pub expires_at: DateTime<Utc>,
    pub conflicting_proposal_ids: BTreeSet<String>,
}

impl ShadowApprovalGate {
    pub fn eligible(
        proposal_id: impl Into<String>,
        expires_at: DateTime<Utc>,
        conflicting_proposal_ids: BTreeSet<String>,
    ) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            approval_available: true,
            expires_at,
            conflicting_proposal_ids,
        }
    }

    pub fn from_comparison(
        comparison: &ShadowComparison,
        expires_at: DateTime<Utc>,
        conflicting_proposal_ids: BTreeSet<String>,
    ) -> Self {
        Self {
            proposal_id: comparison.proposal_id.clone(),
            approval_available: comparison.approval_available,
            expires_at,
            conflicting_proposal_ids,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackTrigger {
    pub triggered: bool,
    pub reasons: Vec<String>,
}

pub fn assess_auto_rollback(
    control: &EvaluationMetrics,
    candidate: &EvaluationMetrics,
    thresholds: &ShadowThresholds,
) -> RollbackTrigger {
    if candidate.attempts < thresholds.minimum_samples {
        return RollbackTrigger {
            triggered: false,
            reasons: Vec::new(),
        };
    }
    let comparison = compare_metrics(
        "active-monitoring".into(),
        candidate.attempts,
        control.clone(),
        candidate.clone(),
        thresholds,
    );
    RollbackTrigger {
        triggered: !comparison.failures.is_empty(),
        reasons: comparison.failures,
    }
}

fn aggregate<'a>(results: impl Iterator<Item = &'a ShadowResult>) -> EvaluationMetrics {
    let results = results.collect::<Vec<_>>();
    let attempts = results.len() as u32;
    if attempts == 0 {
        return EvaluationMetrics {
            quality_score_milli: 0,
            successes: 0,
            attempts: 0,
            latency_ms: 0,
            cost_usd_micro: 0,
            permissions: BTreeSet::new(),
        };
    }
    let divisor = u64::from(attempts);
    EvaluationMetrics {
        quality_score_milli: (results
            .iter()
            .map(|result| u64::from(result.quality_score_milli))
            .sum::<u64>()
            / divisor) as u32,
        successes: results.iter().filter(|result| result.success).count() as u32,
        attempts,
        latency_ms: results.iter().map(|result| result.latency_ms).sum::<u64>() / divisor,
        cost_usd_micro: results
            .iter()
            .map(|result| result.cost_usd_micro)
            .sum::<u64>()
            / divisor,
        permissions: results
            .iter()
            .flat_map(|result| result.permissions.iter().cloned())
            .collect(),
    }
}

fn compare_metrics(
    proposal_id: String,
    sampled: u32,
    control: EvaluationMetrics,
    candidate: EvaluationMetrics,
    thresholds: &ShadowThresholds,
) -> ShadowComparison {
    let quality_delta =
        i64::from(candidate.quality_score_milli) - i64::from(control.quality_score_milli);
    let success_delta = success_rate_milli(&candidate) - success_rate_milli(&control);
    let latency_delta = signed_delta(candidate.latency_ms, control.latency_ms);
    let cost_delta = signed_delta(candidate.cost_usd_micro, control.cost_usd_micro);
    let permission_delta = PermissionDelta {
        added: candidate
            .permissions
            .difference(&control.permissions)
            .cloned()
            .collect(),
        removed: control
            .permissions
            .difference(&candidate.permissions)
            .cloned()
            .collect(),
    };
    let mut failures = Vec::new();
    if sampled < thresholds.minimum_samples {
        failures.push(format!(
            "minimum sample count not met: {sampled}/{}",
            thresholds.minimum_samples
        ));
    }
    if quality_delta < -i64::from(thresholds.max_quality_drop_milli) {
        failures.push("quality crossed rollback threshold".into());
    }
    if success_delta < -i64::from(thresholds.max_success_rate_drop_milli) {
        failures.push("success rate crossed rollback threshold".into());
    }
    if latency_delta > unsigned_threshold(thresholds.max_latency_increase_ms) {
        failures.push("latency crossed rollback threshold".into());
    }
    if cost_delta > unsigned_threshold(thresholds.max_cost_increase_usd_micro) {
        failures.push("cost crossed rollback threshold".into());
    }
    if !permission_delta.added.is_empty() {
        failures.push("undeclared permission expansion is forbidden".into());
    }
    ShadowComparison {
        proposal_id,
        sampled,
        approval_available: failures.is_empty(),
        control,
        candidate,
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

fn signed_delta(candidate: u64, control: u64) -> i64 {
    i128::from(candidate)
        .saturating_sub(i128::from(control))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn unsigned_threshold(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShadowEvalError {
    #[error("replay dataset must contain at least one sample")]
    EmptyDataset,
    #[error("replay sample id must not be empty")]
    EmptySampleId,
    #[error("duplicate replay sample `{0}`")]
    DuplicateSample(String),
    #[error("unknown replay sample `{0}`")]
    UnknownSample(String),
    #[error("candidate result already recorded for `{0}`")]
    DuplicateResult(String),
    #[error("shadow candidate executed an external effect for `{0}`")]
    ExternalEffectExecuted(String),
}
