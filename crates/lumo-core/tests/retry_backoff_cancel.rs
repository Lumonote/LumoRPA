//! P1-1 (compensation gap): the retry **backoff sleep** between attempts must
//! be raced against the run's cancel token. A cancellation that lands during a
//! long backoff should abort the run promptly with `ExecError::Cancelled`,
//! never stall for the full backoff window.
//!
//! Regression: the backoff was a bare `tokio::time::sleep(backoff).await`, so a
//! cancel mid-backoff waited the entire sleep before the next cancel check.

use async_trait::async_trait;
use lumo_core::error::{ExecError, StepError};
use lumo_core::{Action, ActionRegistry, ActionResult, CancelToken, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Always fails with a generic (`Other`-kind) error, so the step enters the
/// retry loop and hits the backoff sleep before the next attempt.
struct AlwaysFail {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Action for AlwaysFail {
    fn id(&self) -> &'static str {
        "test.always_fail_backoff"
    }
    fn summary(&self) -> &'static str {
        "always fails to drive the retry backoff path"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(StepError::msg("boom"))
    }
}

// `initial_ms: 10000` makes the first backoff 10s — far longer than the ~50ms
// cancel delay, so a fix-less run would block for 10s. `times: 2` ensures the
// loop reaches the backoff sleep after the first failing attempt.
const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: retry-backoff-cancel }
spec:
  steps:
    - id: boom
      action: test.always_fail_backoff
      retry: { times: 2, initial_ms: 10000, on: [] }
"#;

#[tokio::test]
async fn cancel_during_retry_backoff_aborts_promptly() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail {
        calls: calls.clone(),
    });

    let token = CancelToken::new();
    let vm = FlowVm::new(reg, None).with_cancel(token.clone());

    let canceller = token.clone();
    tokio::spawn(async move {
        // Land the cancel while the VM is parked in the (10s) backoff sleep.
        tokio::time::sleep(Duration::from_millis(50)).await;
        canceller.cancel();
    });

    let start = Instant::now();
    let err = vm
        .run(&parse_str(FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("a cancel during backoff must abort the run");
    let elapsed = start.elapsed();

    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
    // Must return well before the 10s backoff would have elapsed.
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel during backoff should be prompt, took {elapsed:?}"
    );
    // Exactly one attempt ran (we cancelled during the backoff before retry).
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
