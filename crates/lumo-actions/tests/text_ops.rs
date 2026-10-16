//! Integration coverage for the `text.*` action family (/ parity).

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn append_text_in_newline_appends_exactly_one_newline() {
    assert_eq!(
        ok(
            "text.append_text_in_newline",
            json!({ "text": "a", "append": "b" })
        )
        .await,
        json!("ab\n")
    );
    // Already-terminated text is not given a second newline.
    assert_eq!(
        ok(
            "text.append_text_in_newline",
            json!({ "text": "a\n", "append": "b" })
        )
        .await,
        json!("a\nb\n")
    );
}

#[tokio::test]
async fn change_text_case_covers_every_documented_mode() {
    let cases = [
        ("uppercase", "Hello World", "HELLO WORLD"),
        ("lowercase", "Hello World", "hello world"),
        ("titlecase", "hello world", "Hello World"),
        ("capitalize", "hello world", "Hello world"),
        ("swapcase", "aBc", "AbC"),
    ];
    for (mode, input, want) in cases {
        assert_eq!(
            ok(
                "text.change_text_case",
                json!({ "text": input, "case": mode })
            )
            .await,
            json!(want),
            "case `{mode}`"
        );
    }
}

#[tokio::test]
async fn change_text_case_rejects_an_unknown_mode() {
    let err = run(
        "text.change_text_case",
        json!({ "text": "x", "case": "sideways" }),
    )
    .await
    .expect_err("unknown case must fail loudly");
    assert!(err.contains("unknown case"), "got: {err}");
}

#[tokio::test]
async fn extract_content_first_match_all_and_capture_group() {
    assert_eq!(
        ok(
            "text.extract_content_from_text",
            json!({ "text": "a1 b2", "pattern": "[0-9]" })
        )
        .await,
        json!("1")
    );
    assert_eq!(
        ok(
            "text.extract_content_from_text",
            json!({ "text": "a1 b2", "pattern": "[0-9]", "all": true })
        )
        .await,
        json!(["1", "2"])
    );
    assert_eq!(
        ok(
            "text.extract_content_from_text",
            json!({ "text": "id=42", "pattern": "id=([0-9]+)", "group": 1 })
        )
        .await,
        json!("42")
    );
}

#[tokio::test]
async fn extract_content_returns_null_when_nothing_matches() {
    assert_eq!(
        ok(
            "text.extract_content_from_text",
            json!({ "text": "abc", "pattern": "[0-9]" })
        )
        .await,
        json!(null)
    );
    assert_eq!(
        ok(
            "text.extract_content_from_text",
            json!({ "text": "abc", "pattern": "[0-9]", "all": true })
        )
        .await,
        json!([])
    );
}

#[tokio::test]
async fn extract_content_reports_an_invalid_pattern() {
    let err = run(
        "text.extract_content_from_text",
        json!({ "text": "abc", "pattern": "([unclosed" }),
    )
    .await
    .expect_err("a bad regex must not panic");
    assert!(err.contains("invalid regular expression"), "got: {err}");
}

#[tokio::test]
async fn text_length_counts_characters_not_bytes() {
    assert_eq!(
        ok("text.get_text_length", json!({ "text": "中文" })).await,
        json!(2),
        "two CJK characters are length 2, not 6 bytes"
    );
    assert_eq!(
        ok("text.get_text_length", json!({ "text": "abc" })).await,
        json!(3)
    );
    assert_eq!(
        ok("text.get_text_length", json!({ "text": "" })).await,
        json!(0)
    );
    // A keycap sequence is 1 grapheme but 3 chars; document the char semantics.
    assert_eq!(
        ok("text.get_text_length", json!({ "text": "😀" })).await,
        json!(1)
    );
}

#[tokio::test]
async fn join_list_to_text_uses_the_separator() {
    assert_eq!(
        ok(
            "text.join_list_to_text",
            json!({ "items": ["a", "b", "c"], "separator": "," })
        )
        .await,
        json!("a,b,c")
    );
    assert_eq!(
        ok("text.join_list_to_text", json!({ "items": ["a", "b"] })).await,
        json!("ab"),
        "separator defaults to empty"
    );
    // Non-string scalars are stringified rather than rejected.
    assert_eq!(
        ok(
            "text.join_list_to_text",
            json!({ "items": [1, true, null], "separator": "-" })
        )
        .await,
        json!("1-true-null")
    );
}

#[tokio::test]
async fn pad_text_directions_and_overflow() {
    assert_eq!(
        ok(
            "text.pad_text",
            json!({ "text": "7", "length": 3, "fill": "0" })
        )
        .await,
        json!("007"),
        "left is the default direction"
    );
    assert_eq!(
        ok(
            "text.pad_text",
            json!({ "text": "7", "length": 3, "direction": "right", "fill": "0" })
        )
        .await,
        json!("700")
    );
    assert_eq!(
        ok(
            "text.pad_text",
            json!({ "text": "7", "length": 5, "direction": "both", "fill": "-" })
        )
        .await,
        json!("--7--")
    );
    // Already long enough passes through untouched.
    assert_eq!(
        ok(
            "text.pad_text",
            json!({ "text": "abcdef", "length": 3, "fill": "0" })
        )
        .await,
        json!("abcdef")
    );
}

#[tokio::test]
async fn pad_text_counts_cjk_characters() {
    // "中文" is 2 characters, so padding to 4 adds exactly 2 fillers.
    assert_eq!(
        ok(
            "text.pad_text",
            json!({ "text": "中文", "length": 4, "fill": "*" })
        )
        .await,
        json!("**中文")
    );
}

#[tokio::test]
async fn replace_content_first_all_and_capture_references() {
    assert_eq!(
        ok(
            "text.replace_content_from_text",
            json!({ "text": "a1 b2", "pattern": "[0-9]", "replacement": "#" })
        )
        .await,
        json!("a# b2"),
        "first occurrence only by default"
    );
    assert_eq!(
        ok(
            "text.replace_content_from_text",
            json!({ "text": "a1 b2", "pattern": "[0-9]", "replacement": "#", "all": true })
        )
        .await,
        json!("a# b#")
    );
    assert_eq!(
        ok(
            "text.replace_content_from_text",
            json!({ "text": "id=42", "pattern": "id=([0-9]+)", "replacement": "no=$1" })
        )
        .await,
        json!("no=42")
    );
}

#[tokio::test]
async fn split_text_by_separator_whitespace_and_remove_empty() {
    assert_eq!(
        ok(
            "text.split_text_to_list",
            json!({ "text": "a,b,c", "separator": "," })
        )
        .await,
        json!(["a", "b", "c"])
    );
    // Default separator splits on any run of whitespace.
    assert_eq!(
        ok("text.split_text_to_list", json!({ "text": "a  b\tc" })).await,
        json!(["a", "b", "c"])
    );
    assert_eq!(
        ok(
            "text.split_text_to_list",
            json!({ "text": "a,,b", "separator": ",", "remove_empty": true })
        )
        .await,
        json!(["a", "b"])
    );
    assert_eq!(
        ok(
            "text.split_text_to_list",
            json!({ "text": "a,,b", "separator": "," })
        )
        .await,
        json!(["a", "", "b"]),
        "empty segments are kept unless asked otherwise"
    );
}

#[tokio::test]
async fn sub_text_handles_negative_and_out_of_range_indices() {
    assert_eq!(
        ok(
            "text.sub_text",
            json!({ "text": "abcdef", "start": 1, "length": 3 })
        )
        .await,
        json!("bcd")
    );
    assert_eq!(
        ok("text.sub_text", json!({ "text": "abcdef", "start": 2 })).await,
        json!("cdef"),
        "omitted length takes the remainder"
    );
    assert_eq!(
        ok("text.sub_text", json!({ "text": "abcdef", "start": -2 })).await,
        json!("ef"),
        "negative start counts from the end"
    );
    // Out-of-range clamps to an empty string instead of panicking.
    assert_eq!(
        ok("text.sub_text", json!({ "text": "abc", "start": 99 })).await,
        json!("")
    );
    assert_eq!(
        ok("text.sub_text", json!({ "text": "abc", "start": -99 })).await,
        json!("abc")
    );
}

#[tokio::test]
async fn sub_text_counts_cjk_characters() {
    assert_eq!(
        ok(
            "text.sub_text",
            json!({ "text": "中文字", "start": 1, "length": 2 })
        )
        .await,
        json!("文字")
    );
}

#[tokio::test]
async fn trim_text_modes_and_explicit_characters() {
    assert_eq!(
        ok("text.trim_text", json!({ "text": "  hi  " })).await,
        json!("hi")
    );
    assert_eq!(
        ok(
            "text.trim_text",
            json!({ "text": "  hi  ", "mode": "left" })
        )
        .await,
        json!("hi  ")
    );
    assert_eq!(
        ok(
            "text.trim_text",
            json!({ "text": "  hi  ", "mode": "right" })
        )
        .await,
        json!("  hi")
    );
    assert_eq!(
        ok("text.trim_text", json!({ "text": "xxhixx", "chars": "x" })).await,
        json!("hi"),
        "explicit chars replace the default whitespace set"
    );
}

#[tokio::test]
async fn trim_text_rejects_an_unknown_mode() {
    let err = run("text.trim_text", json!({ "text": "x", "mode": "middle" }))
        .await
        .expect_err("unknown mode must fail loudly");
    assert!(err.contains("unknown mode"), "got: {err}");
}
