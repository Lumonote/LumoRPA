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
