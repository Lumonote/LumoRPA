#[path = "../src/mcp_registry.rs"]
mod mcp_registry;

use mcp_registry::{
    CircuitBreaker, CircuitBreakerConfig, CircuitError, CircuitPhase, McpPublisherMetadata,
    McpRegistry, McpRelease, McpSignatureMetadata, McpSignatureStatus, McpToolDefinition,
    RateLimitError, RateLimitPolicy, RateLimiter, RegistryDriftKind, RegistryError,
};
use serde_json::json;
use std::time::{Duration, Instant};

fn publisher() -> McpPublisherMetadata {
    McpPublisherMetadata {
        id: "com.example.github".into(),
        display_name: "Example Publisher".into(),
        website: Some("https://example.test".into()),
        signature: Some(McpSignatureMetadata {
            algorithm: "ed25519".into(),
            key_id: "release-key-2026".into(),
            digest: "ab".repeat(32),
            status: McpSignatureStatus::Verified,
        }),
    }
}

fn tool(name: &str, schema: serde_json::Value) -> McpToolDefinition {
    McpToolDefinition {
        name: name.into(),
        description: format!("{name} description"),
        input_schema: schema,
    }
}

fn release(version: &str, tools: Vec<McpToolDefinition>) -> McpRelease {
    McpRelease {
        server_id: "github".into(),
        version: version.into(),
        publisher: publisher(),
        tools,
    }
}

#[test]
fn publisher_and_signature_metadata_are_preserved() {
    let mut registry = McpRegistry::new();
    registry
        .discover(release(
            "1.0.0",
            vec![tool("issues.list", json!({"type": "object"}))],
        ))
        .unwrap();

    let metadata = registry.latest_publisher("github").unwrap();
    assert_eq!(metadata.id, "com.example.github");
    let signature = metadata.signature.as_ref().unwrap();
    assert_eq!(signature.key_id, "release-key-2026");
    assert_eq!(signature.status, McpSignatureStatus::Verified);
}

#[test]
fn new_tools_are_hidden_until_exact_release_and_schema_are_approved() {
    let mut registry = McpRegistry::new();
    let update = registry
        .discover(release(
            "1.0.0",
            vec![
                tool("issues.list", json!({"type": "object"})),
                tool(
                    "issues.create",
                    json!({"type": "object", "required": ["title"]}),
                ),
            ],
        ))
        .unwrap();

    assert!(registry.visible_tools("github").is_empty());
    assert_eq!(update.drifts.len(), 2);
    assert!(update
        .drifts
        .iter()
        .all(|drift| drift.kind == RegistryDriftKind::NewTool));

    let pending = registry.pending_tool("github", "issues.list").unwrap();
    let wrong = registry
        .approve_tool("github", "issues.list", "1.0.0", "wrong-hash")
        .unwrap_err();
    assert!(matches!(wrong, RegistryError::ApprovalMismatch { .. }));
    registry
        .approve_tool("github", "issues.list", "1.0.0", &pending.schema_hash)
        .unwrap();

    assert_eq!(registry.visible_tools("github").len(), 1);
    assert!(registry.visible_tool("github", "issues.list").is_some());
    assert!(registry.visible_tool("github", "issues.create").is_none());
}

#[test]
fn schema_and_release_version_drift_keep_last_approval_visible() {
    let mut registry = McpRegistry::new();
    registry
        .discover(release(
            "1.0.0",
            vec![tool("issues.list", json!({"type": "object"}))],
        ))
        .unwrap();
    let first = registry
        .pending_tool("github", "issues.list")
        .unwrap()
        .clone();
    registry
        .approve_tool("github", "issues.list", "1.0.0", &first.schema_hash)
        .unwrap();

    let changed_schema = json!({
        "required": ["state"],
        "properties": {"state": {"type": "string"}},
        "type": "object"
    });
    let update = registry
        .discover(release("2.0.0", vec![tool("issues.list", changed_schema)]))
        .unwrap();
    assert_eq!(update.drifts.len(), 1);
    assert_eq!(
        update.drifts[0].kind,
        RegistryDriftKind::SchemaAndVersionChanged
    );

    let visible = registry.visible_tool("github", "issues.list").unwrap();
    assert_eq!(visible.release_version, "1.0.0");
    let pending = registry.pending_tool("github", "issues.list").unwrap();
    assert_eq!(pending.release_version, "2.0.0");
    assert_ne!(visible.schema_hash, pending.schema_hash);

    registry
        .approve_tool(
            "github",
            "issues.list",
            "2.0.0",
            &pending.schema_hash.clone(),
        )
        .unwrap();
    assert_eq!(
        registry
            .visible_tool("github", "issues.list")
            .unwrap()
            .release_version,
        "2.0.0"
    );
}

#[test]
fn fixed_window_rate_limiter_reports_retry_after() {
    let start = Instant::now();
    let mut limiter = RateLimiter::new(
        RateLimitPolicy {
            max_requests: 2,
            window: Duration::from_secs(10),
        },
        start,
    );
    assert!(limiter.acquire(start).is_ok());
    assert!(limiter.acquire(start + Duration::from_secs(1)).is_ok());
    let error = limiter.acquire(start + Duration::from_secs(3)).unwrap_err();
    assert_eq!(
        error,
        RateLimitError::Limited {
            retry_after: Duration::from_secs(7)
        }
    );
    assert!(limiter.acquire(start + Duration::from_secs(10)).is_ok());
}

#[test]
fn circuit_transitions_closed_open_half_open_with_exponential_backoff() {
    let start = Instant::now();
    let mut circuit = CircuitBreaker::new(CircuitBreakerConfig {
        failure_threshold: 2,
        base_backoff: Duration::from_millis(100),
        max_backoff: Duration::from_millis(400),
    });

    assert_eq!(circuit.phase(), CircuitPhase::Closed);
    circuit.before_call(start).unwrap();
    circuit.record_failure(start);
    assert_eq!(circuit.phase(), CircuitPhase::Closed);
    circuit.record_failure(start);
    assert_eq!(circuit.phase(), CircuitPhase::Open);
    assert!(matches!(
        circuit.before_call(start + Duration::from_millis(50)),
        Err(CircuitError::Open { retry_after }) if retry_after == Duration::from_millis(50)
    ));

    let probe = circuit
        .before_call(start + Duration::from_millis(100))
        .unwrap();
    assert!(probe.is_half_open_probe());
    assert_eq!(circuit.phase(), CircuitPhase::HalfOpen);
    assert!(matches!(
        circuit.before_call(start + Duration::from_millis(100)),
        Err(CircuitError::HalfOpenProbeInFlight)
    ));
    circuit.record_failure(start + Duration::from_millis(100));
    assert_eq!(circuit.phase(), CircuitPhase::Open);
    assert!(matches!(
        circuit.before_call(start + Duration::from_millis(299)),
        Err(CircuitError::Open { retry_after }) if retry_after == Duration::from_millis(1)
    ));

    let probe = circuit
        .before_call(start + Duration::from_millis(300))
        .unwrap();
    assert!(probe.is_half_open_probe());
    circuit.record_success();
    assert_eq!(circuit.phase(), CircuitPhase::Closed);
    assert!(circuit
        .before_call(start + Duration::from_millis(301))
        .is_ok());
}
