//! 步骤级错误处理(`on_error`)的集成覆盖。
//!
//! 三种处置:
//!
//! * `interrupt`(缺省)—— 失败即终止,与历史行为一致;
//! * `continue`  —— 记下错误继续执行;
//! * `retry`     —— 再试若干次。
//!
//! 外加 `output`:失败时给步骤输出兜一个默认值,避免下游引用到未定义值。
//!
//! 这些用例守住四件事:缺省零回归、continue 真的继续、retry 真的重跑、
//! output 真的被下游读到。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 总是失败,记录调用次数。
struct AlwaysFail {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Action for AlwaysFail {
    fn id(&self) -> &'static str {
        "test.on_error_fail"
    }
    fn summary(&self) -> &'static str {
        "always fails"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(StepError::msg("boom"))
    }
}

/// 前 N 次失败,之后成功 —— 用来证明 retry 真的重跑了动作。
struct FailThenOk {
    calls: Arc<AtomicUsize>,
    fail_times: usize,
}

#[async_trait]
impl Action for FailThenOk {
    fn id(&self) -> &'static str {
        "test.on_error_flaky"
    }
    fn summary(&self) -> &'static str {
        "fails then succeeds"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= self.fail_times {
            return Err(StepError::msg(format!("boom #{n}")));
        }
        Ok(ActionResult::from(json!({ "ok": true })))
    }
}

/// 记录收到的入参,用来验证 output 兜底值的下游可见性。
struct Recorder {
    seen: Arc<Mutex<Vec<Value>>>,
}

#[async_trait]
impl Action for Recorder {
    fn id(&self) -> &'static str {
        "test.on_error_recorder"
    }
    fn summary(&self) -> &'static str {
        "records its input"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        self.seen.lock().unwrap().push(input.clone());
        Ok(ActionResult::from(input))
    }
}

fn flow(body: &str) -> String {
    format!(
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: {{ id: on-error-test }}\nspec:\n  steps:\n{body}"
    )
}

/// 只注册失败动作的最小跑法。
async fn run_fail_only(src: &str, calls: Arc<AtomicUsize>) -> Result<(), String> {
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail { calls });
    let parsed = parse_str(src).expect("parse");
    let vm = FlowVm::new(reg, None);
    vm.run(&parsed, RunOptions::default())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// ─── interrupt(缺省) ────────────────────────────────────────────────────────

#[tokio::test]
async fn absent_on_error_keeps_fail_fast() {
    let calls = Arc::new(AtomicUsize::new(0));
    let src = flow("    - id: boom\n      action: test.on_error_fail\n");
    let res = run_fail_only(&src, calls.clone()).await;
    assert!(res.is_err(), "缺省必须失败即终止");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "只应执行一次");
}

#[tokio::test]
async fn explicit_interrupt_behaves_like_absent() {
    let calls = Arc::new(AtomicUsize::new(0));
    let src = flow(
        "    - id: boom\n      action: test.on_error_fail\n      on_error: { mode: interrupt }\n",
    );
    let res = run_fail_only(&src, calls.clone()).await;
    assert!(res.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// ─── continue ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn continue_runs_following_steps() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail {
        calls: calls.clone(),
    });
    reg.register(Recorder { seen: seen.clone() });
    let src = flow(concat!(
        "    - id: boom\n",
        "      action: test.on_error_fail\n",
        "      on_error: { mode: continue }\n",
        "    - id: after\n",
        "      action: test.on_error_recorder\n",
        "      with: { tag: reached }\n",
    ));
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_ok(), "continue 后流程应成功结束: {r:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "失败步骤只执行一次");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "后续步骤必须被执行到");
    assert_eq!(seen[0].get("tag"), Some(&json!("reached")));
}

#[tokio::test]
async fn continue_binds_fallback_output_for_downstream() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail {
        calls: calls.clone(),
    });
    reg.register(Recorder { seen: seen.clone() });
    let src = flow(concat!(
        "    - id: boom\n",
        "      action: test.on_error_fail\n",
        "      on_error: { mode: continue, output: { status: degraded } }\n",
        "    - id: after\n",
        "      action: test.on_error_recorder\n",
        "      with: { got: \"{{ steps.boom.result.status }}\" }\n",
    ));
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_ok(), "应继续执行: {r:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].get("got"),
        Some(&json!("degraded")),
        "下游应读到失败时的输出兜底值"
    );
}

#[tokio::test]
async fn continue_binds_null_when_no_fallback_configured() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail { calls });
    reg.register(Recorder { seen: seen.clone() });
    let src = flow(concat!(
        "    - id: boom\n",
        "      action: test.on_error_fail\n",
        "      on_error: { mode: continue }\n",
        "    - id: after\n",
        "      action: test.on_error_recorder\n",
        "      with: { got: \"{{ steps.boom.result }}\" }\n",
    ));
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_ok(), "应继续执行: {r:?}");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    // 关键:是**已定义**的 null,不是未定义。上游没配兜底值也不能让下游炸。
    assert_eq!(
        seen[0].get("got"),
        Some(&Value::Null),
        "未配兜底时应给已定义的 null"
    );
}

// ─── retry ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn retry_mode_reexecutes_the_action() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(FailThenOk {
        calls: calls.clone(),
        fail_times: 1,
    });
    let src = flow("    - id: flaky\n      action: test.on_error_flaky\n      on_error: { mode: retry, retry_count: 3, retry_interval_ms: 1 }\n");
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_ok(), "重试应把步骤救回来: {r:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "1 次失败 + 1 次成功");
}

#[tokio::test]
async fn retry_mode_gives_up_after_count() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(FailThenOk {
        calls: calls.clone(),
        fail_times: 99,
    });
    let src = flow("    - id: flaky\n      action: test.on_error_flaky\n      on_error: { mode: retry, retry_count: 2, retry_interval_ms: 1 }\n");
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_err(), "配额用尽后应失败");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "首次 + 2 次重试");
}

#[tokio::test]
async fn retry_count_zero_does_not_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(FailThenOk {
        calls: calls.clone(),
        fail_times: 99,
    });
    let src = flow("    - id: flaky\n      action: test.on_error_flaky\n      on_error: { mode: retry, retry_count: 0, retry_interval_ms: 1 }\n");
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1, "retry_count=0 等于不重试");
}

#[tokio::test]
async fn retry_mode_does_not_silently_downgrade_to_continue() {
    // retry 与 continue 是两种不同处置:重试都失败之后必须**失败**,
    // 不能悄悄降级成「忽略错误继续跑」。否则 `retry_count: 0` 等于完全
    // 忽略错误,与作者写下的意图正好相反 —— 而且是最危险的那种相反。
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysFail {
        calls: calls.clone(),
    });
    reg.register(Recorder { seen: seen.clone() });
    let src = flow(concat!(
        "    - id: boom\n",
        "      action: test.on_error_fail\n",
        "      on_error: { mode: retry, retry_count: 2, retry_interval_ms: 1 }\n",
        "    - id: after\n",
        "      action: test.on_error_recorder\n",
    ));
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_err(), "重试用尽后必须失败,不得降级为继续: {r:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "首次 + 2 次重试");
    assert_eq!(seen.lock().unwrap().len(), 0, "失败后不得继续执行后续步骤");
}

// ─── 与 retry 策略叠加 ──────────────────────────────────────────────────────

#[tokio::test]
async fn policy_retry_and_on_error_retry_keep_separate_budgets() {
    // retry.times=1 先重试(共 2 次执行),策略用尽后 on_error.retry_count=2
    // 再给 2 次 —— 两者配额独立,合计 4 次。
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(FailThenOk {
        calls: calls.clone(),
        fail_times: 99,
    });
    let src = flow(concat!(
        "    - id: flaky\n",
        "      action: test.on_error_flaky\n",
        "      retry: { times: 1, initial_ms: 1 }\n",
        "      on_error: { mode: retry, retry_count: 2, retry_interval_ms: 1 }\n",
    ));
    let parsed = parse_str(&src).expect("parse");
    let vm = FlowVm::new(reg, None);
    let r = vm.run(&parsed, RunOptions::default()).await;
    assert!(r.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 4, "策略 2 次 + on_error 2 次");
}

// ─── 校验 ───────────────────────────────────────────────────────────────────

fn validate_src(src: &str) -> Result<(), String> {
    let parsed = parse_str(src).expect("parse");
    lumo_dsl::validate(&parsed)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn invalid_mode_is_rejected_with_field_and_value() {
    let src = flow("    - id: boom\n      action: file.read\n      on_error: { mode: nope }\n");
    let err = validate_src(&src).expect_err("未知 mode 必须被拒绝");
    assert!(err.contains("on_error.mode"), "错误信息应点名字段: {err}");
    assert!(err.contains("nope"), "错误信息应回显非法值: {err}");
    assert!(err.contains("interrupt"), "错误信息应给出合法值: {err}");
}

#[test]
fn retry_count_with_continue_mode_is_rejected() {
    let src = flow("    - id: boom\n      action: file.read\n      on_error: { mode: continue, retry_count: 3 }\n");
    let err = validate_src(&src).expect_err("continue 配 retry_count 应被拒绝");
    assert!(err.contains("retry_count"), "got: {err}");
}

#[test]
fn all_three_modes_pass_validation() {
    for m in ["interrupt", "continue", "retry"] {
        let src = flow(&format!(
            "    - id: boom\n      action: file.read\n      on_error: {{ mode: {m} }}\n"
        ));
        assert!(validate_src(&src).is_ok(), "mode {m} 应合法");
    }
}

#[test]
fn mode_is_case_sensitive_like_retry_backoff() {
    let src =
        flow("    - id: boom\n      action: file.read\n      on_error: { mode: Interrupt }\n");
    assert!(
        validate_src(&src).is_err(),
        "大小写拼错必须报错,而不是静默按 interrupt 处理"
    );
}
