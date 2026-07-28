//! 架构 P2(runs retention)+ P1-3(vars_json 去重)的存储层契约:
//!
//! * `Repo::prune_runs` 按 KeepDays / KeepCount 删 flow_runs,并在同一事务内
//!   级联清掉 step_runs / artifacts / ai_calls;running/queued/paused 永不删;
//!   报告的行数与 blob 路径准确。
//! * `Repo::list_steps` 对写入侧去重产生的 NULL vars_json 向前回溯补齐
//!   (NULL = 与前一条 seq 行相同);从未写过快照的行保持 None。

use chrono::{Duration, Utc};
use lumo_storage::{
    AiCallInsert, ArtifactRow, FlowRunRow, PrunePolicy, PruneReport, Repo, StepRunRow,
};

fn run_row(id: &str, state: &str, started_days_ago: i64) -> FlowRunRow {
    FlowRunRow {
        id: id.into(),
        flow_id: "f1".into(),
        flow_version: "0.1.0".into(),
        trigger_kind: "manual".into(),
        inputs: serde_json::json!({}),
        outputs: None,
        state: state.into(),
        worker_id: None,
        started_at: Some(Utc::now() - Duration::days(started_days_ago)),
        finished_at: None,
        cost_token: 0,
        cost_usd_micro: 0,
        trace_id: None,
    }
}

fn step_row(run_id: &str, seq: i64, vars: Option<serde_json::Value>) -> StepRunRow {
    StepRunRow {
        flow_run_id: run_id.into(),
        seq,
        path: format!("s{seq}"),
        parent_path: None,
        depth: 0,
        step_id: format!("s{seq}"),
        idx: seq,
        state: "ok".into(),
        attempt: 1,
        input_hash: vec![],
        output_json: None,
        vars_json: vars,
        error: None,
        started_at: Some(Utc::now()),
        finished_at: Some(Utc::now()),
        span_id: None,
    }
}

fn artifact_row(id: &str, run_id: &str) -> ArtifactRow {
    ArtifactRow {
        id: id.into(),
        flow_run_id: run_id.into(),
        step_id: None,
        kind: "screenshot".into(),
        mime: "image/png".into(),
        size: 1,
        blob_path: format!("/tmp/does-not-exist/{id}.png"),
        sha256: vec![0u8; 32],
        created_at: Utc::now(),
    }
}

/// 一个 run 挂 2 条 step、1 条 artifact、1 条 ai_call。
fn seed_run(repo: &Repo, id: &str, state: &str, started_days_ago: i64) {
    repo.create_run(&run_row(id, state, started_days_ago))
        .unwrap();
    repo.insert_step(&step_row(id, 0, Some(serde_json::json!({"v": 0}))))
        .unwrap();
    repo.insert_step(&step_row(id, 1, None)).unwrap();
    repo.insert_artifact(&artifact_row(&format!("A-{id}"), id))
        .unwrap();
    repo.record_ai_call(AiCallInsert {
        flow_run_id: id,
        step_id: None,
        helper: "chat",
        provider: "p",
        model: "m",
        input_tokens: 1,
        output_tokens: 1,
        latency_ms: 1,
        cost_usd_micro: 1,
    })
    .unwrap();
}

fn table_count(repo: &Repo, table: &str) -> i64 {
    repo.with_raw(|c| c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))
        .unwrap()
}

#[test]
fn prune_keep_count_deletes_oldest_and_cascades() {
    let repo = Repo::open_in_memory().unwrap();
    for (i, id) in ["R-old2", "R-old1", "R-new1", "R-new0"].iter().enumerate() {
        seed_run(&repo, id, "ok", 3 - i as i64); // 3,2,1,0 天前
    }

    let report = repo.prune_runs(PrunePolicy::KeepCount(2)).unwrap();
    assert_eq!(report.runs, 2, "keep the newest 2, prune the older 2");
    assert_eq!(report.steps, 4, "2 steps per pruned run");
    assert_eq!(report.artifacts, 2);
    assert_eq!(report.ai_calls, 2);
    assert_eq!(
        report.blob_paths.len(),
        2,
        "blob paths handed back for cleanup"
    );
    assert!(report.blob_paths.iter().all(|p| p.contains("R-old")));

    // 最新两条保留,更老的连同级联行一并消失。
    assert!(repo.get_run("R-new0").unwrap().is_some());
    assert!(repo.get_run("R-new1").unwrap().is_some());
    assert!(repo.get_run("R-old1").unwrap().is_none());
    assert!(repo.get_run("R-old2").unwrap().is_none());
    assert_eq!(table_count(&repo, "step_runs"), 4);
    assert_eq!(table_count(&repo, "artifacts"), 2);
    assert_eq!(table_count(&repo, "ai_calls"), 2);
}

#[test]
fn prune_keep_days_deletes_only_older_than_cutoff() {
    let repo = Repo::open_in_memory().unwrap();
    seed_run(&repo, "R-ancient", "failed", 10);
    seed_run(&repo, "R-recent", "ok", 1);

    let report = repo.prune_runs(PrunePolicy::KeepDays(7)).unwrap();
    assert_eq!(report.runs, 1);
    assert!(repo.get_run("R-ancient").unwrap().is_none());
    assert!(repo.get_run("R-recent").unwrap().is_some());

    // 再跑一次:没有可删的,返回零报告(且不报错)。
    assert_eq!(
        repo.prune_runs(PrunePolicy::KeepDays(7)).unwrap(),
        PruneReport::default()
    );
}

#[test]
fn prune_never_touches_running_queued_or_paused_runs() {
    let repo = Repo::open_in_memory().unwrap();
    seed_run(&repo, "R-running", "running", 30);
    seed_run(&repo, "R-queued", "queued", 30);
    seed_run(&repo, "R-paused", "paused", 30);
    seed_run(&repo, "R-done", "ok", 30);

    let by_days = repo.prune_runs(PrunePolicy::KeepDays(1)).unwrap();
    assert_eq!(by_days.runs, 1, "only the terminal-state run is pruned");
    assert!(repo.get_run("R-done").unwrap().is_none());

    // KeepCount(0)(全删)也保护活动/可续跑状态。
    let by_count = repo.prune_runs(PrunePolicy::KeepCount(0)).unwrap();
    assert_eq!(by_count.runs, 0);
    for id in ["R-running", "R-queued", "R-paused"] {
        assert!(repo.get_run(id).unwrap().is_some(), "{id} must survive");
    }
}

#[test]
fn list_steps_backfills_deduped_vars_json() {
    // P1-3:NULL = 与前一条 seq 行相同;list_steps 返回前向前回溯补齐。
    let repo = Repo::open_in_memory().unwrap();
    repo.create_run(&run_row("RV", "ok", 0)).unwrap();
    repo.insert_step(&step_row("RV", 0, Some(serde_json::json!({"a": 1}))))
        .unwrap();
    repo.insert_step(&step_row("RV", 1, None)).unwrap();
    repo.insert_step(&step_row("RV", 2, Some(serde_json::json!({"a": 2}))))
        .unwrap();
    repo.insert_step(&step_row("RV", 3, None)).unwrap();
    repo.insert_step(&step_row("RV", 4, None)).unwrap();

    let steps = repo.list_steps("RV").unwrap();
    let vars: Vec<_> = steps.iter().map(|s| s.vars_json.clone()).collect();
    assert_eq!(
        vars,
        vec![
            Some(serde_json::json!({"a": 1})),
            Some(serde_json::json!({"a": 1})),
            Some(serde_json::json!({"a": 2})),
            Some(serde_json::json!({"a": 2})),
            Some(serde_json::json!({"a": 2})),
        ],
        "NULL rows resolve to the nearest preceding full snapshot"
    );

    // 库内的原始行保持去重形态(3 条 NULL),不因读取被改写。
    let raw_nulls: i64 = repo
        .with_raw(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM step_runs WHERE flow_run_id='RV' AND vars_json IS NULL",
                [],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(raw_nulls, 3);
}

#[test]
fn list_steps_leaves_legacy_all_null_runs_untouched() {
    // v3 之前的老行整跑没有快照:没有可回溯的全量,保持 None。
    let repo = Repo::open_in_memory().unwrap();
    repo.create_run(&run_row("RL", "ok", 0)).unwrap();
    repo.insert_step(&step_row("RL", 0, None)).unwrap();
    repo.insert_step(&step_row("RL", 1, None)).unwrap();
    let steps = repo.list_steps("RL").unwrap();
    assert!(steps.iter().all(|s| s.vars_json.is_none()));
}
