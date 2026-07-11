use std::collections::BTreeSet;

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthClientMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthPkceSession {
    pub metadata: OAuthClientMetadata,
    pub state: String,
    pub verifier: String,
    pub authorization_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in_secs: u64,
    pub scopes: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOAuthTokenRefs {
    pub access_token_vault_ref: String,
    pub refresh_token_vault_ref: Option<String>,
    pub expires_at_unix_ms: i64,
    pub scopes: BTreeSet<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OAuthError {
    #[error("OAuth callback state does not match the pending authorization")]
    StateMismatch,
    #[error("invalid OAuth configuration: {0}")]
    InvalidConfiguration(String),
    #[error("OAuth refresh token is missing")]
    RefreshTokenMissing,
    #[error("OAuth transport failed: {0}")]
    Transport(String),
}

#[async_trait]
pub trait OAuthTransport: Send + Sync {
    async fn exchange_code(&self, code: &str, verifier: &str) -> Result<OAuthTokenResponse, OAuthError>;
    async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokenResponse, OAuthError>;
}

pub fn begin_pkce(metadata: OAuthClientMetadata, state: String, verifier: String) -> Result<OAuthPkceSession, OAuthError> {
    if state.len() < 7 || verifier.len() < 16 {
        return Err(OAuthError::InvalidConfiguration("state or PKCE verifier is too short".into()));
    }
    let mut url = reqwest::Url::parse(&metadata.authorization_endpoint)
        .map_err(|error| OAuthError::InvalidConfiguration(error.to_string()))?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &metadata.client_id)
        .append_pair("redirect_uri", &metadata.redirect_uri)
        .append_pair("scope", &metadata.scopes.iter().cloned().collect::<Vec<_>>().join(" "))
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(OAuthPkceSession { metadata, state, verifier, authorization_url: url.to_string() })
}

pub async fn complete_authorization(
    transport: &dyn OAuthTransport,
    session: &OAuthPkceSession,
    returned_state: &str,
    code: &str,
) -> Result<OAuthTokenResponse, OAuthError> {
    if !constant_time_eq(session.state.as_bytes(), returned_state.as_bytes()) {
        return Err(OAuthError::StateMismatch);
    }
    transport.exchange_code(code, &session.verifier).await
}

pub async fn refresh_if_needed(
    transport: &dyn OAuthTransport,
    token: OAuthTokenResponse,
    expired: bool,
) -> Result<OAuthTokenResponse, OAuthError> {
    if !expired { return Ok(token); }
    let refresh = token.refresh_token.as_deref().ok_or(OAuthError::RefreshTokenMissing)?;
    transport.refresh(refresh).await
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() { return false; }
    left.iter().zip(right).fold(0_u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
