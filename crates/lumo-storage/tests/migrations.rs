//! P1-7: schema-versioning + per-connection PRAGMA tests.

use chrono::Utc;
use lumo_storage::{FlowRunRow, Repo};
use rusqlite::Connection;

/// Must match `lumo_storage::repo::LATEST_USER_VERSION`. Hard-coded here on
/// purpose so an accidental change to the migration list trips the test.
const EXPECTED_USER_VERSION: i64 = 2;

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

fn user_version(path: &std::path::Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

fn busy_timeout(repo: &Repo) -> i64 {
    repo.with_raw(|c| c.query_row("PRAGMA busy_timeout", [], |r| r.get(0)))
        .unwrap()
}

#[test]
fn fresh_db_is_at_latest_user_version() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lumo.db");
    let _repo = Repo::open(&path).unwrap();
    assert_eq!(user_version(&path), EXPECTED_USER_VERSION);
}

#[test]
fn reopen_is_idempotent_and_preserves_data() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lumo.db");

    let repo = Repo::open(&path).unwrap();
    repo.create_run(&make_run("RX")).unwrap();
    drop(repo);
    let v1 = user_version(&path);

    let again = Repo::open(&path).unwrap();
    assert!(again.get_run("RX").unwrap().is_some());
    drop(again);
    let v2 = user_version(&path);

    assert_eq!(v1, EXPECTED_USER_VERSION);
    assert_eq!(v1, v2, "reopening must not bump user_version");
}

#[test]
fn busy_timeout_is_nonzero_after_open() {
    let repo = Repo::open_in_memory().unwrap();
    assert!(busy_timeout(&repo) > 0, "busy_timeout must be set on open");
}

#[test]
fn foreign_keys_enabled_after_open() {
    let repo = Repo::open_in_memory().unwrap();
    let on: i64 = repo
        .with_raw(|c| c.query_row("PRAGMA foreign_keys", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(on, 1, "foreign_keys must be ON per-connection");
}

/// Simulate a legacy DB (pre-versioning, no `seq` column) and confirm the
/// migration framework upgrades it in place to the latest version while
/// preserving the existing rows.
#[test]
fn legacy_db_without_seq_is_migrated_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("legacy.db");

    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE flow_runs (
              id TEXT PRIMARY KEY,
              flow_id TEXT NOT NULL,
              flow_version TEXT NOT NULL,
              trigger_kind TEXT NOT NULL,
              inputs TEXT NOT NULL,
              outputs TEXT,
              state TEXT NOT NULL,
              worker_id TEXT,
              started_at INTEGER,
              finished_at INTEGER,
              cost_token INTEGER NOT NULL DEFAULT 0,
              cost_usd_micro INTEGER NOT NULL DEFAULT 0,
              trace_id TEXT
            );
            CREATE TABLE step_runs (
              flow_run_id TEXT NOT NULL,
              step_id TEXT NOT NULL,
              idx INTEGER NOT NULL,
              state TEXT NOT NULL,
              attempt INTEGER NOT NULL DEFAULT 1,
              input_hash BLOB NOT NULL,
              output_json TEXT,
              error TEXT,
              started_at INTEGER,
              finished_at INTEGER,
              span_id TEXT
            );
            INSERT INTO flow_runs(id,flow_id,flow_version,trigger_kind,inputs,state)
              VALUES ('R_old','f1','0.1.0','manual','{}','ok');
            INSERT INTO step_runs(flow_run_id,step_id,idx,state,input_hash)
              VALUES ('R_old','s1',0,'ok',X'00');
            "#,
        )
        .unwrap();
        // legacy DB has no user_version set (defaults to 0)
    }

    let repo = Repo::open(&path).unwrap();
    // legacy row preserved + migrated into the new schema with a `seq` column
    let steps = repo.list_steps("R_old").unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].step_id, "s1");
    drop(repo);

    assert_eq!(user_version(&path), EXPECTED_USER_VERSION);
}
