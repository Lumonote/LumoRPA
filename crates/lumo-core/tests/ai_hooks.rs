//! P0-1 end-to-end guard: with an `AiHookProvider` attached, a step marked
//! `ai.mode: fallback` whose action fails with `SelectorNotFound` triggers
//! `heal_selector` and recovers. Without a provider (the pre-P0-1 production
//! state, where no runner ever called `with_ai_provider`) the same failure is
//! terminal. These two tests together prove the wiring is the thing that makes
//! the AI-hooks subsystem live.

use async_trait::async_trait;
use lumo_core::ai_hook::{
    AiCallUsage, AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark,
};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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

/// Stub provider: records `heal_selector` calls, hands back `#healed`, and
/// meters each call so the cost-ledger wiring (P1-4) has usage to drain.
struct StubHooks {
    heal_calls: Arc<AtomicUsize>,
    usage: Mutex<Vec<AiCallUsage>>,
}

impl StubHooks {
    fn new(heal_calls: Arc<AtomicUsize>) -> Self {
        Self {
            heal_calls,
            usage: Mutex::new(Vec::new()),
        }
    }
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
        self.usage.lock().unwrap().push(AiCallUsage {
            helper: "heal_selector".into(),
            provider: "stub".into(),
            model: "stub-model".into(),
            input_tokens: 11,
            output_tokens: 22,
            latency_ms: 7,
            cost_usd_micro: 99,
        });
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
    fn take_usage(&self) -> Vec<AiCallUsage> {
        std::mem::take(&mut self.usage.lock().unwrap())
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
    let hooks = Arc::new(StubHooks::new(heal_calls.clone()));

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

/// P1-4: when a hook recovers a step, the VM must drain the provider's metered
/// usage and write one `ai_calls` ledger row per call, attributed to the run
/// and step. Before P1-4 only the `ai.chat` action recorded; hook calls were
/// invisible to `lumo runs cost`.
#[tokio::test]
async fn heal_selector_hook_writes_a_cost_ledger_row() {
    let flow = parse_str(HEAL_FLOW).expect("parse");
    let mut reg = ActionRegistry::new();
    reg.register(FlakyClick);
    let heal_calls = Arc::new(AtomicUsize::new(0));
    let hooks = Arc::new(StubHooks::new(heal_calls.clone()));

    // A real (in-memory) repo so the VM has somewhere to write the ledger row.
    let repo = Repo::open_in_memory().expect("open in-memory repo");
    let vm = FlowVm::new(reg, Some(repo.clone())).with_ai_provider(hooks);
    let report = vm.run(&flow, RunOptions::default()).await.expect("run");

    assert!(report.success, "run should recover via AI heal");
    assert_eq!(heal_calls.load(Ordering::SeqCst), 1);

    let calls = repo
        .list_ai_calls(&report.run_id)
        .expect("read back ai_calls");
    assert_eq!(
        calls.len(),
        1,
        "the heal_selector hook must write exactly one ledger row"
    );
    let row = &calls[0];
    assert_eq!(row.helper, "heal_selector");
    assert_eq!(row.step_id.as_deref(), Some("click"));
    assert_eq!(row.input_tokens, 11);
    assert_eq!(row.output_tokens, 22);
    assert_eq!(row.latency_ms, 7);
    assert_eq!(row.cost_usd_micro, 99);

    // Requirement 2: the runtime `_ai` trace must also carry tokens/latency/cost
    // so `steps.<id>._ai.usage` and the Studio timeline can show per-hook spend.
    let outputs = report.outputs.expect("outputs snapshot present on success");
    let usage = &outputs["click"]["_ai"]["usage"];
    assert_eq!(usage["input_tokens"], 11);
    assert_eq!(usage["output_tokens"], 22);
    assert_eq!(usage["latency_ms"], 7);
    assert_eq!(usage["cost_usd_micro"], 99);

    // The roll-up the VM persists for `lumo runs list` must include hook spend.
    let (tokens, cost) = repo.rollup_run_cost(&report.run_id).expect("rollup");
    assert_eq!(tokens, 33, "input+output tokens summed across hook calls");
    assert_eq!(cost, 99);
}
