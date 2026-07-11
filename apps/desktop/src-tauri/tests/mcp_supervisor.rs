#![allow(dead_code)]

#[path = "../src/mcp_supervisor.rs"]
mod mcp_supervisor;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use lumo_actions::mcp::oauth::{
    OAuthClientMetadata, OAuthError, OAuthTokenResponse, OAuthTransport,
};
use lumo_agent::{
    CircuitBreakerConfig, McpPublisherMetadata, McpRelease, McpSignatureMetadata,
    McpSignatureStatus, McpToolDefinition, RateLimitPolicy,
};
use mcp_supervisor::{
    complete_oauth, refresh_oauth, McpHealthSupervisor, McpOAuthState, McpSchemaSupervisor,
    OAuthBrowser, OAuthTokenVault, SupervisorError,
};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
struct FakeBrowser(Mutex<Vec<String>>);

impl OAuthBrowser for FakeBrowser {
    fn open(&self, url: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(url.into());
        Ok(())
    }
}

#[derive(Default)]
struct FakeVault(Mutex<BTreeMap<String, String>>);

impl OAuthTokenVault for FakeVault {
    fn put(&self, reference: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(reference.into(), value.into());
        Ok(())
    }

    fn get(&self, reference: &str) -> Result<String, String> {
        self.0
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or_else(|| format!("missing {reference}"))
    }
}

struct FakeTransport {
    exchange: OAuthTokenResponse,
    refresh: OAuthTokenResponse,
    refresh_inputs: Mutex<Vec<String>>,
}

#[async_trait]
impl OAuthTransport for FakeTransport {
    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<OAuthTokenResponse, OAuthError> {
        assert_eq!(code, "auth-code");
        assert!(verifier.len() >= 16);
        Ok(self.exchange.clone())
    }

    async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokenResponse, OAuthError> {
        self.refresh_inputs
            .lock()
            .unwrap()
            .push(refresh_token.into());
        Ok(self.refresh.clone())
    }
}

fn metadata() -> OAuthClientMetadata {
    OAuthClientMetadata {
        authorization_endpoint: "https://auth.example.test/authorize".into(),
        token_endpoint: "https://auth.example.test/token".into(),
        client_id: "lumo-desktop".into(),
        redirect_uri: "http://127.0.0.1/oauth/callback".into(),
        scopes: BTreeSet::from(["tools:call".into()]),
    }
}

fn token(access: &str, refresh: Option<&str>) -> OAuthTokenResponse {
    OAuthTokenResponse {
        access_token: access.into(),
        refresh_token: refresh.map(str::to_string),
        expires_in_secs: 3600,
        scopes: BTreeSet::from(["tools:call".into()]),
    }
}

#[tokio::test]
async fn oauth_start_callback_and_refresh_keep_plaintext_only_in_vault() {
    let browser = FakeBrowser::default();
    let mut state = McpOAuthState::default();
    let started = state
        .start(
            "github",
            metadata(),
            "state-123456".into(),
            "verifier-1234567890".into(),
            &browser,
        )
        .unwrap();
    assert!(started.authorization_url.contains("code_challenge="));
    assert_eq!(browser.0.lock().unwrap().len(), 1);

    let transport = FakeTransport {
        exchange: token("access-one", Some("refresh-one")),
        refresh: token("access-two", Some("refresh-two")),
        refresh_inputs: Mutex::new(Vec::new()),
    };
    let vault = FakeVault::default();
    let session = state.pending("github").unwrap();
    let now = Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let stored = complete_oauth(
        &transport,
        &session,
        "state-123456",
        "auth-code",
        &vault,
        "github",
        now,
    )
    .await
    .unwrap();
    state.complete("github", &session.state).unwrap();

    assert_eq!(stored.access_token_vault_ref, "mcp.oauth.github.access_token");
    assert_eq!(
        stored.refresh_token_vault_ref.as_deref(),
        Some("mcp.oauth.github.refresh_token")
    );
    let encoded = serde_json::to_string(&stored).unwrap();
    assert!(!encoded.contains("access-one"));
    assert_eq!(
        vault.get("mcp.oauth.github.access_token").unwrap(),
        "access-one"
    );

    let refreshed = refresh_oauth(&transport, &stored, &vault, "github", now)
        .await
        .unwrap();
    assert_eq!(
        transport.refresh_inputs.lock().unwrap().as_slice(),
        ["refresh-one"]
    );
    assert_eq!(
        vault.get("mcp.oauth.github.access_token").unwrap(),
        "access-two"
    );
    assert_eq!(
        refreshed.refresh_token_vault_ref.as_deref(),
        Some("mcp.oauth.github.refresh_token")
    );
}

#[tokio::test]
async fn oauth_state_mismatch_never_writes_vault() {
    let browser = FakeBrowser::default();
    let mut state = McpOAuthState::default();
    state
        .start(
            "github",
            metadata(),
            "state-123456".into(),
            "verifier-1234567890".into(),
            &browser,
        )
        .unwrap();
    let transport = FakeTransport {
        exchange: token("access", Some("refresh")),
        refresh: token("new", Some("new-refresh")),
        refresh_inputs: Mutex::new(Vec::new()),
    };
    let vault = FakeVault::default();
    let error = complete_oauth(
        &transport,
        &state.pending("github").unwrap(),
        "attacker-state",
        "auth-code",
        &vault,
        "github",
        Utc::now(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, SupervisorError::OAuth(OAuthError::StateMismatch)));
    assert!(vault.0.lock().unwrap().is_empty());
}

fn release(version: &str, schema: serde_json::Value) -> McpRelease {
    McpRelease {
        server_id: "github".into(),
        version: version.into(),
        publisher: McpPublisherMetadata {
            id: "com.example.github".into(),
            display_name: "GitHub MCP".into(),
            website: Some("https://example.test".into()),
            signature: Some(McpSignatureMetadata {
                algorithm: "ed25519".into(),
                key_id: "release-key".into(),
                digest: "ab".repeat(32),
                status: McpSignatureStatus::Verified,
            }),
        },
        tools: vec![McpToolDefinition {
            name: "issues.list".into(),
            description: "List issues".into(),
            input_schema: schema,
        }],
    }
}

#[test]
fn schema_drift_emits_pending_event_and_exact_approval_changes_visibility() {
    let mut supervisor = McpSchemaSupervisor::default();
    let event = supervisor
        .discover(release("1.0.0", json!({"type": "object"})))
        .unwrap();
    assert_eq!(event.changes.len(), 1);
    assert!(!event.changes[0].visible);
    assert!(supervisor.registry().visible_tools("github").is_empty());

    let pending = supervisor
        .registry()
        .pending_tool("github", "issues.list")
        .unwrap();
    supervisor
        .approve(
            "github",
            "issues.list",
            "1.0.0",
            &pending.schema_hash,
        )
        .unwrap();
    assert_eq!(supervisor.registry().visible_tools("github").len(), 1);

    let event = supervisor
        .discover(release(
            "2.0.0",
            json!({"type":"object", "required":["state"]}),
        ))
        .unwrap();
    assert_eq!(event.changes.len(), 1);
    assert!(!event.changes[0].visible);
    assert_eq!(
        supervisor
            .registry()
            .visible_tool("github", "issues.list")
            .unwrap()
            .release_version,
        "1.0.0"
    );
}

#[test]
fn health_supervisor_rate_limits_and_recovers_through_half_open_probe() {
    let start = Instant::now();
    let mut supervisor = McpHealthSupervisor::new(
        RateLimitPolicy {
            max_requests: 2,
            window: Duration::from_secs(10),
        },
        CircuitBreakerConfig {
            failure_threshold: 2,
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(400),
        },
        start,
    );
    supervisor.before_call(start).unwrap();
    supervisor.record_failure(start);
    supervisor.before_call(start).unwrap();
    supervisor.record_failure(start);
    assert!(matches!(
        supervisor.before_call(start + Duration::from_millis(50)),
        Err(SupervisorError::Circuit(_))
    ));

    let probe = supervisor
        .before_call(start + Duration::from_secs(10))
        .unwrap();
    assert!(probe.is_half_open_probe());
    supervisor.record_success();
    assert!(supervisor
        .before_call(start + Duration::from_secs(20))
        .is_ok());
}
