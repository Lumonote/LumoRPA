//! Integration coverage for the `dialog.*` family.
//!
//! The harness attaches no host prompter, so every dialog action must fail with
//! a clear, uniform "no host" error instead of hanging, panicking, or returning
//! a silent empty value. That contract is the main thing this file pins: a
//! headless run must never deadlock on a dialog.

mod common;
use common::{ok, run};
use serde_json::json;

const NO_HOST: &str = "host does not support human interaction";

#[tokio::test]
async fn message_fails_cleanly_without_a_host() {
    let err = run("dialog.message", json!({ "message": "hi" }))
        .await
        .expect_err("no host means no dialog");
    assert!(err.contains(NO_HOST), "got: {err}");
}

#[tokio::test]
async fn every_dialog_action_reports_the_missing_host() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("dialog.message", json!({ "message": "hi" })),
        ("dialog.confirm", json!({ "message": "sure?" })),
        ("dialog.input", json!({ "message": "name?" })),
        (
            "dialog.select",
            json!({ "message": "pick", "options": ["a", "b"] }),
        ),
        ("dialog.select_file", json!({})),
        ("dialog.select_dir", json!({})),
        ("dialog.form", json!({ "fields": [{ "name": "x" }] })),
        ("dialog.custom", json!({ "spec": {} })),
    ];
    for (id, input) in cases {
        // Each must fail with the uniform no-host message, not hang or panic.
        let err = run(id, input)
            .await
            .err()
            .unwrap_or_else(|| panic!("`{id}` must not silently succeed without a host"));
        assert!(
            err.contains(NO_HOST),
            "`{id}` should report the missing host, got: {err}"
        );
    }
}

#[tokio::test]
async fn dialog_progress_is_pure_and_needs_no_host() {
    // Progress is a computation, not an interaction: it returns the normalised
    // fraction locally so flows can log it even with no UI attached.
    let out = ok(
        "dialog.progress",
        json!({ "message": "working", "value": 1, "total": 4 }),
    )
    .await;
    assert_eq!(out["fraction"], json!(0.25));
    // value/total echo back as floats (they are declared f64).
    assert_eq!(out["value"], json!(1.0));
    assert_eq!(out["total"], json!(4.0));

    // A bare fraction passes through unchanged.
    let out = ok("dialog.progress", json!({ "message": "x", "value": 0.5 })).await;
    assert_eq!(out["fraction"], json!(0.5));

    // No value at all is reported as unknown, not as 0.
    let out = ok("dialog.progress", json!({ "message": "x" })).await;
    assert_eq!(out["fraction"], json!(null));
}

#[tokio::test]
async fn dialog_progress_clamps_out_of_range_values() {
    let out = ok(
        "dialog.progress",
        json!({ "message": "x", "value": 9, "total": 4 }),
    )
    .await;
    assert_eq!(out["fraction"], json!(1.0), "over-complete clamps to 1");
    let out = ok("dialog.progress", json!({ "message": "x", "value": -3 })).await;
    assert_eq!(out["fraction"], json!(0.0), "negative clamps to 0");
    // total of 0 must not divide by zero.
    let out = ok(
        "dialog.progress",
        json!({ "message": "x", "value": 1, "total": 0 }),
    )
    .await;
    assert_eq!(out["fraction"], json!(1.0));
}

#[tokio::test]
async fn message_rejects_an_unknown_level_before_reaching_the_host() {
    let err = run(
        "dialog.message",
        json!({ "message": "hi", "level": "purple" }),
    )
    .await
    .expect_err("unknown level must be rejected");
    assert!(err.contains("unknown level"), "got: {err}");
}

#[tokio::test]
async fn select_requires_a_non_empty_option_list() {
    let err = run("dialog.select", json!({ "message": "pick", "options": [] }))
        .await
        .expect_err("empty options must be rejected");
    assert!(err.contains("must not be empty"), "got: {err}");
}

#[tokio::test]
async fn form_requires_at_least_one_field() {
    let err = run("dialog.form", json!({ "fields": [] }))
        .await
        .expect_err("empty fields must be rejected");
    assert!(err.contains("must not be empty"), "got: {err}");
}

#[tokio::test]
async fn dialog_actions_reject_unknown_input_keys() {
    // deny_unknown_fields keeps typos loud rather than silently ignored.
    let err = run("dialog.message", json!({ "message": "hi", "buton": "ok" }))
        .await
        .expect_err("typo keys must be rejected");
    assert!(err.contains("input invalid"), "got: {err}");
}
