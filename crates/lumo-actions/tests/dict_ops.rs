//! Integration coverage for the `dictionary.*` action family (/ parity).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn create_preserves_insertion_order_and_overwrites_duplicates() {
    assert_eq!(
        ok(
            "dictionary.create",
            json!({ "entries": [ { "key": "b", "value": 2 }, { "key": "a", "value": 1 } ] })
        )
        .await,
        json!({ "b": 2, "a": 1 })
    );

    // A repeated key overwrites in place rather than appending a second entry.
    let dup = ok(
        "dictionary.create",
        json!({ "entries": [ { "key": "b", "value": 2 }, { "key": "b", "value": 3 } ] }),
    )
    .await;
    assert_eq!(dup, json!({ "b": 3 }));
    assert_eq!(dup.as_object().unwrap().len(), 1);
}

#[tokio::test]
async fn create_from_no_entries_is_an_empty_dictionary() {
    assert_eq!(ok("dictionary.create", json!({})).await, json!({}));
}

#[tokio::test]
async fn get_value_returns_the_value_or_the_default() {
    assert_eq!(
        ok(
            "dictionary.get_value",
            json!({ "dictionary": { "a": 1 }, "key": "a" })
        )
        .await,
        json!(1)
    );

    // Missing key: `default` when given, `null` when omitted — never an error.
    assert_eq!(
        ok(
            "dictionary.get_value",
            json!({ "dictionary": { "a": 1 }, "key": "zzz", "default": "n/a" })
        )
        .await,
        json!("n/a")
    );
    assert_eq!(
        ok(
            "dictionary.get_value",
            json!({ "dictionary": { "a": 1 }, "key": "zzz" })
        )
        .await,
        json!(null)
    );
}

#[tokio::test]
async fn get_value_rejects_a_non_dictionary_with_a_readable_error() {
    let err = run(
        "dictionary.get_value",
        json!({ "dictionary": [1, 2], "key": "a" }),
    )
    .await
    .expect_err("a list is not a dictionary");
    assert!(
        err.contains("expected a dictionary"),
        "error should name the expected shape, got: {err}"
    );
}

#[tokio::test]
async fn get_keys_follows_insertion_order() {
    assert_eq!(
        ok(
            "dictionary.get_keys",
            json!({ "dictionary": { "z": 1, "m": 2, "a": 3 } })
        )
        .await,
        json!(["z", "m", "a"])
    );
    assert_eq!(
        ok("dictionary.get_keys", json!({ "dictionary": {} })).await,
        json!([])
    );
}

#[tokio::test]
async fn get_values_all_and_by_key_subset() {
    assert_eq!(
        ok(
            "dictionary.get_values",
            json!({ "dictionary": { "a": 1, "b": 2 } })
        )
        .await,
        json!([1, 2])
    );

    // Explicit `keys` selects and orders; unknown keys yield null (no error).
    assert_eq!(
        ok(
            "dictionary.get_values",
            json!({ "dictionary": { "a": 1, "b": 2 }, "keys": ["b", "missing", "a"] })
        )
        .await,
        json!([2, null, 1])
    );
}

#[tokio::test]
async fn set_inserts_and_overwrites() {
    assert_eq!(
        ok(
            "dictionary.set",
            json!({ "dictionary": { "a": 1 }, "key": "b", "value": 2 })
        )
        .await,
        json!({ "a": 1, "b": 2 })
    );
    assert_eq!(
        ok(
            "dictionary.set",
            json!({ "dictionary": { "a": 1 }, "key": "a", "value": 9 })
        )
        .await,
        json!({ "a": 9 })
    );
}

#[tokio::test]
async fn set_can_add_a_key_to_an_empty_dictionary() {
    assert_eq!(
        ok(
            "dictionary.set",
            json!({ "dictionary": {}, "key": "k", "value": true })
        )
        .await,
        json!({ "k": true })
    );
}

#[tokio::test]
async fn pop_removes_the_key_and_returns_value_plus_remainder() {
    let out = ok(
        "dictionary.pop",
        json!({ "dictionary": { "a": 1, "b": 2 }, "key": "a" }),
    )
    .await;
    assert_eq!(out["value"], json!(1));
    assert_eq!(out["dictionary"], json!({ "b": 2 }));
}

#[tokio::test]
async fn pop_on_a_missing_key_is_not_an_error() {
    let out = ok(
        "dictionary.pop",
        json!({ "dictionary": { "a": 1 }, "key": "nope", "default": "fallback" }),
    )
    .await;
    assert_eq!(out["value"], json!("fallback"));
    assert_eq!(out["dictionary"], json!({ "a": 1 }));
}

#[tokio::test]
async fn pop_nested_values_round_trip() {
    let out = ok(
        "dictionary.pop",
        json!({ "dictionary": { "cfg": { "deep": [1, 2] } }, "key": "cfg" }),
    )
    .await;
    assert_eq!(out["value"], json!({ "deep": [1, 2] }));
    assert_eq!(out["dictionary"], json!({}));
}
