//! Integration coverage for the `list.*` extensions.
//!
//! The LumoRPA-native `list.*` family is covered by `list_ops.rs`; this file
//! pins the added compatibility-named actions.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn create_fills_every_element() {
    assert_eq!(
        ok("list.create", json!({ "length": 3 })).await,
        json!([null, null, null])
    );
    assert_eq!(
        ok("list.create", json!({ "length": 2, "value": 7 })).await,
        json!([7, 7])
    );
    assert_eq!(ok("list.create", json!({ "length": 0 })).await, json!([]));
}

#[tokio::test]
async fn get_elem_supports_negative_and_out_of_range() {
    assert_eq!(
        ok("list.get_elem", json!({ "items": [1, 2, 3], "index": 1 })).await,
        json!(2)
    );
    assert_eq!(
        ok("list.get_elem", json!({ "items": [1, 2, 3], "index": -1 })).await,
        json!(3),
        "negative index counts from the end"
    );
    assert_eq!(
        ok("list.get_elem", json!({ "items": [1], "index": 9 })).await,
        json!(null),
        "out of range gives null, not an error"
    );
    assert_eq!(
        ok(
            "list.get_elem",
            json!({ "items": [1], "index": 9, "default": "x" })
        )
        .await,
        json!("x")
    );
}

#[tokio::test]
async fn set_elem_writes_in_place_and_reports_out_of_range() {
    assert_eq!(
        ok(
            "list.set_elem",
            json!({ "items": [1, 2, 3], "index": 0, "value": 9 })
        )
        .await,
        json!([9, 2, 3])
    );
    assert_eq!(
        ok(
            "list.set_elem",
            json!({ "items": [1, 2, 3], "index": -1, "value": 9 })
        )
        .await,
        json!([1, 2, 9])
    );
    // Out of range fails unless explicitly allowed to append.
    let err = run(
        "list.set_elem",
        json!({ "items": [1], "index": 5, "value": 9 }),
    )
    .await
    .expect_err("out-of-range index must not silently no-op");
    assert!(err.contains("out of range"), "got: {err}");

    assert_eq!(
        ok(
            "list.set_elem",
            json!({ "items": [1], "index": 5, "value": 9, "extend": true })
        )
        .await,
        json!([1, 9])
    );
}

#[tokio::test]
async fn get_len_matches_the_native_length_action() {
    assert_eq!(
        ok("list.get_len", json!({ "items": [1, 2, 3] })).await,
        json!(3)
    );
    assert_eq!(ok("list.get_len", json!({ "items": [] })).await, json!(0));
    assert_eq!(
        ok("list.get_len", json!({ "items": [1, 2, 3] })).await,
        ok("list.length", json!({ "items": [1, 2, 3] })).await
    );
}

#[tokio::test]
async fn get_index_returns_minus_one_when_absent() {
    assert_eq!(
        ok(
            "list.get_index",
            json!({ "items": ["a", "b"], "value": "b" })
        )
        .await,
        json!(1)
    );
    assert_eq!(
        ok("list.get_index", json!({ "items": ["a"], "value": "z" })).await,
        json!(-1),
        "Semantics: -1 rather than null"
    );
}

#[tokio::test]
async fn append_or_insert_covers_both_modes() {
    assert_eq!(
        ok(
            "list.append_or_insert",
            json!({ "items": [1, 2], "value": 3 })
        )
        .await,
        json!([1, 2, 3]),
        "no position appends"
    );
    assert_eq!(
        ok(
            "list.append_or_insert",
            json!({ "items": [1, 3], "value": 2, "position": 1 })
        )
        .await,
        json!([1, 2, 3])
    );
    // A position past the end clamps to append rather than panicking.
    assert_eq!(
        ok(
            "list.append_or_insert",
            json!({ "items": [1], "value": 2, "position": 99 })
        )
        .await,
        json!([1, 2])
    );
    assert_eq!(
        ok(
            "list.append_or_insert",
            json!({ "items": [1, 2], "value": 0, "position": -1 })
        )
        .await,
        json!([1, 0, 2]),
        "negative position counts from the end"
    );
}

#[tokio::test]
async fn extend_appends_every_element() {
    assert_eq!(
        ok("list.extend", json!({ "items": [1], "other": [2, 3] })).await,
        json!([1, 2, 3])
    );
    assert_eq!(
        ok("list.extend", json!({ "items": [1], "other": [] })).await,
        json!([1])
    );
}

#[tokio::test]
async fn remove_all_or_bounded_count() {
    assert_eq!(
        ok("list.remove", json!({ "items": [1, 2, 1, 3], "value": 1 })).await,
        json!([2, 3]),
        "no count removes every occurrence"
    );
    assert_eq!(
        ok(
            "list.remove",
            json!({ "items": [1, 2, 1, 3], "value": 1, "count": 1 })
        )
        .await,
        json!([2, 1, 3])
    );
    assert_eq!(
        ok("list.remove", json!({ "items": [1], "value": 9 })).await,
        json!([1]),
        "removing an absent value is a no-op"
    );
}

#[tokio::test]
async fn remove_list_drops_every_listed_value() {
    assert_eq!(
        ok(
            "list.remove_list",
            json!({ "items": [1, 2, 3, 2], "values": [2, 3] })
        )
        .await,
        json!([1])
    );
    assert_eq!(
        ok("list.remove_list", json!({ "items": [1], "values": [] })).await,
        json!([1])
    );
}

#[tokio::test]
async fn clear_yields_an_empty_list() {
    assert_eq!(
        ok("list.clear", json!({ "items": [1, 2] })).await,
        json!([])
    );
}

#[tokio::test]
async fn remove_duplicate_keeps_first_seen_order() {
    assert_eq!(
        ok("list.remove_duplicate", json!({ "items": [3, 1, 3, 2, 1] })).await,
        json!([3, 1, 2])
    );
    assert_eq!(
        ok("list.remove_duplicate", json!({ "items": [] })).await,
        json!([])
    );
}

#[tokio::test]
async fn get_duplicate_lists_repeats_once_each() {
    assert_eq!(
        ok("list.get_duplicate", json!({ "items": [3, 1, 3, 2, 1, 1] })).await,
        json!([3, 1]),
        "each repeated value appears once, in first-seen order"
    );
    assert_eq!(
        ok("list.get_duplicate", json!({ "items": [1, 2, 3] })).await,
        json!([])
    );
}

#[tokio::test]
async fn shuffle_is_deterministic_for_a_seed_and_keeps_the_multiset() {
    let a = ok(
        "list.shuffle",
        json!({ "items": [1, 2, 3, 4, 5], "seed": 42 }),
    )
    .await;
    let b = ok(
        "list.shuffle",
        json!({ "items": [1, 2, 3, 4, 5], "seed": 42 }),
    )
    .await;
    assert_eq!(a, b, "the same seed must reproduce the same order");

    let mut sorted: Vec<i64> = a
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    sorted.sort_unstable();
    assert_eq!(sorted, vec![1, 2, 3, 4, 5], "shuffle is a permutation");

    // A different seed should not collapse to the identity every time.
    let c = ok(
        "list.shuffle",
        json!({ "items": [1, 2, 3, 4, 5], "seed": 7 }),
    )
    .await;
    assert_eq!(c.as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn shuffle_handles_degenerate_lengths() {
    assert_eq!(ok("list.shuffle", json!({ "items": [] })).await, json!([]));
    assert_eq!(
        ok("list.shuffle", json!({ "items": [1] })).await,
        json!([1])
    );
}
