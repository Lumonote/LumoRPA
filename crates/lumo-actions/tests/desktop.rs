//! F-1 `desktop.*` coverage. The CI-runnable tests exercise only the capability
//! gate + input validation — each returns at the gate or parse stage, **before**
//! `spawn_blocking`/`rdev::simulate`, so running them never moves the mouse or
//! types. Real actuation is `#[ignore]` (needs a display + macOS Accessibility
//! grant), mirroring the browser e2e convention.
#![cfg(feature = "desktop")]

mod common;
use common::{ok_with, run, run_with, Capabilities};
use serde_json::json;

/// Grant every desktop category (`*`).
fn desktop_caps() -> Capabilities {
    Capabilities {
        desktop: vec!["*".into()],
        ..Default::default()
    }
}

// ─── capability gate (CI; fail before touching rdev) ────────────────────────

#[tokio::test]
async fn move_denies_without_desktop_grant() {
    let err = run("desktop.move", json!({ "x": 10.0, "y": 20.0 }))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("desktop"), "got: {err}");
}

#[tokio::test]
async fn click_denies_without_desktop_grant() {
    let err = run("desktop.click", json!({ "x": 1.0, "y": 2.0 }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

#[tokio::test]
async fn type_denies_without_desktop_grant() {
    let err = run("desktop.type", json!({ "text": "你好" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// Category granularity: a `mouse`-only grant must not unlock keyboard actions.
#[tokio::test]
async fn keyboard_action_denied_with_only_mouse_grant() {
    let caps = Capabilities {
        desktop: vec!["mouse".into()],
        ..Default::default()
    };
    let err = run_with("desktop.key", json!({ "keys": "ctrl+c" }), caps)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

// ─── input validation (CI; granted, but fail before actuation) ──────────────

#[tokio::test]
async fn click_rejects_unknown_field() {
    let err = run_with(
        "desktop.click",
        json!({ "x": 1.0, "y": 2.0, "bogus": true }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn click_rejects_one_sided_coordinates() {
    // x without y is ambiguous → rejected before any actuation.
    let err = run_with("desktop.click", json!({ "x": 5.0 }), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("both x and y"), "got: {err}");
}

#[tokio::test]
async fn key_rejects_unknown_token() {
    let err = run_with(
        "desktop.key",
        json!({ "keys": "ctrl+nope" }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown key token"), "got: {err}");
}

#[tokio::test]
async fn key_rejects_modifier_only_combo() {
    let err = run_with("desktop.key", json!({ "keys": "ctrl+" }), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("no non-modifier key"), "got: {err}");
}

#[tokio::test]
async fn type_rejects_unknown_field() {
    let err = run_with(
        "desktop.type",
        json!({ "text": "hi", "bogus": 1 }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

// ─── actuation (e2e; #[ignore] — needs a display + Accessibility grant) ──────

#[tokio::test]
#[ignore = "actuates real input; needs a display + macOS Accessibility grant"]
async fn move_actuates() {
    let out = ok_with("desktop.move", json!({ "x": 100.0, "y": 100.0 }), desktop_caps()).await;
    assert_eq!(out.get("x").and_then(|v| v.as_f64()), Some(100.0));
}

#[tokio::test]
#[ignore = "actuates real input; needs a display + macOS Accessibility grant"]
async fn type_actuates_unicode() {
    // "你好 hello" = 8 Unicode scalar values; clipboard-paste handles the CJK.
    let out = ok_with("desktop.type", json!({ "text": "你好 hello" }), desktop_caps()).await;
    assert_eq!(out.get("typed").and_then(|v| v.as_u64()), Some(8));
}
