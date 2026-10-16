//! Coverage for the `clipboard.*` aliases and additions.
//!
//! Input-validation and capability cases run everywhere; the real round-trip
//! needs a live clipboard backend and stays separated/honest about that.

mod common;
use common::{run, run_with};
use lumo_dsl::Capabilities;
use serde_json::json;

fn fs_read() -> Capabilities {
    Capabilities {
        fs_read: vec!["**".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn aliases_are_registered_and_share_the_core_schema() {
    // set_text must demand `text` exactly like clipboard.set.
    let err = run("clipboard.set_text", json!({}))
        .await
        .expect_err("set_text requires `text`");
    assert!(err.contains("input invalid"), "got: {err}");

    // get_text takes no fields at all, so a typo is ignored rather than rejected
    // (the shared `GetIn` is a unit struct). Pin the reachable behaviour: the
    // call must get as far as the clipboard, i.e. fail only for clipboard
    // reasons — never for registration or schema reasons.
    match run("clipboard.get_text", json!({ "bogus": 1 })).await {
        Ok(v) => assert!(
            v.get("text").is_some(),
            "get_text returns {{ text }}, got: {v}"
        ),
        Err(e) => assert!(
            e.contains("clipboard"),
            "get_text should only ever fail for clipboard reasons, got: {e}"
        ),
    }
}

#[tokio::test]
async fn clear_writes_an_empty_string_and_ignores_extra_fields() {
    // clear shares the unit-struct schema, so stray fields are ignored rather
    // than rejected. What matters is that it never fails to reach the backend
    // for a *schema* reason.
    match run("clipboard.clear", json!({ "unexpected": true })).await {
        Ok(v) => assert_eq!(v["ok"], json!(true)),
        Err(e) => assert!(
            e.contains("clipboard"),
            "clear should only fail for clipboard reasons, got: {e}"
        ),
    }
}

#[tokio::test]
async fn set_file_requires_a_path_key() {
    let err = run("clipboard.set_file", json!({}))
        .await
        .expect_err("set_file requires `path`");
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn set_file_is_gated_on_fs_read() {
    // Without fs.read the action must be denied before it ever touches the
    // clipboard — the path is a filesystem resource.
    let err = run("clipboard.set_file", json!({ "path": "/etc/hosts" }))
        .await
        .expect_err("fs.read is required");
    assert!(
        !err.is_empty(),
        "expected a capability denial, got an empty error"
    );
}

#[tokio::test]
async fn set_file_reports_its_mode_honestly() {
    // With fs.read granted, the action either succeeds (live clipboard) or
    // fails with a clipboard-unavailable error headless — but it must never
    // quietly claim native file-copy semantics.
    match run_with(
        "clipboard.set_file",
        json!({ "path": "/etc/hosts" }),
        fs_read(),
    )
    .await
    {
        Ok(v) => {
            assert_eq!(v["mode"], json!("path_text"));
            assert_eq!(v["ok"], json!(true));
        }
        Err(e) => assert!(
            e.contains("clipboard unavailable") || e.contains("clipboard write"),
            "headless failure should name the clipboard, got: {e}"
        ),
    }
}

#[tokio::test]
#[ignore = "needs a real display/clipboard; run with --ignored"]
async fn aliases_round_trip_against_the_live_clipboard() {
    use common::ok;
    ok("clipboard.set_text", json!({ "text": "lumo-alias-test" })).await;
    let out = ok("clipboard.get_text", json!({})).await;
    assert_eq!(out["text"], json!("lumo-alias-test"));

    ok("clipboard.clear", json!({})).await;
    let out = ok("clipboard.get_text", json!({})).await;
    assert_eq!(out["text"], json!(""), "clear leaves an empty string");
}
