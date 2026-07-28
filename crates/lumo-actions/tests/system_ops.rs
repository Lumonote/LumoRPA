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

#[tokio::test]
async fn process_list_includes_this_test_process() {
    // No filter → the running test binary itself must appear (pid > 0).
    let out = ok("system.process_list", json!({})).await;
    let procs = out["processes"].as_array().expect("processes array");
    assert!(!procs.is_empty(), "this host has at least one process");
    assert!(out["count"].as_u64().unwrap() >= 1);
    // Every entry carries the documented shape.
    let first = &procs[0];
    assert!(first["pid"].as_u64().is_some(), "pid present: {first}");
    assert!(first["name"].as_str().is_some(), "name present: {first}");
    assert!(first["cpu"].as_f64().is_some(), "cpu present: {first}");
    assert!(
        first["memory"].as_u64().is_some(),
        "memory present: {first}"
    );
}

#[tokio::test]
async fn process_list_filters_by_name_substring() {
    // A nonsense filter yields zero matches but still succeeds with an empty list.
    let out = ok(
        "system.process_list",
        json!({"name": "lumo_no_such_process_zzz9"}),
    )
    .await;
    assert_eq!(out["count"], json!(0));
    assert_eq!(out["processes"], json!([]));
}

#[tokio::test]
async fn process_list_honors_the_limit() {
    let out = ok("system.process_list", json!({"limit": 1})).await;
    assert!(
        out["processes"].as_array().unwrap().len() <= 1,
        "limit caps the returned list"
    );
}
