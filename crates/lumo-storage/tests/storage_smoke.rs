use chrono::Utc;
use lumo_storage::{FlowRunRow, Repo, StepRunRow};

fn make_run(id: &str) -> FlowRunRow {
    FlowRunRow {
        id: id.into(),
        flow_id: "f1".into(),
        flow_version: "0.1.0".into(),
        trigger_kind: "manual".into(),
        inputs: serde_json::json!({}),
        outputs: None,
        state: "running".into(),
        worker_id: None,
        started_at: Some(Utc::now()),
        finished_at: None,
        cost_token: 0,
        cost_usd_micro: 0,
        trace_id: None,
    }
}

fn make_step(run_id: &str, seq: i64, path: &str) -> StepRunRow {
    StepRunRow {
        flow_run_id: run_id.into(),
        seq,
        path: path.into(),
        parent_path: None,
        depth: 0,
        step_id: "same_step".into(),
        idx: 0,
        state: "ok".into(),
        attempt: 1,
        input_hash: vec![],
        output_json: Some(serde_json::json!({"seq": seq})),
        error: None,
        started_at: Some(Utc::now()),
        finished_at: Some(Utc::now()),
        span_id: None,
        vars_json: Some(serde_json::json!({ "v": seq })),
    }
}

#[test]
fn in_memory_run_lifecycle() {
    let repo = Repo::open_in_memory().unwrap();
    let run = make_run("R1");
    repo.create_run(&run).unwrap();
    let fetched = repo.get_run("R1").unwrap().unwrap();
    assert_eq!(fetched.flow_id, "f1");
    assert_eq!(fetched.state, "running");

    repo.finish_run("R1", "ok", Some(&serde_json::json!({"v": 1})))
        .unwrap();
    let after = repo.get_run("R1").unwrap().unwrap();
    assert_eq!(after.state, "ok");
    assert_eq!(after.outputs, Some(serde_json::json!({"v": 1})));

    let listed = repo.list_runs(10).unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn file_repo_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lumo.db");
    let repo = Repo::open(&path).unwrap();
    repo.create_run(&make_run("R2")).unwrap();
    drop(repo);
    let again = Repo::open(&path).unwrap();
    assert!(again.get_run("R2").unwrap().is_some());
}

#[test]
fn step_runs_allow_repeated_step_ids_with_distinct_paths() {
    let repo = Repo::open_in_memory().unwrap();
    repo.create_run(&make_run("R3")).unwrap();
    repo.insert_step(&make_step("R3", 0, "loop[0]/same_step"))
        .unwrap();
    repo.insert_step(&make_step("R3", 1, "loop[1]/same_step"))
        .unwrap();

    let steps = repo.list_steps("R3").unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].path, "loop[0]/same_step");
    assert_eq!(steps[1].path, "loop[1]/same_step");
}

#[test]
fn step_vars_json_round_trips() {
    // F-19: the per-step variable snapshot persists and reads back intact,
    // including across a close/reopen (exercises the v2→v3 migration on a file DB).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lumo.db");
    let repo = Repo::open(&path).unwrap();
    repo.create_run(&make_run("RV")).unwrap();
    repo.insert_step(&make_step("RV", 0, "set_x")).unwrap();
    drop(repo);

    let again = Repo::open(&path).unwrap();
    let steps = again.list_steps("RV").unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].vars_json, Some(serde_json::json!({ "v": 0 })));
}

#[test]
fn summary_pages_skip_payloads_and_preserve_cursor_order() {
    let repo = Repo::open_in_memory().unwrap();
    for index in 0..4 {
        let mut run = make_run(&format!("page-{index}"));
        run.started_at = Some(chrono::DateTime::from_timestamp_millis(1000).unwrap());
        run.inputs = serde_json::json!({"secret": "large-input"});
        run.outputs = Some(serde_json::json!({"large": "payload"}));
        repo.create_run(&run).unwrap();
    }
    let first = repo.list_runs_page(2, 0).unwrap();
    let second = repo.list_runs_page(2, 2).unwrap();
    assert_eq!(
        first.iter().map(|r| &*r.id).collect::<Vec<_>>(),
        ["page-3", "page-2"]
    );
    assert_eq!(
        second.iter().map(|r| &*r.id).collect::<Vec<_>>(),
        ["page-1", "page-0"]
    );
    assert!(first
        .iter()
        .all(|r| r.outputs.is_none() && r.inputs == serde_json::json!({})));
    for seq in [1, 2, 4, 7] {
        repo.insert_step(&make_step("page-0", seq, &format!("step-{seq}")))
            .unwrap();
    }
    let page = repo.list_step_summaries("page-0", Some(2), 2).unwrap();
    assert_eq!(page.iter().map(|s| s.seq).collect::<Vec<_>>(), [4, 7]);
    assert!(page
        .iter()
        .all(|s| s.output_json.is_none() && s.vars_json.is_none()));
    assert!(repo
        .get_run_summary("page-0")
        .unwrap()
        .unwrap()
        .outputs
        .is_none());
    assert!(repo.get_run("page-0").unwrap().unwrap().outputs.is_some());
}

#[test]
fn selected_step_restores_only_its_nearest_variable_checkpoint() {
    let repo = Repo::open_in_memory().unwrap();
    repo.create_run(&make_run("checkpoints")).unwrap();
    for seq in 0..6 {
        let mut step = make_step("checkpoints", seq, &format!("loop[{seq}]/read"));
        step.vars_json = match seq {
            0 => Some(serde_json::json!({"version":1})),
            3 => Some(serde_json::json!({"version":2})),
            _ => None,
        };
        step.output_json = Some(serde_json::json!(false));
        repo.insert_step(&step).unwrap();
    }
    let earlier = repo.get_step("checkpoints", 2).unwrap().unwrap();
    let later = repo.get_step("checkpoints", 5).unwrap().unwrap();
    assert_eq!(earlier.vars_json, Some(serde_json::json!({"version":1})));
    assert_eq!(later.vars_json, Some(serde_json::json!({"version":2})));
    assert_eq!(later.output_json, Some(serde_json::json!(false)));
    assert!(repo.get_step("checkpoints", 99).unwrap().is_none());
}

#[test]
fn ai_cost_pages_are_bounded_and_isolated_by_run() {
    let repo = Repo::open_in_memory().unwrap();
    for run_id in ["cost-a", "cost-b"] {
        repo.create_run(&make_run(run_id)).unwrap();
        for index in 0..205 {
            repo.record_ai_call(lumo_storage::AiCallInsert {
                flow_run_id: run_id,
                step_id: Some("loop/read"),
                helper: "test",
                provider: "test",
                model: "test",
                input_tokens: index,
                output_tokens: 1,
                latency_ms: 1,
                cost_usd_micro: 1,
            })
            .unwrap();
        }
    }
    let first = repo.list_ai_calls_page("cost-a", None, 1000).unwrap();
    assert_eq!(first.len(), 200);
    let second = repo
        .list_ai_calls_page("cost-a", Some(first.last().unwrap().id), 200)
        .unwrap();
    assert_eq!(second.len(), 5);
    assert!(second.iter().all(|row| row.flow_run_id == "cost-a"));
    assert!(second[0].id > first.last().unwrap().id);
    assert!(repo
        .list_ai_calls_page("cost-a", Some(second.last().unwrap().id), 200)
        .unwrap()
        .is_empty());
}
