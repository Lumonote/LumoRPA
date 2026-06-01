//! B3 (F-16): `retry.on` filters retries by error kind.
//!
//! An empty `on` retries on any error (back-compat with the pre-F-16 loop);
//! a non-empty `on` retries only when the failed step's error kind matches one
//! of the listed names (the snake_case `ErrorKind` spellings). A non-matching
//! error is terminal on the first attempt — no retry, no backoff sleep.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Action that always fails with a `Message` error (`ErrorKind::Other`),
/// counting how many times it is invoked so a test can assert retry behavior.
struct AlwaysFailOther {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Action for AlwaysFailOther {
    fn id(&self) -> &'static str {
        "test.always_fail_other"
    }
    fn summary(&self) -> &'static str {
        "always fails with a generic (Other-kind) error"
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

fn flow_with_retry(on: &str) -> String {
    // `initial_ms: 1` keeps the (back-compat) backoff sleeps negligible.
    format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: retry-on-test }}
spec:
  steps:
    - id: boom
      action: test.always_fail_other
      retry: {{ times: 2, initial_ms: 1, on: {on} }}
"#
    )
}

/// Run the flow (expected to fail) and return how many times the action ran.
async fn count_attempts(on: &str) -> usize {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFailOther {
        calls: calls.clone(),
    });
    let flow = parse_str(&flow_with_retry(on)).expect("parse");
    let vm = FlowVm::new(reg, None);
    let _ = vm.run(&flow, RunOptions::default()).await; // always fails; we count attempts
    calls.load(Ordering::SeqCst)
}

#[tokio::test]
async fn retry_on_matching_kind_retries() {
    // Error kind is Other; on:["other"] matches → retries `times` (2) → 3 calls.
    assert_eq!(count_attempts(r#"["other"]"#).await, 3);
}

#[tokio::test]
async fn retry_on_nonmatching_kind_does_not_retry() {
    // Error kind is Other; on:["selector_not_found"] does NOT match → terminal
    // on the first attempt → exactly 1 call.
    assert_eq!(count_attempts(r#"["selector_not_found"]"#).await, 1);
}

#[tokio::test]
async fn retry_on_empty_retries_on_any_error() {
    // Back-compat: an empty `on` retries on any error → 3 calls.
    assert_eq!(count_attempts("[]").await, 3);
}
