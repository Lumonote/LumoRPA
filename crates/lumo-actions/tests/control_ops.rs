//! Integration coverage for the leaf behavior of `control.*` actions (P1-8).
//! Loop/branch *orchestration* (do:/else:/catch:/finally:) is driven by the VM
//! and is covered in lumo-core; here we pin each action's own contract.

mod common;
use common::{ctx_with, ok, run, Capabilities};
use serde_json::json;

#[tokio::test]
async fn if_reports_truthiness() {
    for truthy in [
        json!(true),
        json!(1),
        json!("yes"),
        json!([1]),
        json!({"a": 1}),
    ] {
        assert_eq!(
            ok("control.if", json!({"cond": truthy.clone()})).await,
            json!(true),
            "{truthy} should be truthy"
        );
    }
    for falsy in [
        json!(false),
        json!(0),
        json!(null),
        json!(""),
        json!("false"),
        json!("0"),
        json!("no"),
        json!([]),
        json!({}),
    ] {
        assert_eq!(
            ok("control.if", json!({"cond": falsy.clone()})).await,
            json!(false),
            "{falsy} should be falsy"
        );
    }
}

#[tokio::test]
async fn fail_propagates_a_user_message() {
    let err = run("control.fail", json!({"message": "boom"}))
        .await
        .unwrap_err();
    assert!(err.contains("boom"), "got: {err}");
    // An empty message falls back to a default.
    let err = run("control.fail", json!({})).await.unwrap_err();
    assert!(err.contains("user fail"), "got: {err}");
}

#[tokio::test]
async fn set_var_writes_into_the_context() {
    let mut ctx = ctx_with(Capabilities::default());
    let action = ctx
        .lookup_action("control.set_var")
        .expect("control.set_var registered");
    let out = action
        .execute(&mut ctx, json!({"name": "greeting", "value": "hi"}))
        .await
        .unwrap();
    assert_eq!(out.output, json!("hi"), "returns the value it set");
    assert_eq!(
        ctx.vars_snapshot()["greeting"],
        json!("hi"),
        "and records it under vars"
    );
}

#[tokio::test]
async fn log_echoes_message_and_level() {
    assert_eq!(
        ok("control.log", json!({"message": "hello"})).await,
        json!({"message": "hello", "level": "info"}),
        "level defaults to info"
    );
    assert_eq!(
        ok("control.log", json!({"message": "uh-oh", "level": "warn"})).await,
        json!({"message": "uh-oh", "level": "warn"})
    );
}

#[tokio::test]
async fn sleep_returns_null() {
    assert_eq!(ok("control.sleep", json!({"ms": 1})).await, json!(null));
}

#[tokio::test]
async fn loop_and_block_markers_are_noops() {
    // The VM drives do:/else:/catch: children; the actions themselves are null
    // markers that only validate their own config.
    assert_eq!(ok("control.for", json!({"to": 3})).await, json!(null));
    assert_eq!(
        ok("control.for_each", json!({"in": [1, 2, 3]})).await,
        json!(null)
    );
    assert_eq!(ok("control.parallel", json!({})).await, json!(null));
    assert_eq!(ok("control.try", json!({})).await, json!(null));
    // `control.for` still requires its bound — bad config is rejected.
    let err = run("control.for", json!({"step": 1})).await.unwrap_err();
    assert!(err.contains("for input invalid"), "got: {err}");
}
