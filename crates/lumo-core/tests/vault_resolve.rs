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

#[tokio::test]
async fn vm_resolves_vault_from_store_at_action_exec() {
    use lumo_actions::register_all;
    use lumo_core::{FlowVm, RunOptions};
    use lumo_dsl::parse_str;

    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    put_secret(&repo, &id, "smtp", "user", "alice@example.com");

    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, Some(repo.clone())).with_vault(Some(Arc::new(id)));

    // `{{ vault.smtp.user }}` is literalized to `${{ … }}` before template
    // render, then resolved from the store at action-exec time.
    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  vault: [smtp]
  steps:
    - id: read
      action: control.set_var
      with: { name: u, value: "{{ vault.smtp.user }}" }
"#,
    )
    .expect("parse");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run");
    assert!(report.success);
    let out = report.outputs.expect("outputs");
    assert_eq!(
        out.pointer("/read/result").and_then(Value::as_str),
        Some("alice@example.com")
    );
}

#[tokio::test]
async fn vault_secret_value_never_enters_persisted_input_hash() {
    // Security regression guard (P1-3): the persisted `step_runs.input_hash` is
    // computed from the `${{ vault.* }}` placeholder form BEFORE decryption (the
    // VM hashes `rendered_input`, then resolves the secret only into the value
    // handed to `action.execute`). So the hash MUST be independent of the secret
    // value. Running the same flow twice with two DIFFERENT stored secrets behind
    // the same placeholder must yield the SAME input_hash. If a future refactor
    // ever hashed the *resolved* input, these would diverge — and the decrypted
    // secret would be leaking into the persisted snapshot. This pins
    // hash(rendered) ahead of resolve().
    async fn input_hash_for_secret(secret: &str) -> Vec<u8> {
        use lumo_actions::register_all;
        use lumo_core::{FlowVm, RunOptions};
        use lumo_dsl::parse_str;

        let repo = Repo::open_in_memory().unwrap();
        let id = VaultIdentity::generate();
        put_secret(&repo, &id, "smtp", "user", secret);
        let mut reg = ActionRegistry::new();
        register_all(&mut reg);
        let vm = FlowVm::new(reg, Some(repo.clone())).with_vault(Some(Arc::new(id)));
        let flow = parse_str(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  vault: [smtp]
  steps:
    - id: read
      action: control.set_var
      with: { name: u, value: "{{ vault.smtp.user }}" }
"#,
        )
        .expect("parse");
        let report = vm.run(&flow, RunOptions::default()).await.expect("run");
        assert!(report.success);
        // Sanity: the secret really was resolved at action-exec time, so this
        // test exercises the resolution path rather than a no-op.
        assert_eq!(
            report
                .outputs
                .as_ref()
                .and_then(|o| o.pointer("/read/result"))
                .and_then(Value::as_str),
            Some(secret)
        );
        repo.list_steps(&report.run_id)
            .unwrap()
            .into_iter()
            .find(|s| s.step_id == "read")
            .expect("read step persisted")
            .input_hash
    }

    let hash_a = input_hash_for_secret("alice@example.com").await;
    let hash_b = input_hash_for_secret("a-totally-different-and-longer-secret@example.org").await;
    assert_eq!(
        hash_a, hash_b,
        "persisted input_hash must be secret-independent (hashed from the vault \
         placeholder form, not the decrypted value)"
    );
}
