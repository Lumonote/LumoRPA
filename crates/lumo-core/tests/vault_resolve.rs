//! P1-3: `${{ vault.* }}` resolution — env wins, encrypted store is the
//! fallback, graceful env-only degrade when no identity is present.

use lumo_core::{ActionRegistry, StepCtx};
use lumo_dsl::Capabilities;
use lumo_storage::{Repo, Vault, VaultIdentity};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

fn ctx_with(
    repo: Option<Repo>,
    identity: Option<Arc<VaultIdentity>>,
    names: Vec<String>,
) -> StepCtx {
    StepCtx::new(
        "run".into(),
        "flow".into(),
        ActionRegistry::new(),
        repo,
        Value::Null,
        Capabilities::default(),
        names,
    )
    .with_vault(identity)
}

fn put_secret(repo: &Repo, id: &VaultIdentity, name: &str, key: &str, val: &str) {
    let mut fields = BTreeMap::new();
    fields.insert(key.to_string(), val.to_string());
    Vault::new(repo, id).put(name, &fields).unwrap();
}

#[test]
fn env_wins_over_store() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    put_secret(&repo, &id, "envwin", "user", "from-store");
    std::env::set_var("LUMO_VAULT_ENVWIN_USER", "from-env");
    let ctx = ctx_with(Some(repo), Some(Arc::new(id)), vec!["envwin".into()]);
    let out = ctx
        .resolve_vault_placeholders(&json!("${{ vault.envwin.user }}"))
        .unwrap();
    assert_eq!(out, json!("from-env"));
    std::env::remove_var("LUMO_VAULT_ENVWIN_USER");
}

#[test]
fn store_used_when_env_absent() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    put_secret(&repo, &id, "storefb", "user", "from-store");
    let ctx = ctx_with(Some(repo), Some(Arc::new(id)), vec!["storefb".into()]);
    let out = ctx
        .resolve_vault_placeholders(&json!("${{ vault.storefb.user }}"))
        .unwrap();
    assert_eq!(out, json!("from-store"));
}

#[test]
fn scalar_secret_empty_key_from_store() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    put_secret(&repo, &id, "token", "", "abc123");
    let ctx = ctx_with(Some(repo), Some(Arc::new(id)), vec!["token".into()]);
    let out = ctx
        .resolve_vault_placeholders(&json!("${{ vault.token }}"))
        .unwrap();
    assert_eq!(out, json!("abc123"));
}

#[test]
fn missing_in_both_errors() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    let ctx = ctx_with(Some(repo), Some(Arc::new(id)), vec!["miss".into()]);
    let err = ctx
        .resolve_vault_placeholders(&json!("${{ vault.miss.user }}"))
        .unwrap_err();
    assert!(err.to_string().contains("is missing"));
}

#[test]
fn undeclared_name_errors() {
    let ctx = ctx_with(None, None, vec![]);
    let err = ctx
        .resolve_vault_placeholders(&json!("${{ vault.undeclared.user }}"))
        .unwrap_err();
    assert!(err.to_string().contains("not declared"));
}

#[test]
fn identity_absent_degrades_to_env() {
    std::env::set_var("LUMO_VAULT_DEGRADE_USER", "env-only");
    let ctx = ctx_with(None, None, vec!["degrade".into()]);
    let out = ctx
        .resolve_vault_placeholders(&json!("${{ vault.degrade.user }}"))
        .unwrap();
    assert_eq!(out, json!("env-only"));
    std::env::remove_var("LUMO_VAULT_DEGRADE_USER");
}
