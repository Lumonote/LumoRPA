//! Coverage for `clipboard.get` / `clipboard.set` (S-class F-5). Input-validation
//! cases run in CI (they never touch the clipboard); the real round-trip needs a
//! display/clipboard backend and is `#[ignore]`d, run locally with `--ignored`.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn set_requires_text() {
    // Reaches input parsing (and proves the action is registered) without
    // touching the clipboard, so it is CI-safe even headless.
    let err = run("clipboard.set", json!({})).await.unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
#[ignore = "needs a real display/clipboard; run with --ignored"]
async fn set_then_get_round_trips() {
    ok("clipboard.set", json!({"text": "lumo-clip-test"})).await;
    let out = ok("clipboard.get", json!({})).await;
    assert_eq!(out["text"], json!("lumo-clip-test"));
}
