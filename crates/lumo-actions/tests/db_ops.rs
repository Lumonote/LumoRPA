//! Integration coverage for the `db.*` SQLite action family (P1-8).
//! A temp database is built with `db.sqlite_exec` then read with
//! `db.sqlite_query`; reads honor `fs.read`, writes honor `fs.write`.

mod common;
use common::{fs_caps, ok_with, run_with, Capabilities};
use serde_json::json;

#[tokio::test]
async fn exec_then_query_round_trips_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let caps = fs_caps(dir.path());

    // DDL reports zero affected rows.
    let created = ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "CREATE TABLE t (id INTEGER, name TEXT)"}),
        caps.clone(),
    )
    .await;
    assert_eq!(created, json!({"rows_affected": 0}));

    // Parameterized insert binds positional args.
    let inserted = ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "INSERT INTO t (id, name) VALUES (?, ?)", "args": [1, "alice"]}),
        caps.clone(),
    )
    .await;
    assert_eq!(inserted, json!({"rows_affected": 1}));

    ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "INSERT INTO t (id, name) VALUES (?, ?)", "args": [2, "bob"]}),
        caps.clone(),
    )
    .await;

    let rows = ok_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT id, name FROM t ORDER BY id"}),
        caps,
    )
    .await;
    assert_eq!(rows["count"], json!(2));
    assert_eq!(rows["truncated"], json!(false));
    assert_eq!(
        rows["rows"],
        json!([{"id": 1, "name": "alice"}, {"id": 2, "name": "bob"}])
    );
}

#[tokio::test]
async fn query_limit_marks_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let caps = fs_caps(dir.path());

    ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "CREATE TABLE t (id INTEGER)"}),
        caps.clone(),
    )
    .await;
    for id in 0..3 {
        ok_with(
            "db.sqlite_exec",
            json!({"db": db, "sql": "INSERT INTO t (id) VALUES (?)", "args": [id]}),
            caps.clone(),
        )
        .await;
    }

    let rows = ok_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT id FROM t", "limit": 1}),
        caps,
    )
    .await;
    assert_eq!(rows["count"], json!(1));
    assert_eq!(
        rows["truncated"],
        json!(true),
        "hitting the limit flags truncation"
    );
}

#[tokio::test]
async fn query_is_denied_without_an_fs_grant() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let err = run_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT 1"}),
        common::Capabilities::default(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}

#[tokio::test]
async fn sqlite_exec_dry_run_does_not_mutate() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("dry.db");
    let caps = fs_caps(dir.path());
    ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "CREATE TABLE t (id INTEGER)"}),
        caps.clone(),
    )
    .await;
    let out = ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "INSERT INTO t VALUES (?)", "args": [7], "dry_run": true}),
        caps.clone(),
    )
    .await;
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["would_execute"], json!(true));
    let rows = ok_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT COUNT(*) AS n FROM t"}),
        caps,
    )
    .await;
    assert_eq!(rows["rows"][0]["n"], json!(0));
}

#[tokio::test]
async fn sqlite_batch_dry_run_previews_without_creating_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("batch-dry.sqlite");
    let out = ok_with(
        "db.sqlite_batch",
        json!({
            "db": db,
            "statements": [
                {"sql": "CREATE TABLE t(v INTEGER)"},
                {"sql": "INSERT INTO t VALUES (?)", "args": [7]}
            ],
            "dry_run": true
        }),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["statements"], json!(2));
    assert!(!db.exists(), "dry-run must not create the SQLite file");
}

#[tokio::test]
async fn batch_commits_all_statements_in_one_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("batch.db");
    let caps = fs_caps(dir.path());

    ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "CREATE TABLE t (id INTEGER, name TEXT)"}),
        caps.clone(),
    )
    .await;

    let out = ok_with(
        "db.sqlite_batch",
        json!({
            "db": db,
            "statements": [
                {"sql": "INSERT INTO t (id, name) VALUES (?, ?)", "args": [1, "a"]},
                {"sql": "INSERT INTO t (id, name) VALUES (?, ?)", "args": [2, "b"]},
                {"sql": "UPDATE t SET name = ? WHERE id = ?", "args": ["B", 2]},
            ]
        }),
        caps.clone(),
    )
    .await;
    assert_eq!(out["statements"], json!(3));
    assert_eq!(out["rows_affected"], json!([1, 1, 1]));
    assert_eq!(out["total_affected"], json!(3));

    let rows = ok_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT id, name FROM t ORDER BY id"}),
        caps,
    )
    .await;
    assert_eq!(
        rows["rows"],
        json!([{"id": 1, "name": "a"}, {"id": 2, "name": "B"}])
    );
}

#[tokio::test]
async fn batch_rolls_back_entirely_on_a_failing_statement() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rollback.db");
    let caps = fs_caps(dir.path());

    ok_with(
        "db.sqlite_exec",
        json!({"db": db, "sql": "CREATE TABLE t (id INTEGER PRIMARY KEY)"}),
        caps.clone(),
    )
    .await;

    // The 2nd insert duplicates the primary key → the whole tx rolls back, so
    // even the (valid) 1st insert must not persist.
    let err = run_with(
        "db.sqlite_batch",
        json!({
            "db": db,
            "statements": [
                {"sql": "INSERT INTO t (id) VALUES (1)"},
                {"sql": "INSERT INTO t (id) VALUES (1)"},
            ]
        }),
        caps.clone(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("statement 1"), "got: {err}");

    let rows = ok_with(
        "db.sqlite_query",
        json!({"db": db, "sql": "SELECT COUNT(*) AS n FROM t"}),
        caps,
    )
    .await;
    assert_eq!(
        rows["rows"][0]["n"],
        json!(0),
        "a failed batch leaves no rows (full rollback)"
    );
}

#[tokio::test]
async fn batch_requires_at_least_one_statement() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty.db");
    let caps = fs_caps(dir.path());
    let err = run_with("db.sqlite_batch", json!({"db": db, "statements": []}), caps)
        .await
        .unwrap_err();
    assert!(err.contains("at least one"), "got: {err}");
}

#[tokio::test]
async fn batch_is_denied_without_an_fs_write_grant() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let err = run_with(
        "db.sqlite_batch",
        json!({"db": db, "statements": [{"sql": "CREATE TABLE t (id INTEGER)"}]}),
        common::Capabilities::default(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("fs.write"), "got: {err}");
}

// ─── T3: a bound `sqlite` resource is one connection, reused across steps ──────

#[tokio::test]
async fn bound_sqlite_resource_reuses_one_connection_across_steps() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("shared.db");
    let caps = fs_caps(dir.path());
    let run_id = "db-reuse-1";
    // The resource declares the path; bound steps omit their own `db`.
    let decl_yaml = format!("kind: sqlite\npath: \"{}\"\n", db.display());
    let res: &[(&str, &str)] = &[("mydb", decl_yaml.as_str())];

    // Step 1 creates a CONNECTION-SCOPED temp table. Temp tables die with the
    // connection, so later steps see it ONLY if the same connection is reused.
    common::run_bound(
        run_id,
        res,
        Some("mydb"),
        "db.sqlite_exec",
        json!({"sql": "CREATE TEMP TABLE marker (v INTEGER)"}),
        caps.clone(),
    )
    .await
    .expect("create temp table on the shared connection");

    // Step 2 inserts into it — this errors "no such table" if the connection was
    // NOT reused (a fresh per-call connection wouldn't have the temp table).
    common::run_bound(
        run_id,
        res,
        Some("mydb"),
        "db.sqlite_exec",
        json!({"sql": "INSERT INTO marker (v) VALUES (42)"}),
        caps.clone(),
    )
    .await
    .expect("insert into the reused temp table");

    // Step 3 reads it back through the same shared connection.
    let rows = common::run_bound(
        run_id,
        res,
        Some("mydb"),
        "db.sqlite_query",
        json!({"sql": "SELECT v FROM marker"}),
        caps.clone(),
    )
    .await
    .expect("query the reused temp table");
    assert_eq!(
        rows["rows"],
        json!([{"v": 42}]),
        "the temp table from step 1 is visible ⇒ one connection was reused"
    );

    // The connection is held for the run until teardown reaps it (idempotently).
    assert!(lumo_actions::db_ops::sqlite_conn_open(run_id));
    lumo_actions::db_ops::close_run_connections(run_id);
    assert!(!lumo_actions::db_ops::sqlite_conn_open(run_id));
    lumo_actions::db_ops::close_run_connections(run_id); // no-op, must not panic
}

#[tokio::test]
async fn bound_sqlite_query_cannot_write_despite_shared_rw_connection() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ro.db");
    let caps = fs_caps(dir.path());
    let run_id = "db-reuse-ro";
    let decl_yaml = format!("kind: sqlite\npath: \"{}\"\n", db.display());
    let res: &[(&str, &str)] = &[("mydb", decl_yaml.as_str())];

    // Seed a table via the bound (writable) exec path.
    common::run_bound(
        run_id,
        res,
        Some("mydb"),
        "db.sqlite_exec",
        json!({"sql": "CREATE TABLE t (id INTEGER)"}),
        caps.clone(),
    )
    .await
    .expect("create table");

    // A write smuggled into `db.sqlite_query` must still be rejected — the shared
    // connection is RW, but `query_only` keeps queries read-only (preserving the
    // fs.read-vs-fs.write boundary that the unbound read-only open enforces).
    let err = common::run_bound(
        run_id,
        res,
        Some("mydb"),
        "db.sqlite_query",
        json!({"sql": "INSERT INTO t (id) VALUES (1)"}),
        caps.clone(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("readonly") || err.contains("read-only") || err.contains("query_only"),
        "a query must not write to the shared RW connection; got: {err}"
    );

    lumo_actions::db_ops::close_run_connections(run_id);
}

// ─── db.postgres_batch / db.mysql_batch:离线可验证的输入校验与网络门禁 ────────
// (事务提交/回滚/超时语义需要真库,见 tests/db_remote.rs 的 DSN 门控测试。)

#[tokio::test]
async fn remote_batch_rejects_empty_statements_before_connecting() {
    // 空 statements 在建池/连库之前就报错 —— 无需真库、无需 dsn 即可验证。
    for action in ["db.postgres_batch", "db.mysql_batch"] {
        let err = run_with(action, json!({"statements": []}), Capabilities::default())
            .await
            .unwrap_err();
        assert!(
            err.contains("at least one statement"),
            "{action} should reject an empty batch; got: {err}"
        );
    }
}

#[tokio::test]
async fn remote_batch_requires_dsn_when_unbound() {
    for action in ["db.postgres_batch", "db.mysql_batch"] {
        let err = run_with(
            action,
            json!({"statements": [{"sql": "SELECT 1"}]}),
            Capabilities::default(),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("dsn"),
            "{action} without a dsn should say so; got: {err}"
        );
    }
}

#[tokio::test]
async fn remote_batch_gates_dsn_host_by_network_capability() {
    // 门禁先于连接:未授权 host 直接拒绝,永远走不到 connect(与 *_exec 同一闸)。
    for (action, dsn) in [
        (
            "db.postgres_batch",
            "postgres://u:p@evil.example.com:5432/app",
        ),
        ("db.mysql_batch", "mysql://u:p@evil.example.com:3306/app"),
    ] {
        let err = run_with(
            action,
            json!({"dsn": dsn, "statements": [{"sql": "SELECT 1"}]}),
            Capabilities::default(),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("network") || err.contains("capability") || err.contains("denied"),
            "{action} must be network-gated; got: {err}"
        );
    }
}

#[tokio::test]
async fn remote_mutation_dry_runs_do_not_connect() {
    let caps = Capabilities {
        network: vec!["db.invalid".into()],
        ..Default::default()
    };
    for (action, dsn, input) in [
        (
            "db.postgres_exec",
            "postgres://u:p@db.invalid:5432/app",
            json!({"sql": "DELETE FROM t", "dry_run": true}),
        ),
        (
            "db.mysql_exec",
            "mysql://u:p@db.invalid:3306/app",
            json!({"sql": "DELETE FROM t", "dry_run": true}),
        ),
        (
            "db.postgres_batch",
            "postgres://u:p@db.invalid:5432/app",
            json!({"statements": [{"sql": "DELETE FROM t"}], "dry_run": true}),
        ),
        (
            "db.mysql_batch",
            "mysql://u:p@db.invalid:3306/app",
            json!({"statements": [{"sql": "DELETE FROM t"}], "dry_run": true}),
        ),
    ] {
        let mut input = input;
        input["dsn"] = json!(dsn);
        let out = run_with(action, input, caps.clone())
            .await
            .unwrap_or_else(|error| panic!("{action} dry-run should not connect: {error}"));
        assert_eq!(out["dry_run"], json!(true), "{action}");
    }
}
