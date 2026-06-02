//! F-20: breakpoint / single-step debugging. A run configured with breakpoints
//! (`with_breakpoints`) or single-step (`with_step_mode`) pauses *before* the
//! matching step — that step does not execute and is not persisted — and the
//! report carries `paused_at`. Resuming the paused run (`with_resume_from`)
//! replays the completed steps, "steps off" the paused step, and continues to
//! the next pause point. This composes the F-13 resume machinery with F-19's
//! per-step var snapshots into a debugger without a long-lived run process.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Counting probe: increments a per-id call counter and echoes `{ tag: <id> }`.
/// The counter lets a test prove which steps actually executed vs. were paused
/// before / replayed from a prior run.
struct Probe {
    id: &'static str,
    calls: Arc<AtomicUsize>,
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
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ActionResult::from(json!({ "tag": self.id })))
    }
}

const FLOW: &str = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: bp-test }
spec:
  steps:
    - { id: a, action: probe.a, with: { tag: "a" } }
    - { id: b, action: probe.b, with: { tag: "b" } }
    - { id: c, action: probe.c, with: { tag: "c" } }
"#;

struct Probes {
    a: Arc<AtomicUsize>,
    b: Arc<AtomicUsize>,
    c: Arc<AtomicUsize>,
}

impl Probes {
    fn new() -> Self {
        Self {
            a: Arc::new(AtomicUsize::new(0)),
            b: Arc::new(AtomicUsize::new(0)),
            c: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn registry(&self) -> ActionRegistry {
        let mut reg = ActionRegistry::new();
        reg.register(Probe { id: "probe.a", calls: self.a.clone() });
        reg.register(Probe { id: "probe.b", calls: self.b.clone() });
        reg.register(Probe { id: "probe.c", calls: self.c.clone() });
        reg
    }
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.a.load(Ordering::SeqCst),
            self.b.load(Ordering::SeqCst),
            self.c.load(Ordering::SeqCst),
        )
    }
}

fn bp(paths: &[&str]) -> HashSet<String> {
    paths.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn breakpoint_pauses_before_step_then_resume_continues() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::open(tmp.path().join("lumo.db")).unwrap();
    let probes = Probes::new();
    let flow = parse_str(FLOW).unwrap();

    // ── Run 1: breakpoint on `b` → a runs, pause before b, c never reached. ──
    let r1 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_breakpoints(bp(&["b"]))
        .run(&flow, RunOptions::default())
        .await
        .expect("a paused run returns Ok with paused_at, not an error");

    assert_eq!(r1.paused_at.as_deref(), Some("b"), "paused before `b`");
    assert!(!r1.success, "a paused run is not a success");
    assert_eq!(probes.counts(), (1, 0, 0), "a ran; b (the breakpoint) and c did not");

    // Persistence: `a` is recorded ok; the un-run breakpoint step `b` is absent
    // (so a resume re-executes from it), and the run row is `paused`.
    let steps = repo.list_steps(&r1.run_id).unwrap();
    assert_eq!(steps.len(), 1, "only `a` persisted");
    assert_eq!((steps[0].path.as_str(), steps[0].state.as_str()), ("a", "ok"));
    assert_eq!(repo.get_run(&r1.run_id).unwrap().unwrap().state, "paused");

    // ── Run 2: continue (resume, same breakpoint) → steps off b, runs b+c. ──
    let r2 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_resume_from(Some(r1.run_id.clone()))
        .with_breakpoints(bp(&["b"]))
        .run(&flow, RunOptions::default())
        .await
        .expect("resume completes");

    assert!(r2.success, "continuing past the breakpoint completes the run");
    assert_eq!(r2.paused_at, None, "no further pause");
    assert_eq!(
        probes.counts(),
        (1, 1, 1),
        "a was replayed (still 1), b stepped off and ran, c ran"
    );
}

#[tokio::test]
async fn single_step_advances_one_step_per_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::open(tmp.path().join("lumo.db")).unwrap();
    let probes = Probes::new();
    let flow = parse_str(FLOW).unwrap();

    // Fresh single-step run pauses before the very first step.
    let r1 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_step_mode(true)
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(r1.paused_at.as_deref(), Some("a"), "pause before first step");
    assert_eq!(probes.counts(), (0, 0, 0), "nothing executed yet");

    // Each resume steps off the current step and pauses before the next.
    let r2 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_resume_from(Some(r1.run_id))
        .with_step_mode(true)
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(r2.paused_at.as_deref(), Some("b"));
    assert_eq!(probes.counts(), (1, 0, 0), "stepped off a → a ran, paused before b");

    let r3 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_resume_from(Some(r2.run_id))
        .with_step_mode(true)
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(r3.paused_at.as_deref(), Some("c"));
    assert_eq!(probes.counts(), (1, 1, 0), "stepped off b → b ran, paused before c");

    // Stepping off the last step finishes the run.
    let r4 = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_resume_from(Some(r3.run_id))
        .with_step_mode(true)
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert!(r4.success, "stepping off the last step completes the run");
    assert_eq!(r4.paused_at, None);
    assert_eq!(probes.counts(), (1, 1, 1), "all three ran exactly once across the session");
}

#[tokio::test]
async fn breakpoint_fires_at_its_step_not_the_first() {
    // A breakpoint deep in the flow lets the earlier steps run normally and only
    // pauses at the marked step.
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repo::open(tmp.path().join("lumo.db")).unwrap();
    let probes = Probes::new();
    let flow = parse_str(FLOW).unwrap();

    let r = FlowVm::new(probes.registry(), Some(repo.clone()))
        .with_breakpoints(bp(&["c"]))
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert_eq!(r.paused_at.as_deref(), Some("c"), "paused before `c`");
    assert_eq!(probes.counts(), (1, 1, 0), "a and b ran; paused before c");
}

#[tokio::test]
async fn no_breakpoints_runs_to_completion() {
    // Sanity: with neither breakpoints nor single-step, the debug path is fully
    // inert — the run completes exactly as a normal run, `paused_at` is None.
    let probes = Probes::new();
    let flow = parse_str(FLOW).unwrap();
    let report = FlowVm::new(probes.registry(), None)
        .run(&flow, RunOptions::default())
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(report.paused_at, None);
    assert_eq!(probes.counts(), (1, 1, 1));
}
