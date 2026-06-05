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
            vision_model: None,
            ocr_model: None,
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
        messages: vec![ChatMessage::text(Role::User, "x")],
        temperature: None,
        max_tokens: None,
        system: None,
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
        messages: vec![ChatMessage::text(Role::User, "x")],
        temperature: None,
        max_tokens: None,
        system: None,
    };
    let r = router.chat(req).await.unwrap();
    assert_eq!(r.provider, "alpha");
}

#[tokio::test]
async fn unknown_model_errors() {
    let cfg = ProvidersConfig {
        active: None,
        profiles: vec![],
    };
    let router = AiRouter::from_config(&cfg);
    let req = ChatRequest {
        model: "vendor/anything".into(),
        messages: vec![],
        temperature: None,
        max_tokens: None,
        system: None,
    };
    assert!(router.chat(req).await.is_err());
}

#[test]
fn active_vision_and_ocr_models_fall_back_in_order() {
    let mut profile = ProviderProfile {
        name: "active".into(),
        kind: "openai".into(),
        base_url: None,
        api_key: None,
        api_key_env: None,
        default_model: Some("text-default".into()),
        vision_model: Some("vision-default".into()),
        ocr_model: Some("ocr-default".into()),
        models: vec![],
        headers: Default::default(),
        wire_api: None,
        reasoning_effort: None,
        notes: None,
    };
    let mut cfg = ProvidersConfig {
        active: Some("active".into()),
        profiles: vec![profile.clone()],
    };
    assert_eq!(cfg.active_vision_model().as_deref(), Some("vision-default"));
    assert_eq!(cfg.active_ocr_model().as_deref(), Some("ocr-default"));

    profile.ocr_model = None;
    cfg.profiles = vec![profile.clone()];
    assert_eq!(cfg.active_ocr_model().as_deref(), Some("vision-default"));

    profile.vision_model = None;
    cfg.profiles = vec![profile];
    assert_eq!(cfg.active_vision_model().as_deref(), Some("text-default"));
    assert_eq!(cfg.active_ocr_model().as_deref(), Some("text-default"));
}
