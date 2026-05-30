//! Encrypted vault (P1-3): age (X25519) crypto primitives + a `Repo`-backed
//! façade.
//!
//! Each namespace is stored as one age-encrypted JSON object `{key -> value}`.
//! The identity (private key) lives in a file outside the DB; only an opaque
//! handle is threaded through the VM, so `lumo-core` never links `age`.

use crate::error::StorageError;
use crate::repo::Repo;
use age::secrecy::ExposeSecret;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::Path;

/// Opaque handle around an age X25519 identity (private key). `Clone` so the
/// VM can hold an `Arc<VaultIdentity>` and hand out `&VaultIdentity`.
#[derive(Clone)]
pub struct VaultIdentity(age::x25519::Identity);

impl VaultIdentity {
    /// Generate a fresh random identity.
    pub fn generate() -> Self {
        Self(age::x25519::Identity::generate())
    }

    /// Load an identity from a file holding an `AGE-SECRET-KEY-1…` string.
    pub fn load(path: &Path) -> Result<Self, StorageError> {
        let raw = std::fs::read_to_string(path)?;
        let id = raw
            .trim()
            .parse::<age::x25519::Identity>()
            .map_err(|e| StorageError::Crypto(format!("parse identity: {e}")))?;
        Ok(Self(id))
    }

    /// Serialize the secret key to `path` with `0600` perms (unix), creating
    /// parent directories as needed. On unix the file is created atomically at
    /// `0600` (via `OpenOptions::mode`) so the secret is never world-readable,
    /// even briefly; `set_permissions` also re-tightens a pre-existing file.
    pub fn save(&self, path: &Path) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let secret = self.0.to_string();
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(secret.expose_secret().as_bytes())?;
            // Belt-and-suspenders: `mode` only applies on create, so re-tighten
            // in case `path` already existed with looser perms.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, secret.expose_secret().as_bytes())?;
        }
        Ok(())
    }

    /// The public recipient for this identity.
    pub fn recipient(&self) -> age::x25519::Recipient {
        self.0.to_public()
    }

    /// The public key as an `age1…` string (safe to print / share).
    pub fn public_string(&self) -> String {
        self.0.to_public().to_string()
    }
}

/// Encrypt `plaintext` to `recipient`, returning age binary ciphertext.
pub fn encrypt(
    recipient: &age::x25519::Recipient,
    plaintext: &[u8],
) -> Result<Vec<u8>, StorageError> {
    age::encrypt(recipient, plaintext).map_err(|e| StorageError::Crypto(e.to_string()))
}

/// Decrypt age `ciphertext` with `identity`.
pub fn decrypt(identity: &VaultIdentity, ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
    age::decrypt(&identity.0, ciphertext).map_err(|e| StorageError::Crypto(e.to_string()))
}

/// Non-sensitive listing of a stored item (no ciphertext, no plaintext).
#[derive(Debug, Clone)]
pub struct VaultListed {
    pub name: String,
    pub keys: Vec<String>,
    pub updated_at: i64,
}

/// A `Repo`-backed encrypted vault. Borrows the repo + identity; reads decrypt
/// on demand, writes encrypt before hitting the DB.
pub struct Vault<'a> {
    repo: &'a Repo,
    identity: &'a VaultIdentity,
}

impl<'a> Vault<'a> {
    pub fn new(repo: &'a Repo, identity: &'a VaultIdentity) -> Self {
        Self { repo, identity }
    }

    /// Encrypt `fields` as a JSON object under `name` (UPSERT). `metadata`
    /// stores only the field names so `list` never has to decrypt.
    pub fn put(&self, name: &str, fields: &BTreeMap<String, String>) -> Result<(), StorageError> {
        let plaintext = serde_json::to_vec(fields)?;
        let ciphertext = encrypt(&self.identity.recipient(), &plaintext)?;
        let keys: Vec<&String> = fields.keys().collect();
        let metadata = serde_json::to_string(&serde_json::json!({ "keys": keys }))?;
        let updated_at = Utc::now().timestamp_millis();
        self.repo
            .vault_put(name, &ciphertext, &metadata, updated_at)
    }

    /// Decrypt and return all fields under `name`, or `None` if absent.
    pub fn get(&self, name: &str) -> Result<Option<BTreeMap<String, String>>, StorageError> {
        let Some(row) = self.repo.vault_get(name)? else {
            return Ok(None);
        };
        let plaintext = decrypt(self.identity, &row.age_ciphertext)?;
        let fields: BTreeMap<String, String> = serde_json::from_slice(&plaintext)?;
        Ok(Some(fields))
    }

    pub fn delete(&self, name: &str) -> Result<(), StorageError> {
        self.repo.vault_delete(name)
    }
}

/// List stored items without an identity — metadata only, never decrypts.
pub fn list(repo: &Repo) -> Result<Vec<VaultListed>, StorageError> {
    let rows = repo.vault_list()?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        // `put` always writes valid JSON here, so a parse failure means the
        // metadata column was corrupted or tampered with — fail loudly.
        let meta: serde_json::Value = serde_json::from_str(&r.metadata)?;
        let keys = meta
            .get("keys")
            .and_then(|k| k.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push(VaultListed {
            name: r.name,
            keys,
            updated_at: r.updated_at,
        });
    }
    Ok(out)
}

/// Decrypt a namespace and return one field for runtime resolution (called by
/// `lumo-core`'s `${{ vault.* }}` resolver). One encrypted blob holds the whole
/// namespace, so this decrypts it and selects `key`. `Ok(None)` if the item or
/// key is absent.
pub fn get_field(
    repo: &Repo,
    identity: &VaultIdentity,
    name: &str,
    key: &str,
) -> Result<Option<String>, StorageError> {
    match Vault::new(repo, identity).get(name)? {
        Some(fields) => Ok(fields.get(key).cloned()),
        None => Ok(None),
    }
}
