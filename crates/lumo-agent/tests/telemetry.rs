#[path = "../src/telemetry.rs"]
mod telemetry;

use serde_json::json;
use telemetry::{DiagnosticEvent, TelemetryBuffer, TelemetryPolicy};

#[test]
fn telemetry_is_policy_disabled_and_never_retains_secrets_or_audio() {
    let mut disabled = TelemetryBuffer::new(TelemetryPolicy { enabled: false, max_events: 10 });
    disabled.push(DiagnosticEvent::new("r1", "voice", json!({"token":"raw", "audio":[1,2]})));
    assert!(disabled.events().is_empty());

    let mut enabled = TelemetryBuffer::new(TelemetryPolicy { enabled: true, max_events: 10 });
    enabled.push(DiagnosticEvent::new("r1", "agent.node", json!({"apiKey":"raw", "value":42, "rawAudio":"bytes"})));
    let serialized = serde_json::to_string(enabled.events()).unwrap();
    assert!(!serialized.contains(":\"raw\""));
    assert!(serialized.contains("••••••••"));
    assert!(serialized.contains("42"));
}

#[test]
fn telemetry_retention_is_bounded_and_correlated() {
    let mut buffer = TelemetryBuffer::new(TelemetryPolicy { enabled: true, max_events: 2 });
    for index in 0..3 { buffer.push(DiagnosticEvent::new("run-7", format!("event-{index}"), json!({}))); }
    assert_eq!(buffer.events().len(), 2);
    assert_eq!(buffer.dropped(), 1);
    assert!(buffer.events().iter().all(|event| event.run_id == "run-7"));
}
