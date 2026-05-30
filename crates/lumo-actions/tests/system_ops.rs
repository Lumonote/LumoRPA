//! Integration coverage for the `system.*` action family (P1-8).
//! `system.shell` is checked via its default-deny gate (no `LUMO_ALLOW_SHELL`),
//! not by actually spawning a process.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn platform_reports_this_host() {
    let out = ok("system.platform", json!({})).await;
    assert_eq!(out["os"], json!(std::env::consts::OS));
    assert_eq!(out["arch"], json!(std::env::consts::ARCH));
    assert_eq!(out["family"], json!(std::env::consts::FAMILY));
}

#[tokio::test]
async fn env_get_reads_a_present_var() {
    // PATH is set in every sane test environment.
    let out = ok("system.env_get", json!({"name": "PATH"})).await;
    assert!(
        !out.as_str().unwrap().is_empty(),
        "PATH should be non-empty"
    );
}

#[tokio::test]
async fn env_get_falls_back_to_default_then_empty() {
    assert_eq!(
        ok(
            "system.env_get",
            json!({"name": "LUMO_NO_SUCH_VAR_X9", "default": "fallback"})
        )
        .await,
        json!("fallback")
    );
    assert_eq!(
        ok("system.env_get", json!({"name": "LUMO_NO_SUCH_VAR_X9"})).await,
        json!(""),
        "missing with no default is the empty string"
    );
}

#[tokio::test]
async fn sleep_returns_the_duration_it_waited() {
    assert_eq!(
        ok("system.sleep", json!({"ms": 5})).await,
        json!({"slept_ms": 5})
    );
}

#[tokio::test]
async fn shell_is_denied_without_the_opt_in() {
    // Without LUMO_ALLOW_SHELL=1 the action must refuse rather than spawn.
    let err = run("system.shell", json!({"command": "echo hi"}))
        .await
        .unwrap_err();
    assert!(err.contains("disabled"), "got: {err}");
}
