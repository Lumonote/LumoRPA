use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelemetryPolicy {
    pub enabled: bool,
    pub max_events: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub run_id: String,
    pub kind: String,
    pub payload: Value,
}

impl DiagnosticEvent {
    pub fn new(run_id: impl Into<String>, kind: impl Into<String>, payload: Value) -> Self {
        Self { run_id: run_id.into(), kind: kind.into(), payload: redact(payload, "") }
    }
}

pub struct TelemetryBuffer {
    policy: TelemetryPolicy,
    events: VecDeque<DiagnosticEvent>,
    dropped: u64,
}

impl TelemetryBuffer {
    pub fn new(policy: TelemetryPolicy) -> Self {
        Self { policy, events: VecDeque::with_capacity(policy.max_events.min(4096)), dropped: 0 }
    }

    pub fn push(&mut self, mut event: DiagnosticEvent) {
        if !self.policy.enabled || self.policy.max_events == 0 { return; }
        event.payload = redact(event.payload, "");
        if self.events.len() == self.policy.max_events { self.events.pop_front(); self.dropped += 1; }
        self.events.push_back(event);
    }

    pub fn events(&self) -> &VecDeque<DiagnosticEvent> { &self.events }
    pub fn dropped(&self) -> u64 { self.dropped }
}

fn redact(value: Value, key: &str) -> Value {
    let normalized = key.to_ascii_lowercase();
    if ["token", "secret", "password", "authorization", "cookie", "apikey", "api_key", "audio", "rawaudio", "raw_audio"].iter().any(|candidate| normalized.contains(candidate)) {
        return Value::String("••••••••".into());
    }
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(|value| redact(value, key)).collect()),
        Value::Object(values) => Value::Object(values.into_iter().map(|(child_key, value)| { let redacted = redact(value, &child_key); (child_key, redacted) }).collect()),
        value => value,
    }
}
