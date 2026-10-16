//! 批次 E：`control.for_each` 的 `rate_per_sec`（迭代限流）与 `max_failures`
//! （失败容忍阈值 / 熔断）。约定：
//!   * `rate_per_sec` > 0 —— 顺序模式在相邻迭代启动之间、并行模式在相邻批次
//!     启动之间保持 `1/rate` 秒的最小间隔；
//!   * `max_failures` >= 1 —— 普通动作错误不再立刻终止循环，累计到阈值才以
//!     熔断错误收场；控制信号（cancel/timeout/pause/break/continue）永不计数。

use async_trait::async_trait;
use lumo_core::error::{ExecError, StepError};
use lumo_core::{Action, ActionRegistry, ActionResult, CancelToken, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 计数动作：每次执行 +1；`with.fail` 为真（bool 或 "true" 字符串）时报错。
struct CountAction(Arc<AtomicUsize>);
#[async_trait]
impl Action for CountAction {
    fn id(&self) -> &'static str {
        "test.count"
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let fail = match input.get("fail") {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s == "true",
            _ => false,
        };
        if fail {
            return Err(StepError::msg("boom"));
        }
        Ok(ActionResult::null())
    }
}

/// 睡眠动作：给取消测试一个可被打断的慢步骤。
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

fn reg(counter: Arc<AtomicUsize>) -> ActionRegistry {
    let mut r = ActionRegistry::new();
    r.register(CountAction(counter));
    r.register(SleepAction);
    r
}

const RATE_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [1, 2, 3, 4], rate_per_sec: 50 }
      do:
        - { id: c, action: test.count, with: {} }
"#;

const ABORT_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [false, true, false, true, true], max_failures: 2 }
      do:
        - { id: c, action: test.count, with: { fail: "{{ item }}" } }
"#;

const TOLERATE_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [false, true, false, true, true], max_failures: 10 }
      do:
        - { id: c, action: test.count, with: { fail: "{{ item }}" } }
    - { id: after, action: test.count, with: {} }
"#;

const PARALLEL_ABORT_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with:
        in: [false, true, false, true, false]
        parallel: true
        max_concurrency: 2
        max_failures: 2
      do:
        - { id: c, action: test.count, with: { fail: "{{ item }}" } }
"#;

const CANCEL_FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [1, 2, 3], max_failures: 5 }
      do:
        - { id: slow, action: test.sleep, with: { ms: 400 } }
"#;

#[tokio::test]
async fn rate_per_sec_spaces_sequential_iterations() {
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter.clone()), None);
    let started = Instant::now();
    vm.run(&parse_str(RATE_FLOW).unwrap(), RunOptions::default())
        .await
        .expect("rate-limited loop should still succeed");
    // 4 次迭代 × 50/s ⇒ 至少 3 个 20ms 间隔。只压下限，避免慢机 flaky。
    assert!(
        started.elapsed() >= Duration::from_millis(60),
        "elapsed {:?} should be >= 60ms",
        started.elapsed()
    );
    assert_eq!(counter.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn max_failures_aborts_when_threshold_reached() {
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter.clone()), None);
    let err = vm
        .run(&parse_str(ABORT_FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("second failure must trip the breaker");
    assert!(
        err.to_string().contains("max_failures"),
        "error should mention max_failures: {err}"
    );
    // 第 4 项触发第 2 次失败并熔断，第 5 项不再执行。
    assert_eq!(counter.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn max_failures_tolerates_failures_below_threshold() {
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter.clone()), None);
    vm.run(&parse_str(TOLERATE_FLOW).unwrap(), RunOptions::default())
        .await
        .expect("failures below max_failures must not fail the loop");
    // 5 个循环体 + 循环后的 after 步骤。
    assert_eq!(counter.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn parallel_batches_respect_max_failures() {
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter.clone()), None);
    let err = vm
        .run(
            &parse_str(PARALLEL_ABORT_FLOW).unwrap(),
            RunOptions::default(),
        )
        .await
        .expect_err("two failures across batches must trip the breaker");
    assert!(
        err.to_string().contains("max_failures"),
        "error should mention max_failures: {err}"
    );
    // 批次 [1,2] 与 [3,4] 执行（第 2、4 项失败后熔断），第 5 项所在批次不再启动。
    assert_eq!(counter.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn max_failures_never_swallows_cancellation() {
    let token = CancelToken::new();
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter), None).with_cancel(token.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
    });
    let err = vm
        .run(&parse_str(CANCEL_FLOW).unwrap(), RunOptions::default())
        .await
        .expect_err("cancellation must abort even inside a tolerant loop");
    assert!(matches!(err, ExecError::Cancelled), "got: {err}");
}

#[tokio::test]
async fn invalid_rate_and_zero_max_failures_are_rejected() {
    let counter = Arc::new(AtomicUsize::new(0));
    let vm = FlowVm::new(reg(counter.clone()), None);
    let bad_rate = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [1], rate_per_sec: 0 }
      do:
        - { id: c, action: test.count, with: {} }
"#;
    let err = vm
        .run(&parse_str(bad_rate).unwrap(), RunOptions::default())
        .await
        .expect_err("rate_per_sec=0 must be rejected");
    assert!(err.to_string().contains("rate_per_sec"), "{err}");

    let vm = FlowVm::new(reg(counter), None);
    let bad_failures = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - id: loop
      action: control.for_each
      with: { in: [1], max_failures: 0 }
      do:
        - { id: c, action: test.count, with: {} }
"#;
    let err = vm
        .run(&parse_str(bad_failures).unwrap(), RunOptions::default())
        .await
        .expect_err("max_failures=0 must be rejected");
    assert!(err.to_string().contains("max_failures"), "{err}");
}
