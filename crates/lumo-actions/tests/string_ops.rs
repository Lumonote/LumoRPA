//! Integration coverage for the `string.*` action family (P1-8).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn case_and_trim() {
    assert_eq!(
        ok("string.upper", json!({"text": "héllo"})).await,
        json!("HÉLLO")
    );
    assert_eq!(
        ok("string.lower", json!({"text": "HÉLLO"})).await,
        json!("héllo")
    );
    assert_eq!(
        ok("string.trim", json!({"text": "  hi \n"})).await,
        json!("hi")
    );
}

#[tokio::test]
async fn length_counts_chars_not_bytes() {
    // "héllo" is 5 chars but 6 bytes (é is 2 bytes in UTF-8).
    assert_eq!(
        ok("string.length", json!({"text": "héllo"})).await,
        json!(5)
    );
}

#[tokio::test]
async fn split_and_join_round_trip() {
    assert_eq!(
        ok("string.split", json!({"text": "a,b,c"})).await,
        json!(["a", "b", "c"])
    );
    assert_eq!(
        ok(
            "string.split",
            json!({"text": "a-b-c", "sep": "-", "limit": 2})
        )
        .await,
        json!(["a", "b-c"]),
        "limit caps the number of pieces"
    );
    assert_eq!(
        ok("string.join", json!({"items": ["a", "b", "c"], "sep": "/"})).await,
        json!("a/b/c")
    );
    // Non-string items stringify.
    assert_eq!(
        ok("string.join", json!({"items": [1, 2, 3]})).await,
        json!("1,2,3")
    );
}

#[tokio::test]
async fn replace_all_vs_once() {
    assert_eq!(
        ok(
            "string.replace",
            json!({"text": "a-a-a", "from": "a", "to": "x"})
        )
        .await,
        json!("x-x-x")
    );
    assert_eq!(
        ok(
            "string.replace",
            json!({"text": "a-a-a", "from": "a", "to": "x", "once": true})
        )
        .await,
        json!("x-a-a")
    );
}

#[tokio::test]
async fn contains_starts_ends() {
    assert_eq!(
        ok("string.contains", json!({"text": "hello", "needle": "ell"})).await,
        json!(true)
    );
    assert_eq!(
        ok(
            "string.contains",
            json!({"text": "Hello", "needle": "hell", "case_sensitive": false})
        )
        .await,
        json!(true),
        "case-insensitive match"
    );
    assert_eq!(
        ok(
            "string.contains",
            json!({"text": "Hello", "needle": "hell"})
        )
        .await,
        json!(false),
        "case-sensitive by default"
    );
    assert_eq!(
        ok(
            "string.starts_with",
            json!({"text": "foobar", "prefix": "foo"})
        )
        .await,
        json!(true)
    );
    assert_eq!(
        ok(
            "string.ends_with",
            json!({"text": "foobar", "suffix": "bar"})
        )
        .await,
        json!(true)
    );
}

#[tokio::test]
async fn substring_handles_negatives_and_order() {
    assert_eq!(
        ok(
            "string.substring",
            json!({"text": "abcdef", "start": 1, "end": 3})
        )
        .await,
        json!("bc")
    );
    assert_eq!(
        ok("string.substring", json!({"text": "abcdef", "start": -2})).await,
        json!("ef"),
        "negative start counts from the end"
    );
    assert_eq!(
        ok(
            "string.substring",
            json!({"text": "abcdef", "start": 4, "end": 1})
        )
        .await,
        json!("bcd"),
        "start/end are normalized into lo..hi"
    );
}

#[tokio::test]
async fn repeat_and_pad() {
    assert_eq!(
        ok("string.repeat", json!({"text": "ab", "times": 3})).await,
        json!("ababab")
    );
    assert_eq!(
        ok("string.repeat", json!({"text": "ab", "times": 0})).await,
        json!("")
    );
    assert_eq!(
        ok(
            "string.pad_left",
            json!({"text": "7", "width": 3, "pad": "0"})
        )
        .await,
        json!("007")
    );
    assert_eq!(
        ok("string.pad_right", json!({"text": "7", "width": 3})).await,
        json!("7  ")
    );
    assert_eq!(
        ok(
            "string.pad_left",
            json!({"text": "already wide", "width": 3})
        )
        .await,
        json!("already wide"),
        "already-wide strings are returned unchanged"
    );
}

#[tokio::test]
async fn format_replaces_placeholders() {
    assert_eq!(
        ok(
            "string.format",
            json!({"template": "Hi {name}, you are {age}", "values": {"name": "Ada", "age": 30}})
        )
        .await,
        json!("Hi Ada, you are 30")
    );
}

#[tokio::test]
async fn invalid_input_is_a_step_error() {
    // Missing required `text` field → action returns an error, not a panic.
    let err = run("string.upper", json!({"nope": 1})).await.unwrap_err();
    assert!(
        err.contains("string.upper"),
        "error names the action: {err}"
    );
}
