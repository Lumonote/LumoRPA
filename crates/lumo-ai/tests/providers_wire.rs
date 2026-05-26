//! Wiremock-based smoke tests for OpenAI / Anthropic providers.

use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter,
};
use serde_json::json;
use std::sync::{Mutex, OnceLock};
use wiremock::{
    matchers::{header, method, path},
    Mock, MockServer, ResponseTemplate,
};

/// Serialise tests that mutate the process-wide `LUMO_ALLOW_LLM_NETWORK`
/// env var. Cargo runs tests in parallel by default; without this guard the
/// `network_disabled_by_default_blocks_call` test would race other tests
/// calling `allow_network()`.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|p| p.into_inner())
}

fn allow_network() {
    std::env::set_var("LUMO_ALLOW_LLM_NETWORK", "1");
}

#[tokio::test]
async fn openai_provider_round_trip() {
    let _g = env_lock();
    allow_network();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "x",
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" }
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
        })))
        .mount(&server)
        .await;

    let cfg = ProvidersConfig {
        active: Some("openai-test".into()),
        profiles: vec![ProviderProfile {
            name: "openai-test".into(),
            kind: "openai".into(),
            base_url: Some(server.uri()),
            api_key: Some("test-key".into()),
            api_key_env: None,
            default_model: Some("gpt-4o-mini".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    let router = AiRouter::from_config(&cfg);

    let req = ChatRequest {
        model: "openai-test/gpt-4o-mini".into(),
        messages: vec![ChatMessage { role: Role::User, content: "ping".into() }],
        temperature: Some(0.1),
        max_tokens: Some(8),
        system: Some("be terse".into()),
    };
    let resp = router.chat(req).await.expect("chat");
    assert_eq!(resp.content, "pong");
    assert_eq!(resp.provider, "openai-test");
    assert_eq!(resp.input_tokens, 5);
    assert_eq!(resp.output_tokens, 1);
}

#[tokio::test]
async fn openai_provider_propagates_api_error() {
    let _g = env_lock();
    allow_network();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"bad key"}"#))
        .mount(&server)
        .await;

    let cfg = ProvidersConfig {
        active: Some("openai-bad".into()),
        profiles: vec![ProviderProfile {
            name: "openai-bad".into(),
            kind: "openai".into(),
            base_url: Some(server.uri()),
            api_key: Some("wrong".into()),
            api_key_env: None,
            default_model: Some("gpt-4o-mini".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    let router = AiRouter::from_config(&cfg);
    let req = ChatRequest {
        model: "openai-bad/gpt-4o-mini".into(),
        messages: vec![ChatMessage { role: Role::User, content: "ping".into() }],
        temperature: None, max_tokens: None, system: None,
    };
    let err = router.chat(req).await.expect_err("should fail");
    let msg = err.to_string();
    assert!(msg.contains("401"), "expected 401 in error, got: {msg}");
}

#[tokio::test]
async fn anthropic_provider_x_api_key_round_trip() {
    let _g = env_lock();
    allow_network();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "msg_1",
            "model": "claude-opus-4-7",
            "content": [
                { "type": "text", "text": "hello " },
                { "type": "text", "text": "world" }
            ],
            "usage": { "input_tokens": 4, "output_tokens": 2 }
        })))
        .mount(&server)
        .await;

    let cfg = ProvidersConfig {
        active: Some("claude-test".into()),
        profiles: vec![ProviderProfile {
            name: "claude-test".into(),
            kind: "anthropic".into(),
            base_url: Some(server.uri()),
            api_key: Some("sk-test".into()),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
            default_model: Some("claude-opus-4-7".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    let router = AiRouter::from_config(&cfg);

    let req = ChatRequest {
        model: "claude-test/claude-opus-4-7".into(),
        messages: vec![ChatMessage { role: Role::User, content: "hi".into() }],
        temperature: None,
        max_tokens: Some(8),
        system: Some("be friendly".into()),
    };
    let resp = router.chat(req).await.expect("chat");
    assert_eq!(resp.content, "hello world");
    assert_eq!(resp.provider, "claude-test");
}

#[tokio::test]
async fn anthropic_provider_bearer_auth_with_beta() {
    let _g = env_lock();
    allow_network();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("Authorization", "Bearer sk-bear-xyz"))
        .and(header("anthropic-beta", "context-1m-2025-08-07"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "claude-opus-4-7[1m]",
            "content": [{ "type": "text", "text": "ok" }],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })))
        .mount(&server)
        .await;

    let mut headers = std::collections::BTreeMap::new();
    headers.insert("anthropic-beta".into(), "context-1m-2025-08-07".into());
    let cfg = ProvidersConfig {
        active: Some("claude-proxy".into()),
        profiles: vec![ProviderProfile {
            name: "claude-proxy".into(),
            kind: "anthropic".into(),
            base_url: Some(server.uri()),
            api_key: Some("sk-bear-xyz".into()),
            api_key_env: Some("ANTHROPIC_AUTH_TOKEN".into()), // triggers Bearer
            default_model: Some("claude-opus-4-7[1m]".into()),
            models: vec![],
            headers,
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    let router = AiRouter::from_config(&cfg);

    let req = ChatRequest {
        model: "claude-proxy/claude-opus-4-7[1m]".into(),
        messages: vec![ChatMessage { role: Role::User, content: "x".into() }],
        temperature: None,
        max_tokens: Some(8),
        system: None,
    };
    let resp = router.chat(req).await.expect("chat");
    assert_eq!(resp.content, "ok");
    assert_eq!(resp.model, "claude-opus-4-7[1m]");
}

#[tokio::test]
async fn network_disabled_by_default_blocks_call() {
    let _g = env_lock();
    std::env::remove_var("LUMO_ALLOW_LLM_NETWORK");
    let cfg = ProvidersConfig {
        active: Some("openai-ng".into()),
        profiles: vec![ProviderProfile {
            name: "openai-ng".into(),
            kind: "openai".into(),
            base_url: Some("http://127.0.0.1:1".into()),
            api_key: Some("x".into()),
            api_key_env: None,
            default_model: Some("gpt-4o-mini".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    let router = AiRouter::from_config(&cfg);
    let req = ChatRequest {
        model: "openai-ng/gpt-4o-mini".into(),
        messages: vec![ChatMessage { role: Role::User, content: "p".into() }],
        temperature: None, max_tokens: None, system: None,
    };
    let err = router.chat(req).await.expect_err("should refuse");
    assert!(err.to_string().contains("LUMO_ALLOW_LLM_NETWORK"));
    drop(_g);
    allow_network();
}
