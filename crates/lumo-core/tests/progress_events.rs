//! P2-7(进度事件)+ P1-3(vars_json 去重/截断)的引擎侧端到端契约,走真实
//! VM 路径(渲染 → 校验 → 派发 → persist):
//!
//! * `FlowVm::with_on_step`:每个执行的步骤发 StepStarted → StepFinished,
//!   `ctx.log`(control.log 通道)发 Log 事件;不注册观察者行为不变。
//! * persist 侧 vars_json:vars 未变的行落 NULL(库内),`list_steps` 读取时
//!   回溯补齐;超过 `VARS_JSON_MAX_BYTES` 的快照落 `{"__truncated__": …}`。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{
    Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx, StepEvent,
    VARS_JSON_MAX_BYTES,
};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};

/// No-op leaf action that echoes its `with:` back as output.
struct Echo;

#[async_trait]
impl Action for Echo {
    fn id(&self) -> &'static str {
        "test.echo"
    }
    fn summary(&self) -> &'static str {
        "echoes its input back as output"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::from(input))
    }
}

/// Writes one line through `ctx.log` — the same channel `control.log` uses.
struct LogLine;

#[async_trait]
impl Action for LogLine {
    fn id(&self) -> &'static str {
        "test.logline"
    }
    fn summary(&self) -> &'static str {
        "writes a line to the run log"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        ctx.log("hello from the flow");
        Ok(ActionResult::from(Value::Null))
    }
}

/// Returns a string bigger than the vars_json cap, so `bind:` blows the snapshot
/// past `VARS_JSON_MAX_BYTES`.
struct BigOut;

#[async_trait]
impl Action for BigOut {
    fn id(&self) -> &'static str {
        "test.bigout"
    }
    fn summary(&self) -> &'static str {
        "returns an oversized string"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| serde_json::json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        Ok(ActionResult::from(Value::String(
            "x".repeat(VARS_JSON_MAX_BYTES + 1024),
        )))
    }
}

fn registry() -> ActionRegistry {
    let mut reg = ActionRegistry::new();
    reg.register(Echo);
    reg.register(LogLine);
    reg.register(BigOut);
    reg
}

fn flow(yaml_steps: &str) -> lumo_dsl::Flow {
    let src = format!(
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: {{ id: progress-events-test }}\nspec:\n  steps:\n{yaml_steps}"
    );
    parse_str(&src).expect("parse flow")
}

/// One `test.echo` step, optional `bind:`.
fn echo_step(id: &str, n: i64, bind: Option<&str>) -> String {
    let mut s = format!("    - id: {id}\n      action: test.echo\n      with: {{ n: {n} }}\n");
    if let Some(b) = bind {
        s.push_str(&format!("      bind: {b}\n"));
    }
    s
}

#[tokio::test]
async fn on_step_observer_sees_started_finished_and_log_events() {
    let flow = flow(&format!(
        "{}    - id: s1\n      action: test.logline\n{}",
        echo_step("s0", 0, Some("v")),
        echo_step("s2", 2, None),
    ));
    let events: Arc<Mutex<Vec<StepEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();

    let vm = FlowVm::new(registry(), Some(Repo::open_in_memory().unwrap())).with_on_step(Some(
        Arc::new(move |e: &StepEvent| {
            sink.lock().unwrap().push(e.clone());
        }),
    ));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success);

    let got = events.lock().unwrap();
    let brief: Vec<String> = got
        .iter()
        .map(|e| match e {
            StepEvent::StepStarted { path, .. } => format!("start:{path}"),
            StepEvent::StepFinished { path, state, .. } => format!("finish:{path}:{state}"),
            StepEvent::Log { line, .. } => format!("log:{line}"),
        })
        .collect();
    assert_eq!(
        brief,
        vec![
            "start:s0",
            "finish:s0:ok",
            "start:s1",
            "log:hello from the flow",
            "finish:s1:ok",
            "start:s2",
            "finish:s2:ok",
        ],
        "full event stream: {brief:?}"
    );

    // 事件字段逐项:run_id 一致、action 就位、Log 归到发出它的步骤路径。
    for e in got.iter() {
        let rid = match e {
            StepEvent::StepStarted { run_id, .. }
            | StepEvent::StepFinished { run_id, .. }
            | StepEvent::Log { run_id, .. } => run_id,
        };
        assert_eq!(rid, &report.run_id);
    }
    assert!(matches!(
        &got[0],
        StepEvent::StepStarted { action, step_id, .. } if action == "test.echo" && step_id == "s0"
    ));
    assert!(matches!(
        &got[3],
        StepEvent::Log { step_path: Some(p), .. } if p == "s1"
    ));
    assert!(matches!(
        &got[1],
        StepEvent::StepFinished {
            attempt: 1,
            error: None,
            ..
        }
    ));
}

#[tokio::test]
async fn without_observer_behavior_is_unchanged() {
    // 向后兼容:不设回调,流程照跑、照落库。
    let flow = flow(&echo_step("s0", 0, None));
    let repo = Repo::open_in_memory().unwrap();
    let vm = FlowVm::new(registry(), Some(repo.clone()));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success);
    assert_eq!(repo.list_steps(&report.run_id).unwrap().len(), 1);
}

#[tokio::test]
async fn vars_json_dedups_unchanged_steps_and_backfills_on_read() {
    // s0 bind ⇒ vars 变更(全量);s1/s2 不动 vars(库内 NULL);s3 bind ⇒ 全量。
    let flow = flow(&format!(
        "{}{}{}{}",
        echo_step("s0", 0, Some("v")),
        echo_step("s1", 1, None),
        echo_step("s2", 2, None),
        echo_step("s3", 3, Some("w")),
    ));
    let repo = Repo::open_in_memory().unwrap();
    let vm = FlowVm::new(registry(), Some(repo.clone()));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success);

    // 库内原始形态:首行全量,未变行 NULL,再变更行重新全量。
    let raw: Vec<Option<String>> = repo
        .with_raw(|c| {
            let mut stmt =
                c.prepare("SELECT vars_json FROM step_runs WHERE flow_run_id=?1 ORDER BY seq ASC")?;
            let rows = stmt.query_map([&report.run_id], |r| r.get::<_, Option<String>>(0))?;
            rows.collect()
        })
        .unwrap();
    assert_eq!(raw.len(), 4);
    assert!(
        raw[0].is_some(),
        "first persisted row writes the full snapshot"
    );
    assert!(raw[1].is_none(), "unchanged vars dedup to NULL");
    assert!(raw[2].is_none(), "unchanged vars dedup to NULL");
    assert!(
        raw[3].is_some(),
        "bind (set_var) re-writes the full snapshot"
    );

    // 读取侧:每行都能看到正确的快照(NULL 已回溯补齐)。
    let steps = repo.list_steps(&report.run_id).unwrap();
    let v0 = serde_json::json!({ "v": { "n": 0 } });
    assert_eq!(steps[0].vars_json, Some(v0.clone()));
    assert_eq!(steps[1].vars_json, Some(v0.clone()), "backfilled from s0");
    assert_eq!(steps[2].vars_json, Some(v0), "backfilled from s0");
    assert_eq!(
        steps[3].vars_json,
        Some(serde_json::json!({ "v": { "n": 0 }, "w": { "n": 3 } }))
    );
}

#[tokio::test]
async fn oversized_vars_snapshot_is_stored_as_truncation_marker() {
    let flow = flow("    - id: big\n      action: test.bigout\n      bind: blob\n");
    let repo = Repo::open_in_memory().unwrap();
    let vm = FlowVm::new(registry(), Some(repo.clone()));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success);

    let steps = repo.list_steps(&report.run_id).unwrap();
    let vars = steps[0].vars_json.as_ref().expect("marker stored");
    assert_eq!(vars["__truncated__"], Value::Bool(true));
    assert!(
        vars["bytes"].as_u64().unwrap() as usize > VARS_JSON_MAX_BYTES,
        "recorded size reflects the oversized snapshot"
    );
    assert!(
        vars.get("blob").is_none(),
        "the oversized payload itself is not persisted"
    );
}
