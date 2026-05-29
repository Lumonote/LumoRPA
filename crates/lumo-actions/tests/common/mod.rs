//! Shared harness for `lumo-actions` integration tests (P1-8).
//!
//! Each `tests/<module>.rs` file is its own crate; they pull this in with
//! `mod common;` and drive built-in actions through the real registry — the
//! same path the VM uses — then assert on the returned JSON `output`.

#![allow(dead_code)] // each test crate uses only the helpers it needs

use lumo_core::{ActionRegistry, StepCtx};
pub use lumo_dsl::Capabilities;
use serde_json::Value;

/// Build a `StepCtx` carrying `caps` and a registry with every built-in action
/// registered (so registry-driven actions like `control.*`/`skill.*` resolve).
pub fn ctx_with(caps: Capabilities) -> StepCtx {
    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    StepCtx::new(
        "run-test".into(),
        "flow-test".into(),
        reg,
        None,
        Value::Null,
        caps,
        Vec::new(),
    )
}

/// Invoke `id` with `input` against a no-capability context, returning its JSON
/// output. Panics if the action is not registered (a real regression worth a
/// loud failure). Action-level errors map to `Err(message)`.
pub async fn run(id: &str, input: Value) -> Result<Value, String> {
    run_with(id, input, Capabilities::default()).await
}

/// Like [`run`] but with an explicit capability sandbox — for `file.*`,
/// `system.*`, `http.*`, `db.*` which gate on fs/network/process grants.
pub async fn run_with(id: &str, input: Value, caps: Capabilities) -> Result<Value, String> {
    let mut reg = ActionRegistry::new();
    lumo_actions::register_all(&mut reg);
    let action = reg
        .get(id)
        .unwrap_or_else(|| panic!("action `{id}` is not registered"));
    let mut ctx = ctx_with(caps);
    action
        .execute(&mut ctx, input)
        .await
        .map(|r| r.output)
        .map_err(|e| e.to_string())
}

/// Convenience: assert `id`(`input`) succeeds and return the output, with a
/// failure message that includes the error.
pub async fn ok(id: &str, input: Value) -> Value {
    match run(id, input).await {
        Ok(v) => v,
        Err(e) => panic!("`{id}` should succeed but errored: {e}"),
    }
}

/// Like [`ok`] but with an explicit capability sandbox.
pub async fn ok_with(id: &str, input: Value, caps: Capabilities) -> Value {
    match run_with(id, input, caps).await {
        Ok(v) => v,
        Err(e) => panic!("`{id}` should succeed but errored: {e}"),
    }
}

/// Grant read+write over everything under `dir` — the standard sandbox for the
/// tempdir-based `file.*`/`csv.*`/`db.*`/`excel.*` tests.
pub fn fs_caps(dir: &std::path::Path) -> Capabilities {
    let glob = format!("{}/**", dir.display());
    Capabilities {
        fs_read: vec![glob.clone()],
        fs_write: vec![glob],
        ..Default::default()
    }
}
