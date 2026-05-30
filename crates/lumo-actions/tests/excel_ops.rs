//! Integration coverage for the `excel.*` actions (P1-8).
//! Writes a real `.xlsx` with `excel.write_row`, then reads it back with
//! `excel.read_rows`; both honor the fs sandbox.

mod common;
use common::{fs_caps, ok_with};
use serde_json::json;

#[tokio::test]
async fn write_then_read_round_trips_a_row() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("book.xlsx");
    let caps = fs_caps(dir.path());

    let wrote = ok_with(
        "excel.write_row",
        json!({"file": file, "row": ["alice", "bob"], "headers": ["first", "last"]}),
        caps.clone(),
    )
    .await;
    assert_eq!(wrote, json!({"rows": 1}));

    let rows = ok_with(
        "excel.read_rows",
        json!({"file": file, "header": true}),
        caps,
    )
    .await;
    assert_eq!(
        rows,
        json!([{"first": "alice", "last": "bob", "_index": 0}])
    );
}

#[tokio::test]
async fn write_row_appends_to_an_existing_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("book.xlsx");
    let caps = fs_caps(dir.path());

    ok_with(
        "excel.write_row",
        json!({"file": file, "row": ["alice", "bob"], "headers": ["first", "last"]}),
        caps.clone(),
    )
    .await;
    let second = ok_with(
        "excel.write_row",
        json!({"file": file, "row": ["carol", "dave"], "headers": ["first", "last"]}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        second,
        json!({"rows": 2}),
        "second append reports two data rows"
    );

    let rows = ok_with(
        "excel.read_rows",
        json!({"file": file, "header": true}),
        caps,
    )
    .await;
    assert_eq!(
        rows,
        json!([
            {"first": "alice", "last": "bob", "_index": 0},
            {"first": "carol", "last": "dave", "_index": 1}
        ])
    );
}
