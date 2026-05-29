//! P1-2: the VM must fire registered run-teardown hooks at end-of-run so that
//! run-scoped external resources (e.g. a launched browser process) are
//! reclaimed whether the flow *succeeded or failed*. A flow that errors before
//! its explicit `browser.close` step must not leak the process.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, RunTeardown, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// Records every `run_id` it is asked to tear down.
struct SpyTeardown {
    seen: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl RunTeardown for SpyTeardown {
    async fn teardown(&self, run_id: &str) {
        self.seen.lock().unwrap().push(run_id.to_string());
    }
}

/// Trivial always-OK action so the success flow has a step to run.
struct OkAction;
#[async_trait]
impl Action for OkAction {
    fn id(&self) -> &'static str {
        "test.ok"
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::null())
    }
}

/// Always-failing action so we can drive the error path.
struct BoomAction;
#[async_trait]
impl Action for BoomAction {
    fn id(&self) -> &'static str {
        "test.boom"
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Err(StepError::msg("boom"))
    }
}

#[tokio::test]
async fn teardown_fires_on_success_with_run_id() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(OkAction);
    reg.register_teardown(Arc::new(SpyTeardown { seen: seen.clone() }));
    let vm = FlowVm::new(reg, None);

    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: a, action: test.ok, with: {} }
"#,
    )
    .expect("parse");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run");
    assert!(report.success);

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "teardown must fire exactly once");
    assert_eq!(seen[0], report.run_id, "teardown receives the run id");
}

#[tokio::test]
async fn teardown_fires_even_when_flow_fails() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(BoomAction);
    reg.register_teardown(Arc::new(SpyTeardown { seen: seen.clone() }));
    let vm = FlowVm::new(reg, None);

    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: boom, action: test.boom, with: {} }
"#,
    )
    .expect("parse");

    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("flow should fail");
    assert!(err.to_string().contains("boom"));

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        1,
        "teardown must fire on the failure path too (this is the leak fix)"
    );
    assert!(!seen[0].is_empty(), "teardown still gets a run id on failure");
}
