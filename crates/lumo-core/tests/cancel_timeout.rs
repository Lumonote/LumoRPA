//! P1-1: the VM supports cooperative cancellation (a `CancelToken` checked
//! before each step and able to interrupt an in-flight step) and a per-step
//! timeout. A cancelled run reports `ExecError::Cancelled`; a step that exceeds
//! the timeout reports `ExecError::Timeout`.

use async_trait::async_trait;
use lumo_core::error::{ExecError, StepError};
use lumo_core::{
    Action, ActionRegistry, ActionResult, CancelToken, FlowVm, HumanPromptRequest, HumanPrompter,
    HumanResponse, RunOptions, StepCtx,
};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
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

/// A slow step guarded by `control.try` with a `catch:`. Used to prove that a
/// hard interrupt (cancel / per-step timeout) inside the `do:` block is NOT
/// swallowed by the catch — the run must still abort.
const GUARDED_SLEEP_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: guard
      action: control.try
      with: {}
      do:
        - { id: slow, action: test.sleep, with: { ms: 600 } }
      catch:
        - { id: rescue, action: test.sleep, with: { ms: 0 } }
      finally:
        - { id: fin, action: test.sleep, with: { ms: 0 } }
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

#[tokio::test]
async fn timeout_inside_try_is_not_caught() {
    // P1-1: a per-step timeout is a *hard* ceiling, not a catchable failure. A
    // `control.try` wrapping the timed-out step must NOT swallow the timeout via
    // its `catch:` — the run must abort with `Timeout`, exactly as if there were
    // no try. (Regression: `run_try` used to stringify *any* error and run the
    // catch branch, silently recovering from a timeout and continuing the run.)
    let vm = FlowVm::new(reg(), None).with_step_timeout(Duration::from_millis(40));
    let err = vm
        .run(
            &parse_str(GUARDED_SLEEP_FLOW).unwrap(),
            RunOptions::default(),
        )
        .await
        .expect_err("a timeout inside try must propagate, not be caught");
    assert!(matches!(err, ExecError::Timeout { .. }), "got: {err}");
    assert!(err.to_string().contains("timed out"));
}

#[tokio::test]
async fn cancel_inside_try_is_not_caught() {
    // Cancellation is likewise a hard interrupt: a `control.try` around a step
    // that gets cancelled mid-flight must abort the run with `Cancelled`, never
    // route through the `catch:` branch.
    let token = CancelToken::new();
    let vm = FlowVm::new(reg(), None).with_cancel(token.clone());
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        canceller.cancel();
    });
    let err = vm
        .run(
            &parse_str(GUARDED_SLEEP_FLOW).unwrap(),
            RunOptions::default(),
        )
        .await
        .expect_err("cancellation inside try must abort the run, not be caught");
    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
}

// ─── P0-2:协作中断 + 可重试超时 ─────────────────────────────────────────────
//
// `select!` 超时只能 drop 动作 future,动作里 `spawn_blocking` 的孤儿阻塞任务
// 不会随之停止。引擎判死后翻步级中断位,阻塞闭包在检查点观察到它提前退出,
// 写回(commit)不落地;`retry: {on: [timeout]}` 显式声明时超时可重试。

/// 模拟"长阻塞计算 + 末尾写回"的动作:`spawn_blocking` 里循环 sleep,每轮
/// 检查 `StepInterrupt`;只有完整跑完才置 `committed`(对应 db commit /
/// excel 写回这类副作用落地点)。
struct BlockingCommit {
    committed: Arc<AtomicBool>,
    saw_interrupt: Arc<AtomicBool>,
}
#[async_trait]
impl Action for BlockingCommit {
    fn id(&self) -> &'static str {
        "test.blocking_commit"
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let interrupt = ctx.step_interrupt();
        let committed = self.committed.clone();
        let saw = self.saw_interrupt.clone();
        tokio::task::spawn_blocking(move || {
            for _ in 0..100 {
                if interrupt.is_interrupted() {
                    saw.store(true, Ordering::SeqCst);
                    return Err(StepError::msg("interrupted at checkpoint"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            committed.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .map_err(|e| StepError::msg(e.to_string()))??;
        Ok(ActionResult::null())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_blocking_task_stops_at_checkpoint_without_committing() {
    let committed = Arc::new(AtomicBool::new(false));
    let saw = Arc::new(AtomicBool::new(false));
    let mut reg = ActionRegistry::new();
    reg.register(BlockingCommit {
        committed: committed.clone(),
        saw_interrupt: saw.clone(),
    });
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: blk, action: test.blocking_commit, with: {} }
"#,
    )
    .unwrap();
    let vm = FlowVm::new(reg, None).with_step_timeout(Duration::from_millis(40));
    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("the blocking step must hit the 40ms timeout");
    assert!(matches!(err, ExecError::Timeout { .. }), "got: {err}");
    // future 已被 drop,孤儿阻塞任务还活着 —— 轮询等它走到下一个检查点退出
    // (最坏一轮 20ms;给足余量,避免慢 CI 误报)。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !saw.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        saw.load(Ordering::SeqCst),
        "blocking task must observe the interrupt at a checkpoint"
    );
    assert!(
        !committed.load(Ordering::SeqCst),
        "side effect must NOT land after the step was judged timed out"
    );
}

/// 第一次调用睡过超时线,之后立刻成功 —— 用于验证 `retry: {on: [timeout]}`。
struct SlowThenQuick {
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Action for SlowThenQuick {
    fn id(&self) -> &'static str {
        "test.slow_then_quick"
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        Ok(ActionResult::null())
    }
}

fn retry_flow(retry: &str) -> String {
    format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: t }}
spec:
  steps:
    - id: flaky
      action: test.slow_then_quick
      with: {{}}
      retry: {retry}
"#
    )
}

#[tokio::test]
async fn retry_on_timeout_retries_and_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(SlowThenQuick {
        calls: calls.clone(),
    });
    let repo = Repo::open_in_memory().expect("repo");
    let flow = parse_str(&retry_flow("{ times: 2, initial_ms: 1, on: [timeout] }")).unwrap();
    let vm = FlowVm::new(reg, Some(repo.clone())).with_step_timeout(Duration::from_millis(40));
    let report = vm
        .run(&flow, RunOptions::default())
        .await
        .expect("attempt 2 succeeds, so the run must succeed");
    assert!(report.success);
    assert_eq!(calls.load(Ordering::SeqCst), 2, "exactly one retry");
    // 落库轨迹:attempt 1 记 retrying(降级后的可重试超时),attempt 2 记 ok。
    let rows = repo.list_steps(&report.run_id).expect("list steps");
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert_eq!(states, vec!["retrying", "ok"], "rows: {rows:?}");
    assert!(
        rows[0].error.as_deref().unwrap_or("").contains("timed out"),
        "retrying row carries the timeout message: {:?}",
        rows[0].error
    );
}

#[tokio::test]
async fn retry_without_explicit_timeout_keeps_hard_interrupt() {
    // 空 `on` 的"任意错误都重试"不含超时:超时仍是硬中断,一次尝试后整个
    // 运行立即终止 —— 既有语义不得被 P0-2 改变。
    let calls = Arc::new(AtomicUsize::new(0));
    let mut reg = ActionRegistry::new();
    reg.register(SlowThenQuick {
        calls: calls.clone(),
    });
    let flow = parse_str(&retry_flow("{ times: 2, initial_ms: 1 }")).unwrap();
    let vm = FlowVm::new(reg, None).with_step_timeout(Duration::from_millis(40));
    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("timeout must stay a hard interrupt when `on` omits it");
    assert!(matches!(err, ExecError::Timeout { .. }), "got: {err}");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry on hard timeout");
}

#[tokio::test]
async fn exhausted_timeout_retries_persist_timeout_state() {
    // 重试预算耗尽后,最后一次超时必须按 `timeout` 状态落库(语义保真),
    // 不得因为走过重试循环而被记成 `failed`。
    struct AlwaysSlow;
    #[async_trait]
    impl Action for AlwaysSlow {
        fn id(&self) -> &'static str {
            "test.slow_then_quick"
        }
        async fn execute(
            &self,
            _ctx: &mut StepCtx,
            _input: Value,
        ) -> Result<ActionResult, StepError> {
            tokio::time::sleep(Duration::from_millis(600)).await;
            Ok(ActionResult::null())
        }
    }
    let mut reg = ActionRegistry::new();
    reg.register(AlwaysSlow);
    let repo = Repo::open_in_memory().expect("repo");
    let flow = parse_str(&retry_flow("{ times: 1, initial_ms: 1, on: [timeout] }")).unwrap();
    let vm = FlowVm::new(reg, Some(repo.clone())).with_step_timeout(Duration::from_millis(40));
    let err = vm
        .run(&flow, RunOptions::default())
        .await
        .expect_err("every attempt times out, so the run must fail");
    assert!(matches!(err, ExecError::Timeout { .. }), "got: {err}");
    // 失败 run 拿不到 report,从 runs 表取最近一条的 id 再查步骤轨迹。
    let run = &repo.list_runs(1).expect("list runs")[0];
    let rows = repo.list_steps(&run.id).expect("list steps");
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert_eq!(states, vec!["retrying", "timeout"], "rows: {rows:?}");
}

// ─── P1-4:human.* 步骤不被更短的全局步级超时截杀 ────────────────────────────
//
// human.* 的等待上限（自身 `timeout_ms`，默认 1 小时）通常远大于宿主全局
// 步级超时（桌面端默认 10 分钟）。VM 对 "human." 前缀步骤取
// max(全局步级超时, 自身等待上限 + 小幅余量)，其余步骤维持原语义。这里用
// 真实的 `human.input` 动作（lumo-actions dev-dep）+ 延迟应答的 prompter 桩
// 驱动完整 select 竞速路径。

/// 延迟 `delay_ms` 后回执固定值的 prompter 桩 —— 模拟"真人过了一会儿才答"。
struct DelayedPrompter {
    delay_ms: u64,
    value: Value,
}
#[async_trait]
impl HumanPrompter for DelayedPrompter {
    async fn prompt(&self, _req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(HumanResponse {
            value: self.value.clone(),
            by: None,
            comment: None,
        })
    }
}

/// 永不回执的 prompter 桩 —— 驱动 human 自身超时路径。
struct NeverPrompter;
#[async_trait]
impl HumanPrompter for NeverPrompter {
    async fn prompt(&self, _req: HumanPromptRequest) -> Result<HumanResponse, StepError> {
        std::future::pending().await
    }
}

fn human_reg() -> ActionRegistry {
    let mut r = ActionRegistry::new();
    lumo_actions::register_all(&mut r);
    r
}

fn human_flow(with: &str) -> lumo_dsl::Flow {
    parse_str(&format!(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: human-t }}
spec:
  steps:
    - {{ id: ask, action: human.input, with: {with} }}
"#
    ))
    .expect("parse human flow")
}

#[tokio::test]
async fn human_step_survives_shorter_global_step_timeout() {
    // 全局步级超时 40ms << 应答延迟 250ms << 自身 timeout_ms 60s。
    // 修复前：步级超时在 40ms 硬杀等待（approve 语义下即"误拒"）。
    // 修复后：有效上限抬到 max(40ms, 60s+余量)，真人应答照常送达。
    let vm = FlowVm::new(human_reg(), None)
        .with_step_timeout(Duration::from_millis(40))
        .with_human_prompter(Some(Arc::new(DelayedPrompter {
            delay_ms: 250,
            value: Value::String("Alice".into()),
        })));
    let flow = human_flow(r#"{ message: "name?", timeout_ms: 60000 }"#);
    let report = vm
        .run(&flow, RunOptions::default())
        .await
        .expect("human step must outlive the shorter global step timeout");
    assert!(report.success);
    assert_eq!(
        report
            .outputs
            .expect("outputs")
            .pointer("/ask/result/value")
            .and_then(Value::as_str),
        Some("Alice")
    );
}

#[tokio::test]
async fn human_default_wait_ceiling_applies_without_explicit_timeout_ms() {
    // 不配 `timeout_ms` 时走 DEFAULT_HUMAN_TIMEOUT_MS（1 小时）分支：40ms 的
    // 全局步级超时同样不得截杀 250ms 后到达的应答。
    let vm = FlowVm::new(human_reg(), None)
        .with_step_timeout(Duration::from_millis(40))
        .with_human_prompter(Some(Arc::new(DelayedPrompter {
            delay_ms: 250,
            value: Value::String("Bob".into()),
        })));
    let flow = human_flow(r#"{ message: "name?" }"#);
    let report = vm
        .run(&flow, RunOptions::default())
        .await
        .expect("default 1h human ceiling must raise the 40ms step limit");
    assert!(report.success);
    assert_eq!(
        report
            .outputs
            .expect("outputs")
            .pointer("/ask/result/value")
            .and_then(Value::as_str),
        Some("Bob")
    );
}

#[tokio::test]
async fn human_own_shorter_timeout_semantics_unchanged() {
    // human 自身 timeout_ms(80ms) < 全局步级超时(10s)：语义不变 —— 动作自己
    // 的超时先分辨并回落 `default`，绝不等到步级上限。
    let t0 = std::time::Instant::now();
    let vm = FlowVm::new(human_reg(), None)
        .with_step_timeout(Duration::from_secs(10))
        .with_human_prompter(Some(Arc::new(NeverPrompter)));
    let flow = human_flow(r#"{ message: "name?", timeout_ms: 80, default: "fallback" }"#);
    let report = vm
        .run(&flow, RunOptions::default())
        .await
        .expect("the action's own timeout must fall back to `default`");
    assert!(report.success);
    assert_eq!(
        report
            .outputs
            .expect("outputs")
            .pointer("/ask/result/value")
            .and_then(Value::as_str),
        Some("fallback")
    );
    assert!(
        t0.elapsed() < Duration::from_secs(5),
        "must resolve via the 80ms human timeout, not any step ceiling; took {:?}",
        t0.elapsed()
    );
}
