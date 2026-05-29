//! Integration coverage for the `db.*` SQLite action family (P1-8).
//! A temp database is built with `db.sqlite_exec` then read with
//! `db.sqlite_query`; reads honor `fs.read`, writes honor `fs.write`.

mod common;
use common::{fs_caps, ok_with, run_with};
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
    assert_eq!(rows["truncated"], json!(true), "hitting the limit flags truncation");
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
