//! Integration coverage for the `data.filter` / `data.group_by` / `data.join`
//! table action family (F-12). Sections are added alongside each action.

mod common;
use common::{ok, run};
use serde_json::{json, Value};

/// Small "people" fixture reused across the table-action cases.
fn people() -> Value {
    json!([
        {"name": "Alice", "age": 30, "dept": "eng",   "city": "NYC"},
        {"name": "Bob",   "age": 25, "dept": "eng"},
        {"name": "Carol", "age": 35, "dept": "sales", "city": "LA"},
        {"name": "Dave",  "age": 9,  "dept": "sales", "city": "NYC"}
    ])
}

/// Extract the `name` field from each row for compact assertions.
fn names(v: &Value) -> Vec<&str> {
    v.as_array()
        .expect("output is an array")
        .iter()
        .map(|r| r["name"].as_str().expect("row has a string name"))
        .collect()
}

// ---- data.filter ----------------------------------------------------------

#[tokio::test]
async fn filter_eq_and_ne() {
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "dept", "op": "eq", "value": "eng"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Bob"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "dept", "op": "ne", "value": "eng"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Carol", "Dave"]);
}

#[tokio::test]
async fn filter_numeric_comparisons_not_lexicographic() {
    // 30 > 9 must hold numerically; lexicographically "30" < "9".
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "age", "op": "gt", "value": 9}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Bob", "Carol"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "age", "op": "gte", "value": 30}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Carol"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "age", "op": "lt", "value": 30}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Bob", "Dave"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "age", "op": "lte", "value": 25}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Bob", "Dave"]);
}

#[tokio::test]
async fn filter_string_contains_starts_ends() {
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "name", "op": "contains", "value": "ar"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Carol"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "name", "op": "starts_with", "value": "A"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "name", "op": "ends_with", "value": "e"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Dave"]);
}

#[tokio::test]
async fn filter_contains_array_membership() {
    let out = ok(
        "data.filter",
        json!({
            "items": [
                {"id": 1, "tags": ["x", "y"]},
                {"id": 2, "tags": ["z"]}
            ],
            "where": [{"field": "tags", "op": "contains", "value": "x"}]
        }),
    )
    .await;
    assert_eq!(out, json!([{"id": 1, "tags": ["x", "y"]}]));
}

#[tokio::test]
async fn filter_in_and_not_in() {
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "dept", "op": "in", "value": ["eng"]}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Bob"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "dept", "op": "not_in", "value": ["eng"]}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Carol", "Dave"]);
}

#[tokio::test]
async fn filter_exists_and_not_exists() {
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "city", "op": "exists"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Carol", "Dave"]);

    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "city", "op": "not_exists"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Bob"]);
}

#[tokio::test]
async fn filter_multiple_predicates_are_anded() {
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [
                {"field": "dept", "op": "eq", "value": "eng"},
                {"field": "age",  "op": "gt", "value": 26}
            ]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice"]);
}

#[tokio::test]
async fn filter_missing_field_excludes_row_for_value_ops() {
    // Bob has no `city`; an eq predicate on city drops him (no match) rather
    // than erroring.
    let out = ok(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "city", "op": "eq", "value": "NYC"}]
        }),
    )
    .await;
    assert_eq!(names(&out), ["Alice", "Dave"]);
}

#[tokio::test]
async fn filter_empty_or_absent_where_returns_all() {
    let out = ok("data.filter", json!({"items": people(), "where": []})).await;
    assert_eq!(names(&out), ["Alice", "Bob", "Carol", "Dave"]);

    let out = ok("data.filter", json!({"items": people()})).await;
    assert_eq!(names(&out), ["Alice", "Bob", "Carol", "Dave"]);
}

#[tokio::test]
async fn filter_unknown_operator_errors() {
    let err = run(
        "data.filter",
        json!({
            "items": people(),
            "where": [{"field": "age", "op": "between", "value": 5}]
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("unknown operator") && err.contains("between"),
        "got: {err}"
    );
}
