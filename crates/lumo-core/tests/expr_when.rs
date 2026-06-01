//! B1 (F-14): the VM evaluates bare-expression `when:` and `control.if` conds
//! through the expression evaluator — not only `{{ }}` templates. Proves the
//! wiring in `execute_step` (when) and `run_if` (control.if).

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Counts how many times it executes, so a test can tell "ran" from "skipped".
struct Mark {
    hits: Arc<AtomicUsize>,
}

#[async_trait]
impl Action for Mark {
    fn id(&self) -> &'static str {
        "test.mark"
    }
    fn summary(&self) -> &'static str {
        "counts executions"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Ok(ActionResult::null())
    }
}

const WHEN_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: when-test }
spec:
  inputs: [{ name: n, type: integer }]
  steps:
    - id: maybe
      action: test.mark
      when: "inputs.n > 3"
"#;

const IF_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: if-test }
spec:
  inputs: [{ name: n, type: integer }]
  steps:
    - id: branch
      action: control.if
      with: { cond: "inputs.n > 3" }
      do:
        - id: hit
          action: test.mark
"#;

/// Run `flow_src` with `inputs.n = n` and report how many times `test.mark` ran.
async fn run_with(flow_src: &str, n: i64) -> usize {
    let hits = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(Mark { hits: hits.clone() });
    let flow = parse_str(flow_src).expect("parse");
    let vm = FlowVm::new(reg, None);
    vm.run(
        &flow,
        RunOptions {
            inputs: json!({ "n": n }),
            ..Default::default()
        },
    )
    .await
    .expect("run");
    hits.load(Ordering::SeqCst)
}

#[tokio::test]
async fn when_bare_expression_true_runs() {
    // `inputs.n > 3` is a bare expression (no `{{ }}`); with n=5 it is true.
    assert_eq!(run_with(WHEN_FLOW, 5).await, 1);
}

#[tokio::test]
async fn when_bare_expression_false_skips() {
    // With n=1 the bare expression is false → the step is skipped, never runs.
    // (Pre-F-14 this wrongly ran: any non-empty `when` string was truthy.)
    assert_eq!(run_with(WHEN_FLOW, 1).await, 0);
}

#[tokio::test]
async fn control_if_bare_expression_true_takes_do() {
    assert_eq!(run_with(IF_FLOW, 5).await, 1);
}

#[tokio::test]
async fn control_if_bare_expression_false_skips_do() {
    assert_eq!(run_with(IF_FLOW, 1).await, 0);
}
