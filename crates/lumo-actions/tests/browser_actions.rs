//! Coverage for the F-10 browser action completions (eval / screenshot / scroll
//! / hover / select / cookies / set_cookie). Input + capability validation runs
//! in CI (it errors before any Chrome session is needed); behavioural paths need
//! a real Chrome and are `#[ignore]`d, mirroring `browser_wait.rs`.

mod common;
use common::run;
use serde_json::json;

#[tokio::test]
async fn screenshot_gates_fs_write_before_session() {
    // fs-write is checked BEFORE the browser session, so an ungranted dest fails
    // with a capability error (not "browser not launched") and without a Chrome.
    let err = run("browser.screenshot", json!({ "path": "/tmp/lumo-shot.png" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("fs.write"),
        "expected an fs.write capability error, got: {err}"
    );
    assert!(
        !err.contains("not launched"),
        "fs gate must run before the session lookup, got: {err}"
    );
}

#[tokio::test]
async fn select_requires_value_label_or_index() {
    // All three targets absent → rejected before a session is needed.
    let err = run("browser.select", json!({ "selector": "#dropdown" }))
        .await
        .unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
}

#[tokio::test]
async fn eval_without_session_is_a_clean_error() {
    let err = run("browser.eval", json!({ "expr": "1 + 1" }))
        .await
        .unwrap_err();
    assert!(err.contains("not launched"), "got: {err}");
}

#[tokio::test]
async fn set_cookie_requires_name_and_value() {
    // `name`/`value` are required fields — the execute deserialize rejects a
    // missing one (the derived schema enforces the same in the VM path).
    let err = run("browser.set_cookie", json!({ "value": "v" }))
        .await
        .unwrap_err();
    assert!(err.contains("invalid"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn eval_and_cookies_roundtrip() {
    // Sketch for local e2e: browser.open a data: URL, browser.set_cookie, then
    // browser.eval "document.title" / browser.cookies reflect the page state, and
    // browser.scroll / browser.hover / browser.select drive a small fixture page.
}
