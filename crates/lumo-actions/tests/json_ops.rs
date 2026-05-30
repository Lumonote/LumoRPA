//! Integration coverage for the `json.*` action family (P1-8).

mod common;
use common::ok;
use serde_json::json;

#[tokio::test]
async fn get_dotted_path_with_array_index() {
    let value = json!({"a": {"b": [10, 20, 30]}});
    assert_eq!(
        ok("json.get", json!({"value": value, "path": "a.b.1"})).await,
        json!(20)
    );
}

#[tokio::test]
async fn get_missing_path_falls_back_to_default() {
    let value = json!({"a": 1});
    assert_eq!(
        ok(
            "json.get",
            json!({"value": value, "path": "a.missing.deep", "default": 99})
        )
        .await,
        json!(99)
    );
}

#[tokio::test]
async fn set_creates_nested_objects() {
    assert_eq!(
        ok("json.set", json!({"value": {}, "path": "a.b", "new": 7})).await,
        json!({"a": {"b": 7}})
    );
}

#[tokio::test]
async fn set_extends_array_index() {
    assert_eq!(
        ok(
            "json.set",
            json!({"value": {"a": [0, 0]}, "path": "a.3", "new": 9})
        )
        .await,
        json!({"a": [0, 0, null, 9]}),
        "indices past the end backfill with null"
    );
}

#[tokio::test]
async fn merge_is_shallow_with_b_winning() {
    assert_eq!(
        ok(
            "json.merge",
            json!({"a": {"x": 1, "y": 2}, "b": {"y": 9, "z": 3}})
        )
        .await,
        json!({"x": 1, "y": 9, "z": 3})
    );
}

#[tokio::test]
async fn keys_and_values_of_object() {
    let value = json!({"a": 1, "b": 2});
    assert_eq!(
        ok("json.keys", json!({"value": value.clone()})).await,
        json!(["a", "b"])
    );
    assert_eq!(
        ok("json.values", json!({"value": value})).await,
        json!([1, 2])
    );
    // Non-objects yield an empty array rather than erroring.
    assert_eq!(ok("json.keys", json!({"value": 5})).await, json!([]));
}

#[tokio::test]
async fn delete_removes_key() {
    assert_eq!(
        ok(
            "json.delete",
            json!({"value": {"a": 1, "b": 2}, "path": "b"})
        )
        .await,
        json!({"a": 1})
    );
}
