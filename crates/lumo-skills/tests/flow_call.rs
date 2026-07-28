//! F-15: `flow.call` runs another flow *file* as a sub-flow.
//!
//! It mirrors `skill.invoke` (sub-VM on the same action registry, capabilities
//! clamped to the caller, recursion bounded by the shared `skill_depth`) but
//! sources the sub-flow from a `.yaml` file resolved **within an injected base
//! directory** — absolute paths and `..` escapes are rejected so a flow can
//! only call sibling flows under that base, never read arbitrary files.

use async_trait::async_trait;
use lumo_core::error::{ExecError, StepError};
use lumo_core::{
    Action, ActionRegistry, ActionResult, CancelToken, FlowVm, HumanPromptKind, HumanPromptRequest,
    HumanPrompter, HumanResponse, RunOptions, StepCtx, StepInterrupt,
};
use lumo_dsl::parse_str;
use lumo_skills::register_flow_call_action;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

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
    let parent = parse_str(&parent_calling(
        "sub.yaml",
        r#"{ msg: "hello-from-parent" }"#,
    ))
    .unwrap();

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
    assert_eq!(
        call["outputs"]["echo"]["result"]["echoed"],
        json!("hello-from-parent")
    );
}

#[tokio::test]
async fn flow_call_rejects_dotdot_escape() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("../sub.yaml", "{}")).unwrap();

    let res = FlowVm::new(reg, None)
        .run(&parent, RunOptions::default())
        .await;
    assert!(res.is_err(), "`..` path escape must fail the run");
    assert!(seen.lock().is_empty(), "no sub-flow should have executed");
}

#[tokio::test]
async fn flow_call_rejects_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen.clone());
    let parent = parse_str(&parent_calling("/etc/hosts", "{}")).unwrap();

    let res = FlowVm::new(reg, None)
        .run(&parent, RunOptions::default())
        .await;
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

    let res = FlowVm::new(reg, None)
        .run(&parent, RunOptions::default())
        .await;
    assert!(
        res.is_err(),
        "self-recursive flow.call must terminate with an error"
    );
    let msg = format!("{}", res.unwrap_err());
    assert!(
        msg.contains("recursion") || msg.contains("depth") || msg.contains("limit"),
        "error should explain the recursion bound, got: {msg}"
    );
}

// ─── 架构 P0-1:子 VM 经 FlowVm::child_of 继承父运行执行环境 ────────────────

/// 阻塞到被中断的动作:先把自己的 [`StepInterrupt`] 句柄递出去(证明子流程
/// 真的跑进了这一步),再轮询中断位 —— 模拟子流程里的长任务 /
/// `spawn_blocking` 孤儿。父运行取消后,该句柄必须翻转为已中断。
struct BlockUntilInterrupted {
    handle_tx: tokio::sync::mpsc::UnboundedSender<StepInterrupt>,
}

#[async_trait]
impl Action for BlockUntilInterrupted {
    fn id(&self) -> &'static str {
        "test.block"
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let si = ctx.step_interrupt();
        let _ = self.handle_tx.send(si.clone());
        loop {
            if si.is_interrupted() {
                return Err(StepError::msg("interrupted at checkpoint"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

const BLOCKING_SUB_YAML: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: subblock }
spec:
  steps:
    - { id: block, action: test.block, with: {} }
    - id: after
      action: test.echo
      with: { value: "must-not-run" }
"#;

#[tokio::test]
async fn parent_cancel_interrupts_flow_call_subflow() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sub.yaml"), BLOCKING_SUB_YAML).unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = registry(dir.path().to_path_buf(), seen.clone());
    reg.register(BlockUntilInterrupted { handle_tx: tx });
    let parent = parse_str(&parent_calling("sub.yaml", "{}")).unwrap();

    let token = CancelToken::new();
    let vm = FlowVm::new(reg, None).with_cancel(token.clone());
    // 同任务上并跑:右臂等子流程跑进 block 步骤、拿到其步级中断句柄后取消。
    let (run_res, si) = tokio::join!(vm.run(&parent, RunOptions::default()), async {
        let si = rx.recv().await.expect("sub-flow must reach its block step");
        assert!(
            !si.is_interrupted(),
            "before cancel the sub-flow step must be live"
        );
        token.cancel();
        si
    });

    let err = run_res.expect_err("cancelled parent run must fail");
    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
    // 关键断言:父运行级取消穿透到子流程动作的 StepInterrupt(同一令牌)。
    assert!(
        si.is_interrupted(),
        "parent cancel must flip the sub-flow step's StepInterrupt"
    );
    assert!(
        seen.lock().is_empty(),
        "no sub-flow step after the cancel point may execute"
    );
}

/// 子流程内真的走一轮 human 提示:无 prompter 即报错,有则回显宿主答复。
struct AskHuman;

#[async_trait]
impl Action for AskHuman {
    fn id(&self) -> &'static str {
        "test.ask"
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let prompter = ctx
            .human_prompter()
            .ok_or_else(|| StepError::msg("no human prompter in sub-flow ctx"))?;
        let resp = prompter
            .prompt(HumanPromptRequest {
                kind: HumanPromptKind::Input,
                message: "?".into(),
                default: None,
                timeout_ms: 1_000,
                run_id: ctx.run_id().to_string(),
                step_path: ctx.current_step_path().unwrap_or_default(),
            })
            .await?;
        Ok(ActionResult::from(json!({ "answer": resp.value })))
    }
}

struct StubPrompter;

#[async_trait]
impl HumanPrompter for StubPrompter {
    async fn prompt(&self, _req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        Ok(HumanResponse {
            value: json!("stub-answer"),
            by: None,
            comment: None,
        })
    }
}

const ASK_SUB_YAML: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: subask }
spec:
  steps:
    - { id: ask, action: test.ask, with: {} }
"#;

#[tokio::test]
async fn flow_call_subflow_inherits_human_prompter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sub.yaml"), ASK_SUB_YAML).unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = registry(dir.path().to_path_buf(), seen);
    reg.register(AskHuman);
    let parent = parse_str(&parent_calling("sub.yaml", "{}")).unwrap();

    let report = FlowVm::new(reg, None)
        .with_human_prompter(Some(Arc::new(StubPrompter)))
        .run(&parent, RunOptions::default())
        .await
        .expect("sub-flow with inherited prompter must succeed");
    assert!(report.success);
    let out = report.outputs.expect("outputs");
    assert_eq!(
        out["call_sub"]["result"]["outputs"]["ask"]["result"]["answer"],
        json!("stub-answer"),
        "sub-flow human.* must reach the host prompter, got: {out}"
    );
}

#[tokio::test]
async fn flow_call_sub_run_persists_with_inherited_repo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sub.yaml"), SUB_YAML).unwrap();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = registry(dir.path().to_path_buf(), seen);
    let repo = lumo_storage::Repo::open_in_memory().expect("repo");
    let parent = parse_str(&parent_calling("sub.yaml", r#"{ msg: "hi" }"#)).unwrap();

    let report = FlowVm::new(reg, Some(repo.clone()))
        .run(&parent, RunOptions::default())
        .await
        .expect("run ok");
    assert!(report.success);

    let runs = repo.list_runs(10).expect("list runs");
    assert_eq!(runs.len(), 2, "parent + sub-flow each persist a run");
    assert!(
        runs.iter()
            .any(|r| r.trigger_kind == "flow.call:sub.yaml" && r.state == "ok"),
        "sub run row must carry its flow.call trigger kind, got: {runs:?}"
    );
}
