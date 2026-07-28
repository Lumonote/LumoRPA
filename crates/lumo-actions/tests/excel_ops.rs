//! Integration coverage for the `excel.*` actions (P1-8).
//! Writes a real `.xlsx` with `excel.write_row`, then reads it back with
//! `excel.read_rows`; both honor the fs sandbox.

mod common;
use common::{fs_caps, ok_with, run, run_bound};
use serde_json::json;

#[test]
fn every_excel_action_exposes_timeout_ms() {
    let mut registry = lumo_core::ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    let excel_ids: Vec<_> = registry
        .iter_ids()
        .filter(|id| id.starts_with("excel."))
        .collect();
    assert!(!excel_ids.is_empty());
    for id in excel_ids {
        let schema = registry.get(&id).unwrap().schema().clone();
        assert!(
            schema["properties"].get("timeout_ms").is_some(),
            "{id} must expose timeout_ms"
        );
    }
}

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
        rows["rows"],
        json!([{"first": "alice", "last": "bob", "_index": 0}])
    );
    assert_eq!(rows["truncated"], json!(false));
}

#[tokio::test]
async fn bound_xlsx_resource_supplies_file_for_repeated_actions() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bound.xlsx");
    let caps = fs_caps(dir.path());
    let resource = format!("kind: xlsx\npath: {}\n", file.display());
    let resources = [("book", resource.as_str())];

    run_bound(
        "run-xlsx-resource",
        &resources,
        Some("book"),
        "excel.write_cell",
        json!({"sheet": "Data", "cell": "A1", "value": "shared"}),
        caps.clone(),
    )
    .await
    .expect("bound write should use the resource path");

    let value = run_bound(
        "run-xlsx-resource",
        &resources,
        Some("book"),
        "excel.read_cell",
        json!({"sheet": "Data", "cell": "A1"}),
        caps,
    )
    .await
    .expect("bound read should reuse the resource path");
    assert_eq!(value, json!("shared"));
}

#[tokio::test]
async fn read_rows_limit_returns_metadata_and_truncates() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bounded.xlsx");
    let caps = fs_caps(dir.path());
    for value in ["a", "b", "c"] {
        ok_with(
            "excel.write_row",
            json!({"file": file, "row": [value], "headers": ["value"]}),
            caps.clone(),
        )
        .await;
    }

    let out = ok_with(
        "excel.read_rows",
        json!({"file": file, "header": true, "limit": 2}),
        caps,
    )
    .await;
    assert_eq!(out["count"], json!(2));
    assert_eq!(out["truncated"], json!(true));
    assert_eq!(out["rows"].as_array().unwrap().len(), 2);
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
        rows["rows"],
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
async fn worksheet_and_axis_structure_actions_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("structure.xlsx");
    let caps = fs_caps(dir.path());
    ok_with(
        "excel.write_range",
        json!({"file": file, "sheet": "S", "cell": "A1", "values": [["a", "b"], ["c", "d"]]}),
        caps.clone(),
    )
    .await;

    ok_with(
        "excel.add_sheet",
        json!({"file": file, "name": "Extra"}),
        caps.clone(),
    )
    .await;
    ok_with(
        "excel.rename_sheet",
        json!({"file": file, "sheet": "Extra", "new_name": "Renamed"}),
        caps.clone(),
    )
    .await;

    ok_with(
        "excel.insert_rows",
        json!({"file": file, "sheet": "S", "row": 1, "count": 1}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with(
            "excel.read_cell",
            json!({"file": file, "sheet": "S", "cell": "A2"}),
            caps.clone()
        )
        .await,
        json!("a")
    );
    ok_with(
        "excel.delete_rows",
        json!({"file": file, "sheet": "S", "row": 1, "count": 1}),
        caps.clone(),
    )
    .await;
    ok_with(
        "excel.insert_columns",
        json!({"file": file, "sheet": "S", "column": "A", "count": 1}),
        caps.clone(),
    )
    .await;
    assert_eq!(
        ok_with(
            "excel.read_cell",
            json!({"file": file, "sheet": "S", "cell": "B1"}),
            caps.clone()
        )
        .await,
        json!("a")
    );
    ok_with(
        "excel.delete_columns",
        json!({"file": file, "sheet": "S", "column": "A", "count": 1}),
        caps.clone(),
    )
    .await;
    ok_with(
        "excel.delete_sheet",
        json!({"file": file, "sheet": "Renamed"}),
        caps.clone(),
    )
    .await;

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

// ─── typed error classification (retry.on contract) ─────────────────────────

#[tokio::test]
async fn read_rows_missing_file_classifies_as_io() {
    let dir = tempfile::tempdir().unwrap();
    let kind = common::err_kind_with(
        "excel.read_rows",
        json!({"file": dir.path().join("absent.xlsx")}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Io);
}

#[tokio::test]
async fn read_cell_missing_file_classifies_as_io() {
    let dir = tempfile::tempdir().unwrap();
    let kind = common::err_kind_with(
        "excel.read_cell",
        json!({"file": dir.path().join("absent.xlsx"), "cell": "A1"}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Io);
}

#[tokio::test]
async fn bad_cell_ref_stays_kind_other() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("wb.xlsx");
    ok_with(
        "excel.write_row",
        json!({"file": file, "row": ["a"]}),
        fs_caps(dir.path()),
    )
    .await;
    let kind = common::err_kind_with(
        "excel.read_cell",
        json!({"file": file, "cell": "not-a-cell"}),
        fs_caps(dir.path()),
    )
    .await;
    assert_eq!(kind, lumo_core::error::ErrorKind::Other);
}
