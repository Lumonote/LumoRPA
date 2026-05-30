//! P1-4: a real `AiHooks` provider must meter every hook round-trip so the VM
//! can drain the usage and write `ai_calls` ledger rows. This drives the
//! `heal_selector` insertion point against a mock OpenAI endpoint and checks
//! that `take_usage()` reports the response's tokens with a derived cost.

use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    AiHooks, AiRouter, RunBudget,
};
use lumo_core::ai_hook::AiHookProvider;
use serde_json::json;
use std::sync::Arc;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

fn profile_for(server_uri: String) -> ProvidersConfig {
    ProvidersConfig {
        active: Some("openai-test".into()),
        profiles: vec![ProviderProfile {
            name: "openai-test".into(),
            kind: "openai".into(),
            base_url: Some(server_uri),
            api_key: Some("test-key".into()),
            api_key_env: None,
            default_model: Some("gpt-4o-mini".into()),
            models: vec![],
            headers: Default::default(),
            wire_api: None,
            reasoning_effort: None,
            notes: None,
        }],
    }
}

#[tokio::test]
async fn ai_hooks_meter_each_call_for_the_cost_ledger() {
    std::env::set_var("LUMO_ALLOW_LLM_NETWORK", "1");
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "{\"css\":\"#ok\",\"xpath\":null,\"confidence\":0.9,\"reasoning\":\"r\"}"
                }
            }],
            "usage": { "prompt_tokens": 12, "completion_tokens": 34 }
        })))
        .mount(&server)
        .await;

    let router = Arc::new(AiRouter::from_config(&profile_for(server.uri())));
    let hooks = AiHooks::new(router, RunBudget::new(8));

    // Nothing recorded before any call.
    assert!(hooks.take_usage().is_empty());

    let healed = hooks
        .heal_selector(
            "#missing",
            "the OK button",
            None,
            Some("openai-test/gpt-4o-mini"),
        )
        .await
        .expect("heal_selector should succeed against the mock");
    assert_eq!(healed.css.as_deref(), Some("#ok"));

    let usage = hooks.take_usage();
    assert_eq!(usage.len(), 1, "one chat round-trip → one usage record");
    let u = &usage[0];
    assert_eq!(u.helper, "heal_selector");
    assert_eq!(u.provider, "openai-test");
    assert_eq!(u.model, "gpt-4o-mini");
    assert_eq!(u.input_tokens, 12);
    assert_eq!(u.output_tokens, 34);
    assert!(
        u.cost_usd_micro > 0,
        "cost must be derived from provider/model/tokens, got {}",
        u.cost_usd_micro
    );

    // Draining consumed the record — a second drain is empty.
    assert!(hooks.take_usage().is_empty());
}
