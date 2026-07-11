use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use lumo_actions::mcp::oauth::{
    begin_pkce, complete_authorization, refresh_if_needed, OAuthClientMetadata, OAuthError,
    OAuthPkceSession, OAuthTokenResponse, OAuthTransport, StoredOAuthTokenRefs,
};
use lumo_agent::{
    CircuitBreaker, CircuitBreakerConfig, CircuitError, CircuitPermit, McpRegistry, McpRelease,
    RateLimitError, RateLimitPolicy, RateLimiter, RegisteredMcpTool, RegistryDriftKind,
    RegistryError,
};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

pub(crate) trait OAuthBrowser: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

pub(crate) trait OAuthTokenVault: Send + Sync {
    fn put(&self, reference: &str, value: &str) -> Result<(), String>;
    fn get(&self, reference: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpOAuthStart {
    pub server_id: String,
    pub authorization_url: String,
    pub state: String,
}

#[derive(Default)]
pub(crate) struct McpOAuthState {
    pending: HashMap<String, OAuthPkceSession>,
}

impl McpOAuthState {
    pub(crate) fn start(
        &mut self,
        server_id: &str,
        metadata: OAuthClientMetadata,
        state: String,
        verifier: String,
        browser: &dyn OAuthBrowser,
    ) -> Result<McpOAuthStart, SupervisorError> {
        validate_server_id(server_id)?;
        let session = begin_pkce(metadata, state, verifier)?;
        browser
            .open(&session.authorization_url)
            .map_err(SupervisorError::Browser)?;
        let result = McpOAuthStart {
            server_id: server_id.into(),
            authorization_url: session.authorization_url.clone(),
            state: session.state.clone(),
        };
        self.pending.insert(server_id.into(), session);
        Ok(result)
    }

    pub(crate) fn pending(&self, server_id: &str) -> Result<OAuthPkceSession, SupervisorError> {
        self.pending
            .get(server_id)
            .cloned()
            .ok_or_else(|| SupervisorError::PendingOAuthMissing(server_id.into()))
    }

    pub(crate) fn complete(
        &mut self,
        server_id: &str,
        expected_state: &str,
    ) -> Result<(), SupervisorError> {
        let session = self
            .pending
            .get(server_id)
            .ok_or_else(|| SupervisorError::PendingOAuthMissing(server_id.into()))?;
        if session.state != expected_state {
            return Err(SupervisorError::OAuth(OAuthError::StateMismatch));
        }
        self.pending.remove(server_id);
        Ok(())
    }
}

pub(crate) async fn complete_oauth(
    transport: &dyn OAuthTransport,
    session: &OAuthPkceSession,
    returned_state: &str,
    code: &str,
    vault: &dyn OAuthTokenVault,
    server_id: &str,
    now: DateTime<Utc>,
) -> Result<StoredOAuthTokenRefs, SupervisorError> {
    validate_server_id(server_id)?;
    let token = complete_authorization(transport, session, returned_state, code).await?;
    store_token(vault, server_id, token, now)
}

pub(crate) async fn refresh_oauth(
    transport: &dyn OAuthTransport,
    stored: &StoredOAuthTokenRefs,
    vault: &dyn OAuthTokenVault,
    server_id: &str,
    now: DateTime<Utc>,
) -> Result<StoredOAuthTokenRefs, SupervisorError> {
    validate_server_id(server_id)?;
    let access_token = vault
        .get(&stored.access_token_vault_ref)
        .map_err(SupervisorError::Vault)?;
    let refresh_token = stored
        .refresh_token_vault_ref
        .as_deref()
        .map(|reference| vault.get(reference).map_err(SupervisorError::Vault))
        .transpose()?;
    let previous_refresh = refresh_token.clone();
    let current = OAuthTokenResponse {
        access_token,
        refresh_token,
        expires_in_secs: 0,
        scopes: stored.scopes.clone(),
    };
    let mut refreshed = refresh_if_needed(transport, current, true).await?;
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = previous_refresh;
    }
    store_token(vault, server_id, refreshed, now)
}

fn store_token(
    vault: &dyn OAuthTokenVault,
    server_id: &str,
    token: OAuthTokenResponse,
    now: DateTime<Utc>,
) -> Result<StoredOAuthTokenRefs, SupervisorError> {
    let access_ref = format!("mcp.oauth.{server_id}.access_token");
    let refresh_ref = token
        .refresh_token
        .as_ref()
        .map(|_| format!("mcp.oauth.{server_id}.refresh_token"));
    vault
        .put(&access_ref, &token.access_token)
        .map_err(SupervisorError::Vault)?;
    if let (Some(reference), Some(value)) = (&refresh_ref, &token.refresh_token) {
        vault.put(reference, value).map_err(SupervisorError::Vault)?;
    }
    let seconds = i64::try_from(token.expires_in_secs).unwrap_or(i64::MAX);
    let expires_at = now
        .checked_add_signed(ChronoDuration::seconds(seconds))
        .unwrap_or(DateTime::<Utc>::MAX_UTC);
    Ok(StoredOAuthTokenRefs {
        access_token_vault_ref: access_ref,
        refresh_token_vault_ref: refresh_ref,
        expires_at_unix_ms: expires_at.timestamp_millis(),
        scopes: token.scopes,
    })
}

fn validate_server_id(server_id: &str) -> Result<(), SupervisorError> {
    if server_id.is_empty()
        || !server_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(SupervisorError::InvalidServerId(server_id.into()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpSchemaChangeEvent {
    pub server_id: String,
    pub release_version: String,
    pub changes: Vec<McpSchemaChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpSchemaChange {
    pub tool_name: String,
    pub kind: String,
    pub previous_version: Option<String>,
    pub discovered_version: String,
    pub previous_schema_hash: Option<String>,
    pub discovered_schema_hash: String,
    pub visible: bool,
}

#[derive(Default)]
pub(crate) struct McpSchemaSupervisor {
    registry: McpRegistry,
}

impl McpSchemaSupervisor {
    pub(crate) fn registry(&self) -> &McpRegistry {
        &self.registry
    }

    pub(crate) fn discover(
        &mut self,
        release: McpRelease,
    ) -> Result<McpSchemaChangeEvent, SupervisorError> {
        let update = self.registry.discover(release)?;
        Ok(McpSchemaChangeEvent {
            server_id: update.server_id,
            release_version: update.version,
            changes: update
                .drifts
                .into_iter()
                .map(|drift| McpSchemaChange {
                    tool_name: drift.tool_name,
                    kind: drift_kind_label(drift.kind).into(),
                    previous_version: drift.previous_version,
                    discovered_version: drift.discovered_version,
                    previous_schema_hash: drift.previous_schema_hash,
                    discovered_schema_hash: drift.discovered_schema_hash,
                    visible: false,
                })
                .collect(),
        })
    }

    pub(crate) fn approve(
        &mut self,
        server_id: &str,
        tool_name: &str,
        release_version: &str,
        schema_hash: &str,
    ) -> Result<RegisteredMcpTool, SupervisorError> {
        self.registry
            .approve_tool(server_id, tool_name, release_version, schema_hash)
            .map_err(Into::into)
    }
}

fn drift_kind_label(kind: RegistryDriftKind) -> &'static str {
    match kind {
        RegistryDriftKind::NewTool => "newTool",
        RegistryDriftKind::SchemaChanged => "schemaChanged",
        RegistryDriftKind::VersionChanged => "versionChanged",
        RegistryDriftKind::SchemaAndVersionChanged => "schemaAndVersionChanged",
    }
}

pub(crate) struct McpHealthSupervisor {
    limiter: RateLimiter,
    circuit: CircuitBreaker,
}

impl McpHealthSupervisor {
    pub(crate) fn new(
        rate_limit: RateLimitPolicy,
        circuit: CircuitBreakerConfig,
        now: Instant,
    ) -> Self {
        Self {
            limiter: RateLimiter::new(rate_limit, now),
            circuit: CircuitBreaker::new(circuit),
        }
    }

    pub(crate) fn before_call(&mut self, now: Instant) -> Result<CircuitPermit, SupervisorError> {
        let permit = self.circuit.before_call(now)?;
        self.limiter.acquire(now)?;
        Ok(permit)
    }

    pub(crate) fn record_success(&mut self) {
        self.circuit.record_success();
    }

    pub(crate) fn record_failure(&mut self, now: Instant) {
        self.circuit.record_failure(now);
    }
}

#[derive(Default)]
pub(crate) struct McpSupervisorRuntime {
    pub oauth: McpOAuthState,
    pub schemas: McpSchemaSupervisor,
    health: HashMap<String, McpHealthSupervisor>,
}

impl McpSupervisorRuntime {
    pub(crate) fn health_mut(&mut self, server_id: &str, now: Instant) -> &mut McpHealthSupervisor {
        self.health.entry(server_id.into()).or_insert_with(|| {
            McpHealthSupervisor::new(
                RateLimitPolicy {
                    max_requests: 60,
                    window: Duration::from_secs(60),
                },
                CircuitBreakerConfig {
                    failure_threshold: 3,
                    base_backoff: Duration::from_secs(1),
                    max_backoff: Duration::from_secs(60),
                },
                now,
            )
        })
    }
}

pub(crate) struct ReqwestOAuthTransport {
    client: reqwest::Client,
    metadata: OAuthClientMetadata,
}

impl ReqwestOAuthTransport {
    pub(crate) fn new(metadata: OAuthClientMetadata) -> Self {
        Self {
            client: reqwest::Client::new(),
            metadata,
        }
    }

    async fn token_request(
        &self,
        form: &[(&str, &str)],
    ) -> Result<OAuthTokenResponse, OAuthError> {
        let response = self
            .client
            .post(&self.metadata.token_endpoint)
            .form(form)
            .send()
            .await
            .map_err(|error| OAuthError::Transport(error.without_url().to_string()))?;
        let status = response.status();
        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|error| OAuthError::Transport(error.without_url().to_string()))?;
        if !status.is_success() {
            let message = value
                .get("error_description")
                .or_else(|| value.get("error"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("token endpoint rejected the request");
            return Err(OAuthError::Transport(format!("HTTP {status}: {message}")));
        }
        parse_token_response(value, &self.metadata.scopes)
    }
}

#[async_trait]
impl OAuthTransport for ReqwestOAuthTransport {
    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
    ) -> Result<OAuthTokenResponse, OAuthError> {
        self.token_request(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", verifier),
            ("client_id", &self.metadata.client_id),
            ("redirect_uri", &self.metadata.redirect_uri),
        ])
        .await
    }

    async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokenResponse, OAuthError> {
        self.token_request(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.metadata.client_id),
        ])
        .await
    }
}

fn parse_token_response(
    value: serde_json::Value,
    default_scopes: &BTreeSet<String>,
) -> Result<OAuthTokenResponse, OAuthError> {
    let access_token = value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| OAuthError::Transport("token response missing access_token".into()))?
        .to_owned();
    let refresh_token = value
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let expires_in_secs = match value.get("expires_in") {
        Some(serde_json::Value::Number(number)) => number.as_u64().unwrap_or(3600),
        Some(serde_json::Value::String(value)) => value.parse().unwrap_or(3600),
        _ => 3600,
    };
    let scopes = value
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .map(|scope| scope.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_else(|| default_scopes.clone());
    Ok(OAuthTokenResponse {
        access_token,
        refresh_token,
        expires_in_secs,
        scopes,
    })
}

#[derive(Debug)]
pub(crate) enum SupervisorError {
    OAuth(OAuthError),
    Registry(RegistryError),
    Circuit(CircuitError),
    RateLimit(RateLimitError),
    Browser(String),
    Vault(String),
    PendingOAuthMissing(String),
    InvalidServerId(String),
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OAuth(error) => error.fmt(formatter),
            Self::Registry(error) => error.fmt(formatter),
            Self::Circuit(error) => error.fmt(formatter),
            Self::RateLimit(error) => error.fmt(formatter),
            Self::Browser(error) => write!(formatter, "OAuth browser failed: {error}"),
            Self::Vault(error) => write!(formatter, "OAuth Vault failed: {error}"),
            Self::PendingOAuthMissing(server) => {
                write!(formatter, "pending OAuth session for MCP server `{server}` is missing")
            }
            Self::InvalidServerId(server) => write!(formatter, "invalid MCP server id `{server}`"),
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<OAuthError> for SupervisorError {
    fn from(error: OAuthError) -> Self {
        Self::OAuth(error)
    }
}

impl From<RegistryError> for SupervisorError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<CircuitError> for SupervisorError {
    fn from(error: CircuitError) -> Self {
        Self::Circuit(error)
    }
}

impl From<RateLimitError> for SupervisorError {
    fn from(error: RateLimitError) -> Self {
        Self::RateLimit(error)
    }
}
