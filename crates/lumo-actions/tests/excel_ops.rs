//! Integration coverage for the `excel.*` actions (P1-8).
//! Writes a real `.xlsx` with `excel.write_row`, then reads it back with
//! `excel.read_rows`; both honor the fs sandbox.

mod common;
use common::{fs_caps, ok_with, run};
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

#[tokio::test]
async fn sheet_names_and_cell_read_write_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("book.xlsx");
    let caps = fs_caps(dir.path());

    let wrote = ok_with(
        "excel.write_cell",
        json!({"file": file, "sheet": "Data", "cell": "B2", "value": "hello"}),
        caps.clone(),
    )
    .await;
    assert_eq!(wrote["sheet"], json!("Data"));
    assert_eq!(wrote["cell"], json!("B2"));

    let names = ok_with("excel.sheet_names", json!({"file": file}), caps.clone()).await;
    assert_eq!(names, json!({"sheets": ["Data"]}));

    let value = ok_with(
        "excel.read_cell",
        json!({"file": file, "sheet": "Data", "cell": "B2"}),
        caps,
    )
    .await;
    assert_eq!(value, json!("hello"));
}

#[tokio::test]
async fn write_range_then_read_range_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("grid.xlsx");
    let caps = fs_caps(dir.path());

    let wrote = ok_with(
        "excel.write_range",
        json!({
            "file": file,
            "sheet": "S",
            "cell": "B2",
            "values": [["a", "b"], ["c", "d"]]
        }),
        caps.clone(),
    )
    .await;
    assert_eq!(wrote["cells"], json!(4));

    let read = ok_with(
        "excel.read_range",
        json!({"file": file, "sheet": "S", "range": "B2:C3"}),
        caps,
    )
    .await;
    assert_eq!(read, json!([["a", "b"], ["c", "d"]]));
}

#[tokio::test]
async fn find_replace_rewrites_matching_cells() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("fr.xlsx");
    let caps = fs_caps(dir.path());

    ok_with(
        "excel.write_range",
        json!({"file": file, "sheet": "S", "cell": "A1", "values": [["foo bar"], ["baz"]]}),
        caps.clone(),
    )
    .await;
    let out = ok_with(
        "excel.find_replace",
        json!({"file": file, "sheet": "S", "find": "foo", "replace": "qux"}),
        caps.clone(),
    )
    .await;
    assert_eq!(out["replaced"], json!(1));

    let read = ok_with(
        "excel.read_range",
        json!({"file": file, "sheet": "S", "range": "A1:A2"}),
        caps,
    )
    .await;
    assert_eq!(read, json!([["qux bar"], ["baz"]]));
}

#[tokio::test]
async fn set_formula_writes_a_formula_cell() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("formula.xlsx");
    let caps = fs_caps(dir.path());

    let out = ok_with(
        "excel.set_formula",
        json!({"file": file, "sheet": "S", "cell": "A3", "formula": "SUM(A1:A2)"}),
        caps.clone(),
    )
    .await;
    assert_eq!(out["cell"], json!("A3"));
    // calamine reports the cached value of an unevaluated formula (empty here),
    // so we only assert the write succeeded and the workbook re-opens cleanly.
    let names = ok_with("excel.sheet_names", json!({"file": file}), caps).await;
    assert_eq!(names, json!({"sheets": ["S"]}));
}

#[tokio::test]
async fn read_range_denied_without_fs_grant() {
    let err = run(
        "excel.read_range",
        json!({"file": "/etc/whatever.xlsx", "range": "A1:B2"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
