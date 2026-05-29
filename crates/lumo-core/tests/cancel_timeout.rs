//! P1-1: the VM supports cooperative cancellation (a `CancelToken` checked
//! before each step and able to interrupt an in-flight step) and a per-step
//! timeout. A cancelled run reports `ExecError::Cancelled`; a step that exceeds
//! the timeout reports `ExecError::Timeout`.

use async_trait::async_trait;
use lumo_core::error::{ExecError, StepError};
use lumo_core::{Action, ActionRegistry, ActionResult, CancelToken, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::time::Duration;

/// Sleeps for `ms` (from `with: { ms }`) then succeeds. Lets tests build a
/// long-running step that cancellation / timeout can interrupt.
struct SleepAction;
#[async_trait]
impl Action for SleepAction {
    fn id(&self) -> &'static str {
        "test.sleep"
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ms = input.get("ms").and_then(Value::as_u64).unwrap_or(0);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(ActionResult::null())
    }
}

fn reg() -> ActionRegistry {
    let mut r = ActionRegistry::new();
    r.register(SleepAction);
    r
}

const SLEEP_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: slow, action: test.sleep, with: { ms: 600 } }
    - { id: after, action: test.sleep, with: { ms: 0 } }
"#;

#[tokio::test]
async fn cancel_mid_step_aborts_run() {
    let token = CancelToken::new();
    let vm = FlowVm::new(reg(), None).with_cancel(token.clone());
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        canceller.cancel();
    });
    let err = vm
        .run(&parse_str(SLEEP_FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("cancellation should abort the run");
    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
}

#[tokio::test]
async fn cancel_before_run_stops_at_first_step() {
    let token = CancelToken::new();
    token.cancel();
    let vm = FlowVm::new(reg(), None).with_cancel(token);
    let err = vm
        .run(&parse_str(SLEEP_FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("a pre-cancelled token stops the run immediately");
    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
}

#[tokio::test]
async fn step_timeout_fires() {
    let vm = FlowVm::new(reg(), None).with_step_timeout(Duration::from_millis(40));
    let err = vm
        .run(&parse_str(SLEEP_FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("the 600ms step should exceed the 40ms timeout");
    assert!(matches!(err, ExecError::Timeout { .. }), "got: {err}");
    assert!(err.to_string().contains("timed out"));
}

#[tokio::test]
async fn no_limits_runs_normally() {
    let vm = FlowVm::new(reg(), None);
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: quick, action: test.sleep, with: { ms: 0 } }
"#,
    )
    .unwrap();
    let report = vm.run(&flow, RunOptions::default()).await.unwrap();
    assert!(report.success);
}
