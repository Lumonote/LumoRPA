use crate::trust::ContentOrigin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub run_id: String,
    pub capability_id: String,
    pub completed: bool,
    pub success: bool,
    pub latency_ms: u64,
    pub cost_usd_micro: u64,
    pub retry_count: u32,
    pub manual_correction: Option<String>,
    pub replacement_capability: Option<String>,
    pub payload: Value,
    pub origin: ContentOrigin,
}

impl TraceRecord {
    pub fn completed_failure(run_id: impl Into<String>, capability_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            capability_id: capability_id.into(),
            completed: true,
            success: false,
            latency_ms: 0,
            cost_usd_micro: 0,
            retry_count: 0,
            manual_correction: None,
            replacement_capability: None,
            payload: Value::Null,
            origin: ContentOrigin::Trace,
        }
    }

    pub fn with_replacement(mut self, replacement: impl Into<String>) -> Self {
        self.replacement_capability = Some(replacement.into());
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceAggregate {
    pub runs: u32,
    pub successes: u32,
    pub retries: u32,
    pub total_latency_ms: u64,
    pub total_cost_usd_micro: u64,
    pub manual_corrections: Vec<String>,
    pub replacements: BTreeMap<String, u32>,
    pub origins: BTreeSet<ContentOrigin>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub source_run_ids: Vec<String>,
    pub by_capability: BTreeMap<String, TraceAggregate>,
}

pub struct TraceMiner;

impl TraceMiner {
    pub fn mine(records: &[TraceRecord]) -> TraceSummary {
        let mut summary = TraceSummary::default();
        for record in records.iter().filter(|record| record.completed) {
            summary.source_run_ids.push(record.run_id.clone());
            let aggregate = summary
                .by_capability
                .entry(record.capability_id.clone())
                .or_default();
            aggregate.runs += 1;
            aggregate.successes += u32::from(record.success);
            aggregate.retries = aggregate.retries.saturating_add(record.retry_count);
            aggregate.total_latency_ms = aggregate
                .total_latency_ms
                .saturating_add(record.latency_ms);
            aggregate.total_cost_usd_micro = aggregate
                .total_cost_usd_micro
                .saturating_add(record.cost_usd_micro);
            aggregate.origins.insert(record.origin);
            if let Some(correction) = &record.manual_correction {
                aggregate.manual_corrections.push(redact_text(correction));
            }
            if let Some(replacement) = &record.replacement_capability {
                *aggregate.replacements.entry(replacement.clone()).or_default() += 1;
            }
        }
        summary.source_run_ids.sort();
        summary.source_run_ids.dedup();
        summary
    }
}

fn redact_text(text: &str) -> String {
    let mut redact_next = false;
    text.split_whitespace()
        .map(|word| {
            if redact_next {
                redact_next = false;
                return "[REDACTED]".to_string();
            }
            if word.eq_ignore_ascii_case("bearer") {
                redact_next = true;
                word.to_string()
            } else if word.starts_with("sk-") || word.starts_with("token=") {
                "[REDACTED]".into()
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
