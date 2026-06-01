//! A1: step persistence is offloaded onto tokio's blocking pool
//! (`spawn_blocking`) so the synchronous SQLite writes never block an async
//! worker thread. That is a non-functional change, so the observable contract
//! it must preserve — and what this test pins — is:
//!
//! Under a MULTI-THREADED runtime (where step futures genuinely migrate across
//! worker threads at every `.await`, including the new persist await), every
//! executed step still lands exactly one terminal `step_runs` row, and reading
//! those rows back `ORDER BY seq` reproduces the sequential execution order
//! with strictly increasing `seq`. A botched offload would drop rows, duplicate
//! a seq, scramble ordering, deadlock, or fail to stay `Send` across the await.
//!
//! This is a characterization test: it passes both before and after the
//! refactor by design (the seq is assigned synchronously, in execution order,
//! *before* the blocking write is handed off). Its job is to fail loudly if the
//! offload ever breaks that invariant.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, FlowVm, RunOptions, StepCtx};
use lumo_dsl::parse_str;
use lumo_storage::Repo;
use serde_json::Value;
use std::sync::OnceLock;

/// No-op leaf action that echoes its `with:` back as output. It needs no
/// capabilities, so a flat flow of these runs end-to-end through the real VM
/// path (render → schema-validate → dispatch → persist). The `yield_now`
/// forces a scheduler yield point so, on a multi-thread runtime, the task
/// actually migrates between workers around the persist boundary.
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
        tokio::task::yield_now().await;
        Ok(ActionResult::from(input))
    }
}

fn six_step_flow() -> String {
    let mut s = String::from(
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata: { id: persist-async-test }\nspec:\n  steps:\n",
    );
    for i in 0..6 {
        s.push_str(&format!(
            "    - id: s{i}\n      action: test.echo\n      with: {{ n: {i} }}\n"
        ));
    }
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sequential_steps_persist_in_seq_order_under_multithread() {
    let repo = Repo::open_in_memory().expect("open in-memory repo");
    let mut reg = ActionRegistry::new();
    reg.register(Echo);
    let flow = parse_str(&six_step_flow()).expect("parse");

    let vm = FlowVm::new(reg, Some(repo.clone()));
    let report = vm.run(&flow, RunOptions::default()).await.expect("run ok");
    assert!(report.success, "flow should succeed");

    let rows = repo.list_steps(&report.run_id).expect("list steps");

    // Exactly one terminal row per declared step — no drops, no duplicates.
    assert_eq!(
        rows.len(),
        6,
        "expected one row per step, got {}: {:?}",
        rows.len(),
        rows.iter().map(|r| &r.step_id).collect::<Vec<_>>()
    );

    // `list_steps` returns rows ORDER BY seq; for a sequential flow that must be
    // the declared execution order s0..s5, each with a strictly increasing seq
    // and a terminal `ok` state.
    let seqs: Vec<i64> = rows.iter().map(|r| r.seq).collect();
    let mut prev = i64::MIN;
    for (i, row) in rows.iter().enumerate() {
        assert!(
            row.seq > prev,
            "seq must strictly increase, got {seqs:?} at index {i}"
        );
        prev = row.seq;
        assert_eq!(row.step_id, format!("s{i}"), "step order at index {i}");
        assert_eq!(row.state, "ok", "step {} should be ok", row.step_id);
    }
}
