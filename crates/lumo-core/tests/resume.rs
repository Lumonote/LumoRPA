//! F-13: durable resume — re-running with `with_resume_from(prior_run_id)`
//! replays steps the prior run already completed (matching path + input hash)
//! instead of re-executing them, and runs everything else (the step that
//! failed, plus anything after it) fresh.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Counting probe: increments a call counter, records its last input, and
/// either fails (when its `fail` flag is set) or echoes `{ tag: <id> }`.
struct Probe {
    id: &'static str,
    calls: Arc<AtomicUsize>,
    fail: Arc<AtomicBool>,
    last_input: Arc<Mutex<Option<Value>>>,
}

#[async_trait]
impl Action for Probe {
    fn id(&self) -> &'static str {
        self.id
    }
    fn summary(&self) -> &'static str {
        "counting test probe"
    }
    fn schema(&self) -> &'static Value {
        static S: OnceLock<Value> = OnceLock::new();
        S.get_or_init(|| json!({ "type": "object" }))
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_input.lock() = Some(input);
        if self.fail.load(Ordering::SeqCst) {
            return Err(StepError::msg(format!("{} forced failure", self.id)));
        }
        Ok(ActionResult::from(json!({ "tag": self.id })))
    }
}

const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: resume-test }
spec:
  steps:
    - id: a
      action: probe.a
      with: { tag: "a" }
    - id: b
      action: probe.b
      with: { tag: "b" }
    - id: c
      action: probe.c
      with: { from_a: "{{ steps.a.result.tag }}" }
"#;

#[tokio::test]
async fn resume_replays_completed_steps_and_reruns_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::open(tmp.path().join("lumo.db")).expect("open repo");

    let calls_a = Arc::new(AtomicUsize::new(0));
    let calls_b = Arc::new(AtomicUsize::new(0));
    let calls_c = Arc::new(AtomicUsize::new(0));
    let fail_b = Arc::new(AtomicBool::new(true)); // b fails on the first run
    let c_input = Arc::new(Mutex::new(None));
    let ignored = Arc::new(Mutex::new(None));

    let build = || {
        let mut reg = ActionRegistry::new();
        reg.register(Probe {
            id: "probe.a",
            calls: calls_a.clone(),
            fail: Arc::new(AtomicBool::new(false)),
            last_input: ignored.clone(),
        });
        reg.register(Probe {
            id: "probe.b",
            calls: calls_b.clone(),
            fail: fail_b.clone(),
            last_input: ignored.clone(),
        });
        reg.register(Probe {
            id: "probe.c",
            calls: calls_c.clone(),
            fail: Arc::new(AtomicBool::new(false)),
            last_input: c_input.clone(),
        });
        reg
    };

    let flow = parse_str(FLOW).unwrap();

    // ── Run 1: b fails → a ran + persisted ok, b failed, c never reached. ──
    let r1 = FlowVm::new(build(), Some(repo.clone()))
        .run(&flow, RunOptions::default())
        .await;
    assert!(r1.is_err(), "run 1 should fail at b");
    assert_eq!(calls_a.load(Ordering::SeqCst), 1, "a ran in run 1");
    assert_eq!(calls_b.load(Ordering::SeqCst), 1, "b ran (and failed) in run 1");
    assert_eq!(calls_c.load(Ordering::SeqCst), 0, "c was never reached in run 1");

    let prior_id = repo.list_runs(10).unwrap()[0].id.clone();

    // ── Run 2: resume from run 1; b now succeeds. ──
    fail_b.store(false, Ordering::SeqCst);
    let report = FlowVm::new(build(), Some(repo.clone()))
        .with_resume_from(Some(prior_id))
        .run(&flow, RunOptions::default())
        .await
        .expect("run 2 should succeed");

    assert!(report.success, "run 2 succeeds");
    // The whole point of F-13: `a` is REPLAYED from the prior run, not re-run.
    assert_eq!(
        calls_a.load(Ordering::SeqCst),
        1,
        "a must be replayed on resume, not re-executed (call count stays 1)"
    );
    assert_eq!(
        calls_b.load(Ordering::SeqCst),
        2,
        "b must re-run on resume (it had failed)"
    );
    assert_eq!(
        calls_c.load(Ordering::SeqCst),
        1,
        "c runs for the first time on resume"
    );

    // The replayed output of `a` must be visible to downstream templates: c's
    // `{{ steps.a.result.tag }}` resolved from a's REPLAYED output, proving the
    // memo populated ctx exactly as a fresh run would.
    let seen = c_input.lock().clone().expect("c recorded its input");
    assert_eq!(
        seen["from_a"],
        json!("probe.a"),
        "c saw a's replayed output via the steps namespace"
    );
}

#[tokio::test]
async fn resume_reexecutes_when_input_hash_changes() {
    // If a step's rendered input differs from the prior run, the memo must NOT
    // replay it — the step re-executes so a changed upstream value propagates.
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::open(tmp.path().join("lumo.db")).expect("open repo");

    let calls_a = Arc::new(AtomicUsize::new(0));
    let ignored = Arc::new(Mutex::new(None));
    let build = || {
        let mut reg = ActionRegistry::new();
        reg.register(Probe {
            id: "probe.a",
            calls: calls_a.clone(),
            fail: Arc::new(AtomicBool::new(false)),
            last_input: ignored.clone(),
        });
        reg
    };

    // `a`'s input is templated from a flow input, so changing the input changes
    // its input hash between runs.
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: resume-hash-test }
spec:
  inputs:
    - { name: who, type: string }
  steps:
    - id: a
      action: probe.a
      with: { who: "{{ inputs.who }}" }
"#,
    )
    .unwrap();

    let r1 = FlowVm::new(build(), Some(repo.clone()))
        .run(
            &flow,
            RunOptions {
                inputs: json!({ "who": "alice" }),
                trigger_kind: "manual".into(),
            },
        )
        .await
        .expect("run 1 ok");
    assert_eq!(calls_a.load(Ordering::SeqCst), 1);

    // Resume but with a DIFFERENT input → hash differs → a must re-run.
    FlowVm::new(build(), Some(repo.clone()))
        .with_resume_from(Some(r1.run_id.clone()))
        .run(
            &flow,
            RunOptions {
                inputs: json!({ "who": "bob" }),
                trigger_kind: "manual".into(),
            },
        )
        .await
        .expect("run 2 ok");
    assert_eq!(
        calls_a.load(Ordering::SeqCst),
        2,
        "changed input hash must force re-execution, not replay"
    );
}
