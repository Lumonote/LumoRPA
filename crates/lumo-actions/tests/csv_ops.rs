//! Integration coverage for the `csv.*` action family (P1-8).
//! Pure parse/stringify run capability-free; `csv.read`/`csv.write` round-trip
//! through a tempdir under an explicit fs sandbox.

mod common;
use common::{fs_caps, ok, ok_with, run};
use serde_json::json;

#[tokio::test]
async fn parse_with_headers_yields_objects() {
    assert_eq!(
        ok("csv.parse", json!({"text": "a,b\n1,2\n", "headers": true})).await,
        json!([{"a": "1", "b": "2"}])
    );
}

#[tokio::test]
async fn parse_without_headers_yields_rows() {
    assert_eq!(
        ok("csv.parse", json!({"text": "a,b\n1,2\n"})).await,
        json!([["a", "b"], ["1", "2"]])
    );
}

#[tokio::test]
async fn parse_handles_quoted_fields_with_embedded_separator() {
    assert_eq!(
        ok("csv.parse", json!({"text": "\"x,y\",z\n"})).await,
        json!([["x,y", "z"]])
    );
}

#[tokio::test]
async fn stringify_arrays_and_objects() {
    assert_eq!(
        ok("csv.stringify", json!({"value": [["a", "b"], ["1", "2"]]})).await,
        json!("a,b\n1,2\n")
    );
    assert_eq!(
        ok("csv.stringify", json!({"value": [{"a": "1", "b": "2"}]})).await,
        json!("a,b\n1,2\n"),
        "objects derive a header row from first-seen keys"
    );
}

#[tokio::test]
async fn stringify_quotes_fields_that_need_it() {
    assert_eq!(
        ok("csv.stringify", json!({"value": [["x,y", "z"]]})).await,
        json!("\"x,y\",z\n")
    );
}

#[tokio::test]
async fn stringify_rejects_non_array_input() {
    let err = run("csv.stringify", json!({"value": 5})).await.unwrap_err();
    assert!(err.contains("expected an array"), "got: {err}");
}

#[tokio::test]
async fn write_then_read_round_trips_through_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.csv");
    let caps = fs_caps(dir.path());

    let wrote = ok_with(
        "csv.write",
        json!({"path": path, "value": [["a", "b"], ["1", "2"]]}),
        caps.clone(),
    )
    .await;
    assert_eq!(wrote["rows"], json!(2));

    let read = ok_with(
        "csv.read",
        json!({"path": path, "headers": true}),
        caps,
    )
    .await;
    assert_eq!(read, json!([{"a": "1", "b": "2"}]));
}

#[tokio::test]
async fn read_is_denied_without_an_fs_grant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.csv");
    let err = run("csv.read", json!({"path": path})).await.unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
