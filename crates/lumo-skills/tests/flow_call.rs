//! F-15: `flow.call` runs another flow *file* as a sub-flow.
//!
//! It mirrors `skill.invoke` (sub-VM on the same action registry, capabilities
//! clamped to the caller, recursion bounded by the shared `skill_depth`) but
//! sources the sub-flow from a `.yaml` file resolved **within an injected base
//! directory** — absolute paths and `..` escapes are rejected so a flow can
//! only call sibling flows under that base, never read arbitrary files.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_skills::register_flow_call_action;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};

/// Test leaf action `test.echo`: records every `value` it receives (so a test
/// can prove the sub-flow ran with propagated inputs) and echoes it back.
struct CaptureEcho {
    seen: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl Action for CaptureEcho {
    fn id(&self) -> &'static str {
        "test.echo"
    }
    fn summary(&self) -> &'static str {
        "capture + echo the `value` input"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let v = input.get("value").cloned().unwrap_or(Value::Null);
        self.seen.lock().push(v.clone());
        Ok(ActionResult::from(json!({ "echoed": v })))
    }
}

const SUB_YAML: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: sub }
spec:
  steps:
    - id: echo
      action: test.echo
      with: { value: "{{ inputs.msg }}" }
"#;

fn parent_calling(path: &str, inputs: &str) -> String {
    format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: parent }}
spec:
  steps:
    - id: call_sub
      action: flow.call
      with: {{ path: "{path}", inputs: {inputs} }}
"#
    )
}

fn registry(base: std::path::PathBuf, seen: Arc<Mutex<Vec<Value>>>) -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    reg.register(CaptureEcho { seen });
    register_flow_call_action(&mut reg, base);
    reg
}

#[tokio::test]
async fn flow_call_runs_subflow_and_propagates_inputs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sub.yaml"), SUB_YAML).unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("sub.yaml", r#"{ msg: "hello-from-parent" }"#)).unwrap();

    let report = FlowVm::new(reg, None)
        .run(&parent, RunOptions::default())
        .await
        .expect("parent run ok");

    assert!(report.success, "parent (and sub) flow should succeed");
    assert_eq!(
        seen.lock().clone(),
        vec![Value::String("hello-from-parent".into())],
        "sub-flow must run exactly once with the propagated input"
    );

    // flow.call's own result surfaces the sub-run's id + outputs.
    let out = report.outputs.expect("outputs");
    let call = &out["call_sub"]["result"];
    assert_eq!(call["success"], json!(true));
    assert_eq!(call["flow"], json!("sub.yaml"));
    assert_eq!(call["outputs"]["echo"]["result"]["echoed"], json!("hello-from-parent"));
}

#[tokio::test]
async fn flow_call_rejects_dotdot_escape() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("../sub.yaml", "{}")).unwrap();

    let res = FlowVm::new(reg, None).run(&parent, RunOptions::default()).await;
    assert!(res.is_err(), "`..` path escape must fail the run");
    assert!(seen.lock().is_empty(), "no sub-flow should have executed");
}

#[tokio::test]
async fn flow_call_rejects_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("/etc/hosts", "{}")).unwrap();

    let res = FlowVm::new(reg, None).run(&parent, RunOptions::default()).await;
    assert!(res.is_err(), "absolute path must fail the run");
    assert!(seen.lock().is_empty(), "no sub-flow should have executed");
}

#[tokio::test]
async fn flow_call_bounds_recursion_depth() {
    let dir = tempfile::tempdir().unwrap();
    // self.yaml calls itself → unbounded without the depth guard.
    std::fs::write(
        dir.path().join("self.yaml"),
        parent_calling("self.yaml", "{}").replace("id: parent", "id: selfref"),
    )
    .unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("self.yaml", "{}")).unwrap();

    let res = FlowVm::new(reg, None).run(&parent, RunOptions::default()).await;
    assert!(res.is_err(), "self-recursive flow.call must terminate with an error");
    let msg = format!("{}", res.unwrap_err());
    assert!(
        msg.contains("recursion") || msg.contains("depth") || msg.contains("limit"),
        "error should explain the recursion bound, got: {msg}"
    );
}
