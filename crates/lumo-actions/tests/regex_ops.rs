//! Integration coverage for the `regex.*` action family (P1-8).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn match_tests_partial() {
    assert_eq!(
        ok("regex.match", json!({"pattern": "\\d+", "text": "abc123"})).await,
        json!(true)
    );
    assert_eq!(
        ok("regex.match", json!({"pattern": "^\\d+$", "text": "abc123"})).await,
        json!(false)
    );
}

#[tokio::test]
async fn find_all_returns_every_match() {
    assert_eq!(
        ok("regex.find_all", json!({"pattern": "\\d+", "text": "a1b22c333"})).await,
        json!(["1", "22", "333"])
    );
}

#[tokio::test]
async fn replace_all_and_once_with_capture_refs() {
    assert_eq!(
        ok("regex.replace", json!({"pattern": "a", "text": "banana", "replacement": "X"})).await,
        json!("bXnXnX")
    );
    assert_eq!(
        ok("regex.replace", json!({"pattern": "a", "text": "banana", "replacement": "X", "once": true})).await,
        json!("bXnana")
    );
    assert_eq!(
        ok("regex.replace", json!({"pattern": "(\\w+)@(\\w+)", "text": "user@host", "replacement": "$2.$1"})).await,
        json!("host.user"),
        "$1/$2 capture references expand"
    );
}

#[tokio::test]
async fn captures_returns_full_groups_and_named() {
    let out = ok(
        "regex.captures",
        json!({"pattern": "(?P<year>\\d{4})-(\\d{2})", "text": "2026-05"}),
    )
    .await;
    assert_eq!(out["full"], json!("2026-05"));
    assert_eq!(out["groups"], json!(["2026", "05"]));
    assert_eq!(out["named"]["year"], json!("2026"));
}

#[tokio::test]
async fn captures_no_match_is_null() {
    assert_eq!(
        ok("regex.captures", json!({"pattern": "\\d+", "text": "no digits"})).await,
        json!(null)
    );
}

#[tokio::test]
async fn invalid_pattern_is_an_error() {
    let err = run("regex.match", json!({"pattern": "(unclosed", "text": "x"})).await.unwrap_err();
    assert!(err.contains("regex compile error"), "got: {err}");
}
