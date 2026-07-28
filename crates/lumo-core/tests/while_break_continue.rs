//! 指令集 P1:`control.while` / `control.break` / `control.continue` 的 VM 语义。
//!
//! 契约要点(与 cancel_timeout.rs 的硬中断契约平行):
//! * while 每轮用 F-14 求值器重新求值 cond,假则退出;到 max_iterations 仍真则报错;
//! * break / continue 是循环控制信号:向上 unwind 到最近的循环容器被消化,
//!   穿过 `control.try` 时不得被 catch 捕获(但 finally 仍执行);
//! * parallel 分支是独立作用域,break 不能跨出分支去"替"外层循环做决定;
//! * 循环外使用 break 在运行期兜底报错(静态拦截见 lumo-dsl validate)。

use async_trait::async_trait;
use lumo_actions::register_all;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// 计数 action:统计自己被执行了几次,用来区分"跑了几轮"。
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

fn vm_with_mark(hits: Arc<AtomicUsize>) -> FlowVm {
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    reg.register(Mark { hits });
    FlowVm::new(reg, None)
}

/// 跑一个 flow,返回 (RunReport, test.mark 执行次数)。
async fn run_ok(yaml: &str) -> (lumo_core::RunReport, usize) {
    let hits = Arc::new(AtomicUsize::new(0));
    let vm = vm_with_mark(hits.clone());
    let report = vm
        .run(&parse_str(yaml).expect("parse"), RunOptions::default())
        .await
        .expect("run should succeed");
    (report, hits.load(Ordering::SeqCst))
}

/// 跑一个预期失败的 flow,返回 (错误信息, test.mark 执行次数)。
async fn run_err(yaml: &str) -> (String, usize) {
    let hits = Arc::new(AtomicUsize::new(0));
    let vm = vm_with_mark(hits.clone());
    let err = vm
        .run(&parse_str(yaml).expect("parse"), RunOptions::default())
        .await
        .expect_err("run should fail");
    (err.to_string(), hits.load(Ordering::SeqCst))
}

fn iterations_of(report: &lumo_core::RunReport, step_id: &str) -> i64 {
    report
        .outputs
        .as_ref()
        .expect("outputs")
        .pointer(&format!("/{step_id}/result/iterations"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("step `{step_id}` should record iterations"))
}

// ─── control.while 基本语义 ─────────────────────────────────────────────────

#[tokio::test]
async fn while_exits_when_cond_turns_false() {
    // 第 2 轮(index==2)置 done=true,下一次 cond 求值为假 → 共 3 轮。
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: init, action: control.set_var, with: { name: done, value: false } }
    - id: loop
      action: control.while
      with: { cond: "!vars.done" }
      do:
        - { id: mark, action: test.mark }
        - id: gate
          action: control.if
          with: { cond: "index >= 2" }
          do:
            - { id: stop, action: control.set_var, with: { name: done, value: true } }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 3, "body should run exactly 3 rounds");
    assert_eq!(iterations_of(&report, "loop"), 3);
}

#[tokio::test]
async fn while_runs_zero_rounds_when_cond_initially_false() {
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: init, action: control.set_var, with: { name: done, value: true } }
    - id: loop
      action: control.while
      with: { cond: "!vars.done" }
      do:
        - { id: mark, action: test.mark }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 0, "cond 一直为假 → 零轮");
    assert_eq!(iterations_of(&report, "loop"), 0);
}

#[tokio::test]
async fn while_hitting_max_iterations_errors() {
    // cond 恒真 + max_iterations=3:第 4 次 cond 求值后触发防呆死循环报错。
    let (err, hits) = run_err(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.while
      with: { cond: "true", max_iterations: 3 }
      do:
        - { id: mark, action: test.mark }
"#,
    )
    .await;
    assert!(err.contains("max_iterations=3"), "got: {err}");
    assert_eq!(
        hits, 3,
        "body runs exactly max_iterations rounds before erroring"
    );
}

// ─── break:三种循环容器各消化一次 ──────────────────────────────────────────

#[tokio::test]
async fn break_exits_while() {
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.while
      with: { cond: "true" }
      do:
        - { id: mark, action: test.mark }
        - { id: out, action: control.break }
    - { id: after, action: test.mark }
"#,
    )
    .await;
    assert!(report.success, "break 被 while 消化,不是失败");
    assert_eq!(hits, 2, "body 跑一轮 + 循环后的 after 步");
}

#[tokio::test]
async fn break_exits_for() {
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for
      with: { from: 0, to: 5 }
      do:
        - { id: mark, action: test.mark }
        - id: gate
          action: control.if
          with: { cond: "index >= 2" }
          do:
            - { id: out, action: control.break }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 3, "rounds 0/1/2 run, break at index 2 stops the rest");
}

#[tokio::test]
async fn break_exits_for_each() {
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [a, b, c, d] }
      do:
        - { id: mark, action: test.mark }
        - id: gate
          action: control.if
          with: { cond: "index >= 1" }
          do:
            - { id: out, action: control.break }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 2, "items a/b run, break on b stops c/d");
}

// ─── continue:跳轮且轮次照常推进 ───────────────────────────────────────────

#[tokio::test]
async fn continue_skips_round_and_index_advances() {
    // index==1 的那轮 continue 跳过 mark;游标照常推进,不会死循环。
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for
      with: { from: 0, to: 4 }
      do:
        - id: gate
          action: control.if
          with: { cond: "index == 1" }
          do:
            - { id: skip, action: control.continue }
        - { id: mark, action: test.mark }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 3, "rounds 0/2/3 reach mark; round 1 is skipped");
    assert_eq!(
        iterations_of(&report, "loop"),
        4,
        "continue 的那轮也计入轮次"
    );
}

// ─── 契约钉子:break 穿过 try 不被 catch 捕获 ───────────────────────────────

#[tokio::test]
async fn break_inside_try_is_not_caught() {
    // 照 timeout_inside_try_is_not_caught 的写法:catch 里放 control.fail,
    // 若 break 被 catch 吞掉,run 会以 "catch-must-not-run" 失败。finally
    // (清理语义)仍要执行——用 test.mark 证明。
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.while
      with: { cond: "true" }
      do:
        - id: guard
          action: control.try
          with: {}
          do:
            - { id: out, action: control.break }
          catch:
            - { id: bomb, action: control.fail, with: { message: "catch-must-not-run" } }
          finally:
            - { id: fin, action: test.mark }
"#,
    )
    .await;
    assert!(
        report.success,
        "break 穿过 try 后由 while 消化,run 必须成功"
    );
    assert_eq!(hits, 1, "finally 必须执行恰好一次(catch 不得执行)");
}

// ─── 嵌套循环:break 只退最近一层 ───────────────────────────────────────────

#[tokio::test]
async fn nested_break_only_exits_innermost_loop() {
    let (report, hits) = run_ok(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: outer
      action: control.for
      with: { from: 0, to: 2, bind: o }
      do:
        - id: inner
          action: control.for_each
          with: { in: [x, y, z] }
          do:
            - { id: mark, action: test.mark }
            - { id: out, action: control.break }
"#,
    )
    .await;
    assert!(report.success);
    assert_eq!(hits, 2, "inner 每次首元素即 break,outer 仍完整跑 2 轮");
    assert_eq!(iterations_of(&report, "outer"), 2, "break 不得波及外层循环");
}

// ─── 运行期兜底:循环外 / parallel 分支边界 ────────────────────────────────

#[tokio::test]
async fn break_at_top_level_is_a_runtime_error() {
    // validate 会静态拦截;VM 不跑 validate,这里钉运行期兜底路径。
    let (err, _) = run_err(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: out, action: control.break }
"#,
    )
    .await;
    assert!(err.contains("outside of a loop"), "got: {err}");
}

#[tokio::test]
async fn break_in_parallel_branch_does_not_cross_to_outer_loop() {
    // parallel 分支是独立作用域:分支内的 break 没有分支内循环祖先,即使
    // parallel 本身套在 for 里,也必须报错而不是替外层循环退出。
    let (err, _) = run_err(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: outer
      action: control.for
      with: { from: 0, to: 2 }
      do:
        - id: par
          action: control.parallel
          do:
            - { id: out, action: control.break }
"#,
    )
    .await;
    assert!(
        err.contains("control.break") && err.contains("分支"),
        "break 必须在分支边界降级为错误,不得穿透到外层循环: {err}"
    );
}
