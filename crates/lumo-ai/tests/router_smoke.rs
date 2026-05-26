//! Router-level smoke tests (against the wiremock-backed OpenAI provider).

use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter,
};
use serde_json::json;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

async fn make_router(name: &str) -> (AiRouter, MockServer) {
    std::env::set_var("LUMO_ALLOW_LLM_NETWORK", "1");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "echo",
            "choices": [{
                "message": { "role": "assistant", "content": format!("ok-{}", name) }
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        })))
        .mount(&server)
        .await;
    let cfg = ProvidersConfig {
        active: Some(name.into()),
        profiles: vec![ProviderProfile {
            name: name.into(),
            kind: "openai".into(),
            base_url: Some(server.uri()),
            api_key: Some("x".into()),
            api_key_env: None,
            default_model: Some("any".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    };
    (AiRouter::from_config(&cfg), server)
}

#[tokio::test]
async fn explicit_profile_prefix_routes() {
    let (router, _s) = make_router("beta").await;
    let req = ChatRequest {
        model: "beta/anything".into(),
        messages: vec![ChatMessage { role: Role::User, content: "x".into() }],
        temperature: None, max_tokens: None, system: None,
    };
    let r = router.chat(req).await.unwrap();
    assert_eq!(r.provider, "beta");
    assert_eq!(r.content, "ok-beta");
}

#[tokio::test]
async fn empty_model_falls_back_to_active_default() {
    let (router, _s) = make_router("alpha").await;
    let req = ChatRequest {
        model: "".into(),
        messages: vec![ChatMessage { role: Role::User, content: "x".into() }],
        temperature: None, max_tokens: None, system: None,
    };
    let r = router.chat(req).await.unwrap();
    assert_eq!(r.provider, "alpha");
}

#[tokio::test]
async fn unknown_model_errors() {
    let cfg = ProvidersConfig { active: None, profiles: vec![] };
    let router = AiRouter::from_config(&cfg);
    let req = ChatRequest {
        model: "vendor/anything".into(),
        messages: vec![],
        temperature: None, max_tokens: None, system: None,
    };
    assert!(router.chat(req).await.is_err());
}
