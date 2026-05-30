//! Integration tests for the encrypted vault (P1-3): age crypto primitives,
//! Repo CRUD, and the Vault façade.

use lumo_storage::vault;
use lumo_storage::Repo;
use lumo_storage::VaultIdentity;
// NOTE(P1-3): `Vault` and `BTreeMap` are used by the Vault-façade tests added
// in Task 3. They stay commented out here because the `Vault` type does not
// exist yet (Task 3); restore them when that task appends its tests.
// use lumo_storage::Vault;
// use std::collections::BTreeMap;

#[test]
fn crypto_encrypt_decrypt_roundtrip() {
    let id = VaultIdentity::generate();
    let ct = vault::encrypt(&id.recipient(), b"hunter2").unwrap();
    assert_ne!(ct, b"hunter2", "ciphertext must not equal plaintext");
    let pt = vault::decrypt(&id, &ct).unwrap();
    assert_eq!(pt, b"hunter2");
}

#[test]
fn crypto_wrong_identity_cannot_decrypt() {
    let a = VaultIdentity::generate();
    let b = VaultIdentity::generate();
    let ct = vault::encrypt(&a.recipient(), b"secret").unwrap();
    assert!(vault::decrypt(&b, &ct).is_err());
}

#[test]
fn crypto_save_then_load_roundtrips_and_chmods_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("age-identity.txt");
    let id = VaultIdentity::generate();
    id.save(&path).unwrap();

    // A reloaded identity must decrypt what the original encrypted.
    let reloaded = VaultIdentity::load(&path).unwrap();
    let ct = vault::encrypt(&id.recipient(), b"x").unwrap();
    assert_eq!(vault::decrypt(&reloaded, &ct).unwrap(), b"x");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "identity file must be 0600");
    }
}

#[test]
fn repo_put_get_roundtrip() {
    let repo = Repo::open_in_memory().unwrap();
    repo.vault_put("smtp", b"\x01\x02ciphertext", r#"{"keys":["user"]}"#, 1_700_000_000_000)
        .unwrap();
    let row = repo.vault_get("smtp").unwrap().expect("row present");
    assert_eq!(row.name, "smtp");
    assert_eq!(row.age_ciphertext, b"\x01\x02ciphertext");
    assert_eq!(row.metadata, r#"{"keys":["user"]}"#);
    assert_eq!(row.updated_at, 1_700_000_000_000);
}

#[test]
fn repo_put_is_upsert() {
    let repo = Repo::open_in_memory().unwrap();
    repo.vault_put("smtp", b"old", "{}", 1).unwrap();
    repo.vault_put("smtp", b"new", "{}", 2).unwrap();
    let row = repo.vault_get("smtp").unwrap().unwrap();
    assert_eq!(row.age_ciphertext, b"new");
    assert_eq!(row.updated_at, 2);
    assert_eq!(repo.vault_list().unwrap().len(), 1, "upsert, not insert");
}

#[test]
fn repo_get_missing_is_none() {
    let repo = Repo::open_in_memory().unwrap();
    assert!(repo.vault_get("nope").unwrap().is_none());
}

#[test]
fn repo_list_is_sorted_by_name() {
    let repo = Repo::open_in_memory().unwrap();
    repo.vault_put("b", b"x", "{}", 1).unwrap();
    repo.vault_put("a", b"x", "{}", 1).unwrap();
    let names: Vec<String> = repo.vault_list().unwrap().into_iter().map(|r| r.name).collect();
    assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn repo_delete_removes_row() {
    let repo = Repo::open_in_memory().unwrap();
    repo.vault_put("smtp", b"x", "{}", 1).unwrap();
    repo.vault_delete("smtp").unwrap();
    assert!(repo.vault_get("smtp").unwrap().is_none());
}
