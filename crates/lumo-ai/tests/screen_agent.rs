use async_trait::async_trait;
use lumo_ai::{
    AiRouter, ChatRequest, ChatResponse, LlmProvider, ProviderError, ProviderId, ScreenAgentAction,
};
use lumo_core::{
    Action, ActionRegistry, ActionResult, CancelToken, HumanPromptRequest, HumanPrompter,
    HumanResponse, StepCtx, StepError,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

type Events = Arc<Mutex<Vec<(String, Value)>>>;
type Setup = (ScreenAgentAction, StepCtx, Events);

struct Model {
    responses: Mutex<VecDeque<String>>,
}
#[async_trait]
impl LlmProvider for Model {
    fn name(&self) -> &str {
        "test"
    }
    fn id(&self) -> ProviderId {
        ProviderId::OpenaiCompat
    }
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        assert_eq!(request.messages[0].attachments.len(), 1);
        Ok(ChatResponse {
            content: self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected model request"),
            model: "test/model".into(),
            provider: "test".into(),
            input_tokens: 10,
            output_tokens: 5,
        })
    }
}
struct Observe {
    events: Events,
}
#[async_trait]
impl Action for Observe {
    fn id(&self) -> &'static str {
        "desktop.observe"
    }
    async fn execute(&self, _: &mut StepCtx, _: Value) -> Result<ActionResult, StepError> {
        let mut events = self.events.lock().unwrap();
        let image = format!("image-{}", events.len());
        events.push((self.id().into(), Value::Null));
        Ok(ActionResult::from(
            json!({"png":image,"width":800,"height":600,"screen_x":-1000,"screen_y":0,"screen_width":1600,"screen_height":1200}),
        ))
    }
}
struct Click {
    events: Events,
}
#[async_trait]
impl Action for Click {
    fn id(&self) -> &'static str {
        "desktop.click"
    }
    async fn execute(&self, _: &mut StepCtx, args: Value) -> Result<ActionResult, StepError> {
        self.events.lock().unwrap().push((self.id().into(), args));
        Ok(ActionResult::null())
    }
}
struct Decline;
#[async_trait]
impl HumanPrompter for Decline {
    async fn prompt(&self, _: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        Ok(HumanResponse {
            value: false.into(),
            by: None,
            comment: None,
        })
    }
}

fn setup(responses: &[&str]) -> Setup {
    let model = Model {
        responses: Mutex::new(responses.iter().map(|v| v.to_string()).collect()),
    };
    setup_model(model)
}
fn setup_model(model: impl LlmProvider + 'static) -> Setup {
    let router = AiRouter::builder().with(model).active("test").build();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ActionRegistry::new();
    registry.register(Observe {
        events: events.clone(),
    });
    registry.register(Click {
        events: events.clone(),
    });
    let ctx = StepCtx::new(
        "run".into(),
        "flow".into(),
        registry,
        None,
        json!({}),
        lumo_dsl::Capabilities {
            desktop: vec!["*".into()],
            llm: vec!["*".into()],
            ..Default::default()
        },
        vec![],
    );
    (ScreenAgentAction::new(Arc::new(router)), ctx, events)
}

struct PendingModel {
    started: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl LlmProvider for PendingModel {
    fn name(&self) -> &str {
        "test"
    }
    fn id(&self) -> ProviderId {
        ProviderId::OpenaiCompat
    }
    async fn chat(&self, _: ChatRequest) -> Result<ChatResponse, ProviderError> {
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_pending_model_keeps_unknown_usage_and_never_operates() {
    let started = Arc::new(tokio::sync::Notify::new());
    let (action, ctx, events) = setup_model(PendingModel {
        started: started.clone(),
    });
    let cancel = CancelToken::new();
    let mut ctx = ctx.with_cancel(Some(cancel.clone()));
    tokio::spawn(async move {
        started.notified().await;
        cancel.cancel();
    });
    let error = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        action.execute(&mut ctx, input()),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(ctx.usage_unknown());
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn internal_deadline_interrupts_native_work_and_preserves_unknown_usage() {
    let (action, mut ctx, events) = setup_model(PendingModel {
        started: Arc::new(tokio::sync::Notify::new()),
    });
    let interrupt = ctx.step_interrupt();
    let mut cfg = input();
    cfg["timeout_ms"] = 1000.into();
    assert!(matches!(
        action.execute(&mut ctx, cfg).await.unwrap_err(),
        StepError::Timeout { ms: 1000 }
    ));
    assert!(interrupt.is_interrupted());
    assert!(ctx.usage_unknown());
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn action_limit_allows_no_second_operation() {
    let (action, mut ctx, events) = setup(&[
        r#"{"action":"click","x":1,"y":1}"#,
        r#"{"action":"click","x":2,"y":2}"#,
    ]);
    let mut cfg = input();
    cfg["max_actions"] = 1.into();
    assert!(action
        .execute(&mut ctx, cfg)
        .await
        .unwrap_err()
        .to_string()
        .contains("action limit"));
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| id == "desktop.click")
            .count(),
        1
    );
}
fn input() -> Value {
    json!({"objective":"Open the report","expected":"The report is visible","model":"test/model","confirm_actions":false})
}

struct BlockingClick {
    started: Arc<tokio::sync::Notify>,
    stopped: Arc<tokio::sync::Notify>,
}
#[async_trait]
impl Action for BlockingClick {
    fn id(&self) -> &'static str {
        "desktop.click"
    }
    async fn execute(&self, ctx: &mut StepCtx, _: Value) -> Result<ActionResult, StepError> {
        let interrupt = ctx.step_interrupt();
        let started = self.started.clone();
        let stopped = self.stopped.clone();
        tokio::task::spawn_blocking(move || {
            started.notify_one();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !interrupt.is_interrupted() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(
                interrupt.is_interrupted(),
                "blocking native work survived interruption"
            );
            stopped.notify_one();
            Err(StepError::msg("native work interrupted"))
        })
        .await
        .unwrap()
    }
}

#[tokio::test]
async fn active_native_work_observes_internal_timeout_and_user_cancellation() {
    for cancel_early in [false, true] {
        let (action, ctx, _) = setup(&[r#"{"action":"click","x":1,"y":1}"#]);
        let cancel = CancelToken::new();
        let mut ctx = ctx.with_cancel(Some(cancel.clone()));
        let started = Arc::new(tokio::sync::Notify::new());
        let stopped = Arc::new(tokio::sync::Notify::new());
        ctx.registry.register(BlockingClick {
            started: started.clone(),
            stopped: stopped.clone(),
        });
        let mut cfg = input();
        cfg["timeout_ms"] = 1000.into();
        let task = tokio::spawn(async move { action.execute(&mut ctx, cfg).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        if cancel_early {
            cancel.cancel();
        }
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        if cancel_early {
            assert!(error.to_string().contains("cancelled"));
        } else {
            assert!(matches!(error, StepError::Timeout { ms: 1000 }));
        }
        tokio::time::timeout(std::time::Duration::from_secs(1), stopped.notified())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn visual_loop_recaptures_after_action_and_separately_verifies_result() {
    let (action, mut ctx, events) = setup(&[
        r#"{"action":"click","x":200,"y":150}"#,
        r#"{"action":"done"}"#,
        r#"{"verified":true,"confidence":0.98}"#,
    ]);
    let result = action.execute(&mut ctx, input()).await.unwrap().output;
    assert_eq!(result["verified"], true);
    assert_eq!(result["tokens_used"], 45);
    let events = events.lock().unwrap();
    assert_eq!(
        events.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        vec![
            "desktop.observe",
            "desktop.click",
            "desktop.observe",
            "desktop.observe"
        ]
    );
    assert_eq!(events[1].1, json!({"x":-600.0,"y":300.0}));
    assert!(!result.to_string().contains("image-"));
}

#[tokio::test]
async fn untrusted_model_cannot_dispatch_unknown_tools_or_offscreen_clicks() {
    for response in [
        r#"{"action":"system.shell","command":"echo unsafe"}"#,
        r#"{"action":"click","x":800,"y":0}"#,
        r#"{"action":"key","key":"cmd+space"}"#,
    ] {
        let (action, mut ctx, events) = setup(&[response]);
        assert!(action.execute(&mut ctx, input()).await.is_err());
        assert_eq!(events.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn proposal_done_is_not_success_without_visible_postcondition() {
    let (action, mut ctx, _) = setup(&[
        r#"{"action":"done"}"#,
        r#"{"verified":false,"confidence":0.99}"#,
    ]);
    assert!(action
        .execute(&mut ctx, input())
        .await
        .unwrap_err()
        .to_string()
        .contains("independently verified"));
}

#[tokio::test]
async fn token_limit_prevents_operation_after_model_budget_overrun() {
    let (action, mut ctx, events) = setup(&[r#"{"action":"click","x":1,"y":1}"#]);
    let mut cfg = input();
    cfg["max_tokens"] = 1.into();
    assert!(action
        .execute(&mut ctx, cfg)
        .await
        .unwrap_err()
        .to_string()
        .contains("budget"));
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn declined_confirmation_and_cancel_do_not_operate_desktop() {
    let (action, ctx, events) = setup(&[r#"{"action":"click","x":1,"y":1}"#]);
    let mut ctx = ctx.with_human_prompter(Some(Arc::new(Decline)));
    let mut cfg = input();
    cfg["confirm_actions"] = true.into();
    assert!(action
        .execute(&mut ctx, cfg)
        .await
        .unwrap_err()
        .to_string()
        .contains("declined"));
    assert_eq!(events.lock().unwrap().len(), 1);
    let (action, ctx, events) = setup(&[]);
    let cancel = CancelToken::new();
    cancel.cancel();
    let mut ctx = ctx.with_cancel(Some(cancel));
    assert!(action.execute(&mut ctx, input()).await.is_err());
    assert!(events.lock().unwrap().is_empty());
}
