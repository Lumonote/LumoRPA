use std::collections::BTreeSet;

use async_trait::async_trait;
use lumo_actions::mcp::oauth::{
    begin_pkce, complete_authorization, refresh_if_needed, OAuthClientMetadata, OAuthError,
    OAuthTokenResponse, OAuthTransport,
};

struct FakeTransport;

#[async_trait]
impl OAuthTransport for FakeTransport {
    async fn exchange_code(&self, code: &str, verifier: &str) -> Result<OAuthTokenResponse, OAuthError> {
        assert_eq!(code, "code-1");
        assert_eq!(verifier, "verifier-1234567890");
        Ok(OAuthTokenResponse { access_token: "access".into(), refresh_token: Some("refresh".into()), expires_in_secs: 60, scopes: BTreeSet::from(["tools:call".into()]) })
    }

    async fn refresh(&self, refresh_token: &str) -> Result<OAuthTokenResponse, OAuthError> {
        assert_eq!(refresh_token, "refresh");
        Ok(OAuthTokenResponse { access_token: "access-2".into(), refresh_token: Some("refresh-2".into()), expires_in_secs: 120, scopes: BTreeSet::from(["tools:call".into()]) })
    }
}

fn metadata() -> OAuthClientMetadata {
    OAuthClientMetadata { authorization_endpoint: "https://auth.example/authorize".into(), token_endpoint: "https://auth.example/token".into(), client_id: "lumo".into(), redirect_uri: "http://127.0.0.1/callback".into(), scopes: BTreeSet::from(["tools:call".into()]) }
}

#[tokio::test]
async fn pkce_state_is_validated_before_code_exchange() {
    let session = begin_pkce(metadata(), "state-1".into(), "verifier-1234567890".into()).unwrap();
    assert!(session.authorization_url.contains("code_challenge="));
    let error = complete_authorization(&FakeTransport, &session, "wrong", "code-1").await.unwrap_err();
    assert!(matches!(error, OAuthError::StateMismatch));
    let token = complete_authorization(&FakeTransport, &session, "state-1", "code-1").await.unwrap();
    assert_eq!(token.access_token, "access");
}

#[tokio::test]
async fn expired_tokens_refresh_without_persisting_plaintext_contracts() {
    let token = OAuthTokenResponse { access_token: "expired".into(), refresh_token: Some("refresh".into()), expires_in_secs: 0, scopes: BTreeSet::new() };
    let refreshed = refresh_if_needed(&FakeTransport, token, true).await.unwrap();
    assert_eq!(refreshed.access_token, "access-2");
    assert_eq!(refreshed.scopes, BTreeSet::from(["tools:call".into()]));
}
