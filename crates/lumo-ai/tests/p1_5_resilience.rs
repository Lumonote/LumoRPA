//! P1-5: provider transient-error backoff + image-size guard.

use lumo_ai::{
    budget::RunBudget,
    config::{ProviderProfile, ProvidersConfig},
    extract_visual,
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter,
};
use bytes::Bytes;
use serde_json::json;
use std::sync::OnceLock;
use tokio::sync::{Mutex, MutexGuard};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

async fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

fn allow_network() {
    std::env::set_var("LUMO_ALLOW_LLM_NETWORK", "1");
}

/// 429 once then 200 → provider retries and ultimately succeeds.
#[tokio::test]
async fn openai_retries_on_429_then_succeeds() {
    let _g = env_lock().await;
    allow_network();
    let server = MockServer::start().await;

    // First call: 429 (rate limited). Mounted with expectation of exactly 1 hit.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string(r#"{"error":"slow down"}"#))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;

    // Subsequent calls: 200 OK.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" }
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
        })))
        .expect(1)
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
        messages: vec![ChatMessage::text(Role::User, "ping")],
        temperature: None,
        max_tokens: None,
        system: None,
    };
    let resp = router.chat(req).await.expect("chat should retry past 429");
    assert_eq!(resp.content, "pong");
    // Two requests prove a retry occurred (Mock `.expect(...)` verified on drop).
    let received = server.received_requests().await.unwrap();
    assert!(
        received.len() > 1,
        "expected >1 request (retry), got {}",
        received.len()
    );
}

/// Anthropic 503 once then 200 → retries and succeeds.
#[tokio::test]
async fn anthropic_retries_on_503_then_succeeds() {
    let _g = env_lock().await;
    allow_network();
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(503).set_body_string("overloaded"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "claude-opus-4-7",
            "content": [{ "type": "text", "text": "ok" }],
            "usage": { "input_tokens": 1, "output_tokens": 1 }
        })))
        .expect(1)
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
        messages: vec![ChatMessage::text(Role::User, "hi")],
        temperature: None,
        max_tokens: Some(8),
        system: None,
    };
    let resp = router.chat(req).await.expect("chat should retry past 503");
    assert_eq!(resp.content, "ok");
    let received = server.received_requests().await.unwrap();
    assert!(received.len() > 1, "expected retry, got {}", received.len());
}

/// A non-retryable 4xx (401) is NOT retried — single request, terminal error.
#[tokio::test]
async fn openai_does_not_retry_on_401() {
    let _g = env_lock().await;
    allow_network();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"bad key"}"#))
        .expect(1)
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
        messages: vec![ChatMessage::text(Role::User, "ping")],
        temperature: None,
        max_tokens: None,
        system: None,
    };
    let err = router.chat(req).await.expect_err("should fail");
    assert!(err.to_string().contains("401"));
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "401 must not be retried");
}

/// Image-size guard: an oversized screenshot is rejected before any network
/// call, with an error mentioning the actual and max size.
#[tokio::test]
async fn extract_visual_rejects_oversized_image() {
    // 5 MiB > 4 MiB cap.
    let oversized = Bytes::from(vec![0u8; 5 * 1024 * 1024]);
    let router = AiRouter::from_config(&ProvidersConfig::seed_default());
    let budget = RunBudget::new(8);
    let err = extract_visual(
        &router,
        &budget,
        Some(oversized),
        "the price",
        None,
        None,
        None,
    )
    .await
    .expect_err("oversized image must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("image") && (msg.contains("4") || msg.contains("max")),
        "error should mention image size limit, got: {msg}"
    );
    // Mentions actual size too.
    assert!(
        msg.contains("5242880") || msg.contains("byte") || msg.contains("size"),
        "error should mention actual size, got: {msg}"
    );
}
