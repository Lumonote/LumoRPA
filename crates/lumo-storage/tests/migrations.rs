//! P1-7: schema-versioning + per-connection PRAGMA tests.

use chrono::Utc;
use lumo_storage::{FlowRunRow, Repo};
use rusqlite::Connection;

/// Must match `lumo_storage::repo::LATEST_USER_VERSION`. Hard-coded here on
/// purpose so an accidental change to the migration list trips the test.
/// v4: adds durable agent, MCP, voice, capability, and improvement tables.
const EXPECTED_USER_VERSION: i64 = 4;

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
fn fresh_db_contains_v4_tables() {
    let repo = Repo::open_in_memory().unwrap();
    let expected = [
        "voice_profiles",
        "mcp_servers",
        "mcp_tools",
        "capability_aliases",
        "agent_profiles",
        "agent_runs",
        "agent_events",
        "improvement_proposals",
        "improvement_approvals",
    ];

    for table in expected {
        let exists = repo
            .with_raw(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
            })
            .unwrap();
        assert!(exists, "missing v4 table {table}");
    }
}

#[test]
fn version_three_db_is_upgraded_to_v4() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("v3.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE flow_runs (
              id TEXT PRIMARY KEY,
              state TEXT NOT NULL
            );
            INSERT INTO flow_runs(id, state) VALUES ('legacy-run', 'ok');
            PRAGMA user_version = 3;
            "#,
        )
        .unwrap();
    }

    let repo = Repo::open(&path).unwrap();
    let has_agent_runs = repo
        .with_raw(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_runs')",
                [],
                |row| row.get::<_, bool>(0),
            )
        })
        .unwrap();
    assert!(has_agent_runs);
    let legacy_state: String = repo
        .with_raw(|conn| {
            conn.query_row(
                "SELECT state FROM flow_runs WHERE id='legacy-run'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(legacy_state, "ok");
    drop(repo);
    assert_eq!(user_version(&path), EXPECTED_USER_VERSION);
}

#[test]
fn failed_v4_migration_rolls_back_schema_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("broken-v4.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE VIEW mcp_tools AS SELECT 1 AS placeholder;
            PRAGMA user_version = 3;
            "#,
        )
        .unwrap();
    }

    assert!(Repo::open(&path).is_err());

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3, "failed migration must not stamp v4");
    let voice_profiles_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='voice_profiles')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        !voice_profiles_exists,
        "earlier v4 DDL must roll back when a later statement fails"
    );
}

#[test]
fn v4_boolean_columns_reject_values_outside_zero_and_one() {
    let repo = Repo::open_in_memory().unwrap();

    assert!(repo
        .with_raw(|conn| conn.execute(
            "INSERT INTO mcp_servers(id,name,transport,config_json,enabled,created_at,updated_at) \
             VALUES ('bad-server','Bad','stdio','{}',2,0,0)",
            [],
        ))
        .is_err());

    repo.with_raw(|conn| {
        conn.execute(
            "INSERT INTO mcp_servers(id,name,transport,config_json,enabled,created_at,updated_at) \
             VALUES ('server','Good','stdio','{}',1,0,0)",
            [],
        )
    })
    .unwrap();
    assert!(repo
        .with_raw(|conn| {
            conn.execute(
            "INSERT INTO mcp_tools(server_id,name,input_schema,enabled,version_hash,discovered_at) \
             VALUES ('server','bad-tool','{}',-1,'v1',0)",
            [],
        )
        })
        .is_err());

    assert!(repo
        .with_raw(|conn| conn.execute(
            "INSERT INTO capability_aliases(capability_id,alias,enabled,updated_at) \
             VALUES ('cap','alias',2,0)",
            [],
        ))
        .is_err());
    assert!(repo
        .with_raw(|conn| conn.execute(
            "INSERT INTO agent_profiles(id,name,config_json,is_default,updated_at) \
             VALUES ('profile','Profile','{}',-1,0)",
            [],
        ))
        .is_err());
}

#[test]
fn agent_run_delete_cascades_to_events() {
    let repo = Repo::open_in_memory().unwrap();
    repo.with_raw(|conn| {
        conn.execute_batch(
            r#"
            INSERT INTO agent_runs(id,state,started_at) VALUES ('run','running',0);
            INSERT INTO agent_events(run_id,seq,kind,payload,created_at)
              VALUES ('run',1,'started','{}',0);
            DELETE FROM agent_runs WHERE id='run';
            "#,
        )
    })
    .unwrap();
    let count: i64 = repo
        .with_raw(|conn| conn.query_row("SELECT COUNT(*) FROM agent_events", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn improvement_proposal_delete_cascades_to_approvals() {
    let repo = Repo::open_in_memory().unwrap();
    repo.with_raw(|conn| {
        conn.execute_batch(
            r#"
            INSERT INTO improvement_proposals(
              id,target_kind,target_id,patch_json,rationale,status,base_version_hash,created_at,updated_at
            ) VALUES ('proposal','tool','tool-1','{}','reason','pending','base',0,0);
            INSERT INTO improvement_approvals(
              proposal_id,patch_hash,base_version_hash,approver,decision,created_at
            ) VALUES ('proposal','patch','base','reviewer','approved',0);
            DELETE FROM improvement_proposals WHERE id='proposal';
            "#,
        )
    })
    .unwrap();
    let count: i64 = repo
        .with_raw(|conn| {
            conn.query_row("SELECT COUNT(*) FROM improvement_approvals", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(count, 0);
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
