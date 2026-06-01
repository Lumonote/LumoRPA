//! B2 (F-17): an invalid `with:` must fail BEFORE the action executes.
//!
//! The action records whether it ran; a schema violation must short-circuit the
//! step so it never does, and the run must fail with a clear error.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Action whose schema requires a string `url` and forbids unknown fields.
struct Strict {
    ran: Arc<AtomicBool>,
}

#[async_trait]
impl Action for Strict {
    fn id(&self) -> &'static str {
        "test.strict"
    }
    fn summary(&self) -> &'static str {
        "requires url:string, additionalProperties:false"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": { "url": { "type": "string" } },
                "additionalProperties": false
            })
        })
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.ran.store(true, Ordering::SeqCst);
        Ok(ActionResult::null())
    }
}

const VALID: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: schema-valid }
spec:
  steps:
    - id: s
      action: test.strict
      with: { url: "https://example.com" }
"#;

const UNKNOWN_FIELD: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: schema-unknown }
spec:
  steps:
    - id: s
      action: test.strict
      with: { url: "https://example.com", bogus: 1 }
"#;

const MISSING_REQUIRED: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: schema-missing }
spec:
  steps:
    - id: s
      action: test.strict
      with: {}
"#;

/// Returns (run_ok, action_ran).
async fn run(flow_src: &str) -> (bool, bool) {
    let ran = Arc::new(AtomicBool::new(false));
    let mut reg = ActionRegistry::new();
    reg.register(Strict { ran: ran.clone() });
    let flow = parse_str(flow_src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let ok = vm.run(&flow, RunOptions::default()).await.is_ok();
    (ok, ran.load(Ordering::SeqCst))
}

#[tokio::test]
async fn valid_with_executes() {
    let (ok, ran) = run(VALID).await;
    assert!(ok, "a valid `with` must run successfully");
    assert!(ran, "the action must execute for a valid `with`");
}

#[tokio::test]
async fn unknown_field_rejected_before_execute() {
    let (ok, ran) = run(UNKNOWN_FIELD).await;
    assert!(!ok, "an unknown field must fail the run");
    assert!(!ran, "the action must NOT execute when `with` is invalid");
}

#[tokio::test]
async fn missing_required_rejected_before_execute() {
    let (ok, ran) = run(MISSING_REQUIRED).await;
    assert!(!ok, "a missing required field must fail the run");
    assert!(!ran, "the action must NOT execute when a required field is missing");
}
