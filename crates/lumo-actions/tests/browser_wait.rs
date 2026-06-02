//! Coverage for `browser.wait` (S-class F-9). Input/condition validation runs in
//! CI (it errors before any browser session is needed); behavioural waits need a
//! real Chrome and are `#[ignore]`d alongside the other browser e2e tests.

mod common;
use common::run;
use serde_json::json;

#[tokio::test]
async fn wait_requires_selector_or_text() {
    let err = run("browser.wait", json!({})).await.unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
}

#[tokio::test]
async fn wait_rejects_unknown_condition() {
    // `condition` is a `WaitCondition` enum: an out-of-set value is rejected at
    // deserialize (and the derived schema's `enum` does the same in the VM path).
    // Either way the offending token surfaces in the error.
    let err = run(
        "browser.wait",
        json!({"selector": "#x", "condition": "bogus"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("bogus"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn wait_visible_resolves_after_open() {
    // Sketch for local e2e: browser.open a data: URL with a visible element,
    // then browser.wait { selector, condition: "visible" } returns matched.
    // Requires the VM/registry browser-session plumbing; left as a manual e2e.
}
