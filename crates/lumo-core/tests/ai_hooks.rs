//! P0-1 end-to-end guard: with an `AiHookProvider` attached, a step marked
//! `ai.mode: fallback` whose action fails with `SelectorNotFound` triggers
//! `heal_selector` and recovers. Without a provider (the pre-P0-1 production
//! state, where no runner ever called `with_ai_provider`) the same failure is
//! terminal. These two tests together prove the wiring is the thing that makes
//! the AI-hooks subsystem live.

use async_trait::async_trait;
use lumo_core::ai_hook::{AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Deterministic action that fails with `SelectorNotFound` until invoked with
/// the healed selector `#healed`.
struct FlakyClick;

#[async_trait]
impl Action for FlakyClick {
    fn id(&self) -> &'static str {
        "test.flaky_click"
    }
    fn summary(&self) -> &'static str {
        "fails with SelectorNotFound until selector == #healed"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let sel = input.get("selector").and_then(Value::as_str).unwrap_or("");
        if sel == "#healed" {
            Ok(ActionResult::from(serde_json::json!({ "clicked": sel })))
        } else {
            Err(StepError::SelectorNotFound(sel.to_string()))
        }
    }
}

/// Stub provider: records `heal_selector` calls and hands back `#healed`.
struct StubHooks {
    heal_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AiHookProvider for StubHooks {
    async fn heal_selector(
        &self,
        _failed: &str,
        _prompt: &str,
        _dom: Option<&str>,
        _model: Option<&str>,
    ) -> Result<HealedSelector, StepError> {
        self.heal_calls.fetch_add(1, Ordering::SeqCst);
        Ok(HealedSelector {
            css: Some("#healed".into()),
            xpath: None,
            bbox: None,
            confidence: 0.95,
            reasoning: "stub".into(),
        })
    }
    async fn extract_visual(
        &self,
        _: Option<bytes::Bytes>,
        _: &str,
        _: Option<&str>,
        _: Option<&Value>,
        _: Option<&str>,
    ) -> Result<Value, StepError> {
        Err(StepError::msg("unused"))
    }
    async fn decide(&self, _: &Value, _: &str, _: Option<&str>) -> Result<Decision, StepError> {
        Err(StepError::msg("unused"))
    }
    async fn diagnose(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> Result<String, StepError> {
        Ok(String::new())
    }
    async fn vision_locate(
        &self,
        _: bytes::Bytes,
        _: &str,
        _: &[SoMMark],
        _: Option<&str>,
    ) -> Result<LocatedTarget, StepError> {
        Ok(LocatedTarget::default())
    }
}

const HEAL_FLOW: &str = r##"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: heal-test }
spec:
  steps:
    - id: click
      action: test.flaky_click
      with: { selector: "#missing" }
      ai: { mode: fallback }
"##;

#[tokio::test]
async fn ai_fallback_heals_selector_not_found_when_provider_attached() {
    let flow = parse_str(HEAL_FLOW).expect("parse");
    let mut reg = ActionRegistry::new();
    reg.register(FlakyClick);
    let heal_calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(StubHooks {
        heal_calls: heal_calls.clone(),
    });

    let vm = FlowVm::new(reg, None).with_ai_provider(hooks);
    let report = vm.run(&flow, RunOptions::default()).await.expect("run");

    assert!(report.success, "run should recover via AI heal");
    assert_eq!(
        heal_calls.load(Ordering::SeqCst),
        1,
        "heal_selector must fire exactly once"
    );
}

#[tokio::test]
async fn no_provider_means_selector_failure_is_terminal() {
    // Same flow, NO provider attached (the pre-P0-1 production state):
    // effective_ai_mode resolves to Off, so the failure is terminal.
    let flow = parse_str(HEAL_FLOW).expect("parse");
    let mut reg = ActionRegistry::new();
    reg.register(FlakyClick);

    let vm = FlowVm::new(reg, None);
    let result = vm.run(&flow, RunOptions::default()).await;

    assert!(
        result.is_err(),
        "without a provider the selector failure must be terminal"
    );
}
