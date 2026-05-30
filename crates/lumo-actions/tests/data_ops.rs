//! Integration coverage for the `data.*` JSON actions (P1-8).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn json_parse_reads_a_value() {
    assert_eq!(
        ok(
            "data.json_parse",
            json!({"text": "{\"a\":[1,2],\"b\":true}"})
        )
        .await,
        json!({"a": [1, 2], "b": true})
    );
}

#[tokio::test]
async fn json_parse_rejects_malformed() {
    let err = run("data.json_parse", json!({"text": "{not json"}))
        .await
        .unwrap_err();
    assert!(err.contains("json parse error"), "got: {err}");
}

#[tokio::test]
async fn json_format_compact_and_pretty() {
    assert_eq!(
        ok("data.json_format", json!({"value": {"a": 1}})).await,
        json!("{\"a\":1}"),
        "compact by default"
    );
    let pretty = ok(
        "data.json_format",
        json!({"value": {"a": 1}, "pretty": true}),
    )
    .await;
    assert!(
        pretty.as_str().unwrap().contains('\n'),
        "pretty output is multi-line: {pretty}"
    );
}

#[tokio::test]
async fn json_round_trips_through_parse_and_format() {
    let original = json!({"nums": [1, 2, 3], "nested": {"ok": true}});
    let text = ok("data.json_format", json!({"value": original.clone()})).await;
    let back = ok("data.json_parse", json!({"text": text})).await;
    assert_eq!(back, original);
}
