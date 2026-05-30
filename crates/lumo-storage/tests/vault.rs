//! Integration tests for the encrypted vault (P1-3): age crypto primitives,
//! Repo CRUD, and the Vault façade.

use lumo_storage::vault;
use lumo_storage::VaultIdentity;
// NOTE(P1-3): `Repo`, `Vault`, and `BTreeMap` are used by the Repo-CRUD and
// Vault-façade tests added in Tasks 2/3. They are commented out here because
// the `Vault` type does not exist yet (Task 3); uncomment when those tasks
// append their tests.
// use lumo_storage::{Repo, Vault};
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
