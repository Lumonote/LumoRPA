//! Integration coverage for the `list.*` action family (P1-8).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn length_append_reverse() {
    assert_eq!(ok("list.length", json!({"items": [1, 2, 3]})).await, json!(3));
    assert_eq!(
        ok("list.append", json!({"items": [1, 2], "value": 3})).await,
        json!([1, 2, 3])
    );
    assert_eq!(
        ok("list.reverse", json!({"items": [1, 2, 3]})).await,
        json!([3, 2, 1])
    );
}

#[tokio::test]
async fn sort_ascending_descending_and_by_key() {
    assert_eq!(ok("list.sort", json!({"items": [3, 1, 2]})).await, json!([1, 2, 3]));
    assert_eq!(
        ok("list.sort", json!({"items": [3, 1, 2], "desc": true})).await,
        json!([3, 2, 1])
    );
    assert_eq!(
        ok("list.sort", json!({"items": [{"n": 2}, {"n": 1}], "by": "n"})).await,
        json!([{"n": 1}, {"n": 2}]),
        "by-key sorts arrays of objects"
    );
}

#[tokio::test]
async fn unique_preserves_first_seen_order() {
    assert_eq!(
        ok("list.unique", json!({"items": [1, 2, 1, 3, 2]})).await,
        json!([1, 2, 3])
    );
}

#[tokio::test]
async fn range_ascending_descending_and_step() {
    assert_eq!(ok("list.range", json!({"end": 4})).await, json!([0, 1, 2, 3]));
    assert_eq!(
        ok("list.range", json!({"start": 1, "end": 10, "step": 3})).await,
        json!([1, 4, 7])
    );
    assert_eq!(
        ok("list.range", json!({"start": 3, "end": 0, "step": -1})).await,
        json!([3, 2, 1])
    );
}

#[tokio::test]
async fn range_rejects_zero_step() {
    let err = run("list.range", json!({"end": 5, "step": 0})).await.unwrap_err();
    assert!(err.contains("step must not be 0"), "got: {err}");
}

#[tokio::test]
async fn contains_uses_deep_equality() {
    assert_eq!(
        ok("list.contains", json!({"items": [{"a": 1}], "value": {"a": 1}})).await,
        json!(true)
    );
    assert_eq!(
        ok("list.contains", json!({"items": [1, 2], "value": 3})).await,
        json!(false)
    );
}

#[tokio::test]
async fn get_handles_negatives_and_out_of_range() {
    assert_eq!(ok("list.get", json!({"items": ["a", "b", "c"], "index": 1})).await, json!("b"));
    assert_eq!(
        ok("list.get", json!({"items": ["a", "b", "c"], "index": -1})).await,
        json!("c"),
        "negative index counts from the end"
    );
    assert_eq!(
        ok("list.get", json!({"items": ["a"], "index": 9})).await,
        json!(null),
        "out-of-range yields null"
    );
}

#[tokio::test]
async fn slice_with_negatives() {
    assert_eq!(
        ok("list.slice", json!({"items": [0, 1, 2, 3, 4], "start": 1, "end": 3})).await,
        json!([1, 2])
    );
    assert_eq!(
        ok("list.slice", json!({"items": [0, 1, 2, 3, 4], "start": -2})).await,
        json!([3, 4]),
        "negative start counts from the end"
    );
}

#[tokio::test]
async fn pluck_extracts_field_or_null() {
    assert_eq!(
        ok("list.pluck", json!({"items": [{"id": 1}, {"id": 2}, {"x": 9}], "key": "id"})).await,
        json!([1, 2, null]),
        "missing key yields null for that element"
    );
}
