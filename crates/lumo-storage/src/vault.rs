//! Encrypted vault (P1-3): age (X25519) crypto primitives + a `Repo`-backed
//! façade.
//!
//! Each namespace is stored as one age-encrypted JSON object `{key -> value}`.
//! The identity (private key) lives in a file outside the DB; only an opaque
//! handle is threaded through the VM, so `lumo-core` never links `age`.

use crate::error::StorageError;
use age::secrecy::ExposeSecret;
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
