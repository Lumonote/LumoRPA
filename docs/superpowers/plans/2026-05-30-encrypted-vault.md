# 加密 Vault (P1-3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在已存在的 `vault_items` 表上落地 age(X25519)加解密、`lumo vault` CLI 命令,并让 `${{ vault.* }}` 在环境变量未命中时回退到加密库解密取值。

**Architecture:** 方案 A——age 加解密 + `Vault` 门面内聚在 `lumo-storage`(新 `vault` 模块 + `Repo` 裸行方法);`lumo-core` 仅多持一个不透明的 `Arc<VaultIdentity>` 句柄,经 `FlowVm::with_vault` → `StepCtx` 注入,解析时调 `lumo_storage::vault::get_field`(core 不直接依赖 age);CLI 新增 `cmd::vault`,运行器(run/serve/hotkey/mcp)从身份文件加载句柄注入 VM。解析优先级:env 优先、加密 store 回退、都无则报错;身份缺失则优雅降级为纯 env。

**Tech Stack:** Rust(edition 2021, MSRV 1.83);`age = "0.11"`(纯 Rust X25519/ChaCha20Poly1305,无 C 依赖,利好交叉编译);`rpassword = "7"`(CLI 隐藏输入);rusqlite(`vault_items` 表已在 baseline DDL);serde_json;chrono。

**关联设计:** `docs/superpowers/specs/2026-05-30-encrypted-vault-design.md`(commit 9857d8a)。

---

## 已核实的 age 0.11 API(写代码以此为准)

```rust
// age::x25519
age::x25519::Identity::generate() -> Identity        // 随机身份
identity.to_string() -> age::secrecy::SecretString    // 固有方法(非 Display);用 ExposeSecret 取内层
identity.to_public() -> age::x25519::Recipient
impl FromStr for age::x25519::Identity { type Err = &'static str; }   // 解析 "AGE-SECRET-KEY-1…"
impl Display  for age::x25519::Recipient                              // 输出 "age1…"
impl FromStr  for age::x25519::Recipient { type Err = &'static str; }

// 一次性 API(默认特性即可用)
age::encrypt(recipient: &impl Recipient, plaintext: &[u8]) -> Result<Vec<u8>, age::EncryptError>
age::decrypt(identity:  &impl Identity,  ciphertext: &[u8]) -> Result<Vec<u8>, age::DecryptError>

// age 重导出 secrecy:取密钥串内层用 `use age::secrecy::ExposeSecret;` 后 `secret.expose_secret()`
```

`age::x25519::Identity` 同时实现 `age::Identity` 与 `Clone`;`age::x25519::Recipient` 实现 `age::Recipient`。因此 `age::encrypt(&recipient, ..)` / `age::decrypt(&identity.0, ..)` 可直接传具体类型引用。

> 注:docs.rs 上 0.11.0 标记 yanked。依赖写 `age = "0.11"`,Cargo 解析新建 lockfile 时会跳过 yanked,落到 0.11.x 最新可用版本(API 与上表一致)。若 `cargo build` 因 yank 报无可用版本,改用当时最新的具体 `0.11.x`。

## 与设计文档的有意偏差(YAGNI / 工程化)

- **metadata 仅存 `{"keys": [...]}`**:设计 §4 列了 `description`/`created_at`,但 CLI(§5)无 `--description` 入口、`updated_at` 列已覆盖时间,故二者去掉以免死字段。`list` 所需(name/keys/updated_at)全部可得。
- **`list` 为自由函数 `vault::list(repo)`**(非 `Vault::list(&self)`):list 永不解密,不需要身份;做成自由函数才能在身份缺失时仍可列举。
- **字段容器用 `BTreeMap<String,String>`**:键序确定 → metadata `keys` 与测试稳定。

## 文件结构

- **新建** `crates/lumo-storage/src/vault.rs` — age 句柄 `VaultIdentity`、`encrypt`/`decrypt`、`Vault<'a>` 门面、`list`、`get_field`。
- **改** `crates/lumo-storage/src/error.rs` — 加 `Crypto(String)` 变体。
- **改** `crates/lumo-storage/src/types.rs` — 加 `VaultRow`。
- **改** `crates/lumo-storage/src/repo.rs` — 加 `vault_put/get/list/delete`;import 加 `VaultRow`。
- **改** `crates/lumo-storage/src/lib.rs` — `pub mod vault;` + 重导出。
- **新建** `crates/lumo-storage/tests/vault.rs` — 跨 Task 1/2/3 追加的集成测试。
- **改** `crates/lumo-core/src/ctx.rs` — `StepCtx` 加 `vault_identity` 字段 + `with_vault`;`fork` 传递;3 个 `resolve_vault_*` 自由函数重构为 `VaultResolver` 并接 env→store→err。
- **改** `crates/lumo-core/src/vm.rs` — `FlowVm` 加 `vault_identity` + `with_vault`,`run` 注入 `StepCtx`。
- **新建** `crates/lumo-core/tests/vault_resolve.rs` — 解析优先级 + VM 端到端。
- **新建** `crates/lumo-cli/src/cmd/vault.rs` — `lumo vault` 子命令。
- **改** `crates/lumo-cli/src/cmd/mod.rs` — `pub mod vault;` + `vault_identity_path` + `load_vault_identity`。
- **改** `crates/lumo-cli/src/main.rs` — `Cmd::Vault` + dispatch。
- **改** 运行器 `run.rs`/`serve.rs`(3 处)/`hotkey.rs`/`mcp.rs` — 注入 `.with_vault(..)`。
- **改** `Cargo.toml`(workspace)、`crates/lumo-storage/Cargo.toml`、`crates/lumo-cli/Cargo.toml` — 依赖。
- **改** `docs/04-优化与补充开发-路线图.md` — P1-3 勾选。

---

### Task 1: age 加解密原语(`VaultIdentity` + `encrypt`/`decrypt`)

**Files:**
- Modify: `Cargo.toml`(workspace `[workspace.dependencies]`)
- Modify: `crates/lumo-storage/Cargo.toml`
- Modify: `crates/lumo-storage/src/error.rs`
- Create: `crates/lumo-storage/src/vault.rs`
- Modify: `crates/lumo-storage/src/lib.rs`
- Test: `crates/lumo-storage/tests/vault.rs`

- [ ] **Step 1: 加 workspace 依赖**

在根 `Cargo.toml` 的 `[workspace.dependencies]` 段(按字母序就近)加入:

```toml
age = "0.11"
rpassword = "7"
```

在 `crates/lumo-storage/Cargo.toml` 的 `[dependencies]` 段(`parking_lot.workspace = true` 之后)加入:

```toml
age.workspace = true
```

- [ ] **Step 2: 写失败测试 `crates/lumo-storage/tests/vault.rs`**

```rust
//! Integration tests for the encrypted vault (P1-3): age crypto primitives,
//! Repo CRUD, and the Vault façade.

use lumo_storage::vault;
use lumo_storage::{Repo, Vault, VaultIdentity};
use std::collections::BTreeMap;

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
```

- [ ] **Step 3: 运行测试,确认编译失败**

Run: `cargo test -p lumo-storage --test vault`
Expected: 编译错误 —— `unresolved import lumo_storage::vault` / `Vault` / `VaultIdentity` 尚不存在。

- [ ] **Step 4: 加 `Crypto` 错误变体** —— `crates/lumo-storage/src/error.rs`

在 `Json` 与 `NotFound` 之间加入:

```rust
    #[error("crypto: {0}")]
    Crypto(String),
```

- [ ] **Step 5: 创建 `crates/lumo-storage/src/vault.rs`(本任务部分)**

```rust
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
    /// parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let secret = self.0.to_string();
        std::fs::write(path, secret.expose_secret().as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
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
```

- [ ] **Step 6: 注册模块 + 重导出** —— `crates/lumo-storage/src/lib.rs`

在 `pub mod types;` 之后加:

```rust
pub mod vault;
```

在 `pub use types::*;` 之后加:

```rust
pub use vault::VaultIdentity;
```

- [ ] **Step 7: 运行测试,确认通过**

Run: `cargo test -p lumo-storage --test vault`
Expected: PASS —— `crypto_encrypt_decrypt_roundtrip` / `crypto_wrong_identity_cannot_decrypt` / `crypto_save_then_load_roundtrips_and_chmods_0600` 三个绿。

> 若 `age::secrecy::ExposeSecret` 路径在所选 0.11.x 上不存在(age 未重导出 secrecy),回退:在 `crates/lumo-storage/Cargo.toml` 直接加 `secrecy`(版本对齐 age 的依赖),改 `use secrecy::ExposeSecret;`。Step 7 的编译会暴露此情况。

- [ ] **Step 8: 提交**

```bash
git add Cargo.toml crates/lumo-storage/Cargo.toml \
        crates/lumo-storage/src/error.rs crates/lumo-storage/src/vault.rs \
        crates/lumo-storage/src/lib.rs crates/lumo-storage/tests/vault.rs
git commit -m "feat(P1-3): age X25519 身份 + 加解密原语(lumo-storage vault 模块)"
```

---

### Task 2: `VaultRow` + `Repo` 裸行 CRUD

**Files:**
- Modify: `crates/lumo-storage/src/types.rs`
- Modify: `crates/lumo-storage/src/repo.rs`
- Test: `crates/lumo-storage/tests/vault.rs`(追加)

- [ ] **Step 1: 追加失败测试** —— `crates/lumo-storage/tests/vault.rs` 末尾

```rust
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
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p lumo-storage --test vault repo_`
Expected: 编译错误 —— `no method named vault_put` on `Repo`,`VaultRow` 未定义。

- [ ] **Step 3: 加 `VaultRow`** —— `crates/lumo-storage/src/types.rs` 末尾

```rust
/// A row of the `vault_items` table (P1-3). `age_ciphertext` is opaque age
/// binary; `metadata` is non-sensitive JSON (field names only).
#[derive(Debug, Clone)]
pub struct VaultRow {
    pub name: String,
    pub age_ciphertext: Vec<u8>,
    pub metadata: String,
    pub updated_at: i64,
}
```

- [ ] **Step 4: import 加 `VaultRow`** —— `crates/lumo-storage/src/repo.rs`

把:

```rust
    types::{AiCallInsert, AiCallRow, ArtifactRow, FlowRunRow, StepRunRow},
```

改为:

```rust
    types::{AiCallInsert, AiCallRow, ArtifactRow, FlowRunRow, StepRunRow, VaultRow},
```

- [ ] **Step 5: 加 4 个 Repo 方法** —— `crates/lumo-storage/src/repo.rs`

在 `impl Repo { … }` 内(挑一个已有方法之后,如 `insert_artifact` 附近)加入:

```rust
    /// Insert or replace a vault item (P1-3).
    pub fn vault_put(
        &self,
        name: &str,
        age_ciphertext: &[u8],
        metadata: &str,
        updated_at: i64,
    ) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute(
            "INSERT INTO vault_items (name, age_ciphertext, metadata, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
               age_ciphertext = excluded.age_ciphertext,
               metadata       = excluded.metadata,
               updated_at     = excluded.updated_at",
            params![name, age_ciphertext, metadata, updated_at],
        )?;
        Ok(())
    }

    /// Fetch one vault item by name (`None` if absent).
    pub fn vault_get(&self, name: &str) -> Result<Option<VaultRow>, StorageError> {
        let c = self.inner.lock();
        let row = c
            .query_row(
                "SELECT name, age_ciphertext, metadata, updated_at
                 FROM vault_items WHERE name = ?1",
                params![name],
                |r| {
                    Ok(VaultRow {
                        name: r.get(0)?,
                        age_ciphertext: r.get(1)?,
                        metadata: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// List all vault items, ordered by name.
    pub fn vault_list(&self) -> Result<Vec<VaultRow>, StorageError> {
        let c = self.inner.lock();
        let mut stmt = c.prepare(
            "SELECT name, age_ciphertext, metadata, updated_at
             FROM vault_items ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(VaultRow {
                name: r.get(0)?,
                age_ciphertext: r.get(1)?,
                metadata: r.get(2)?,
                updated_at: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Delete one vault item by name (no-op if absent).
    pub fn vault_delete(&self, name: &str) -> Result<(), StorageError> {
        let c = self.inner.lock();
        c.execute("DELETE FROM vault_items WHERE name = ?1", params![name])?;
        Ok(())
    }
```

- [ ] **Step 6: 运行,确认通过**

Run: `cargo test -p lumo-storage --test vault repo_`
Expected: PASS —— 5 个 `repo_*` 测试绿。

- [ ] **Step 7: 提交**

```bash
git add crates/lumo-storage/src/types.rs crates/lumo-storage/src/repo.rs \
        crates/lumo-storage/tests/vault.rs
git commit -m "feat(P1-3): vault_items 裸行 CRUD(vault_put/get/list/delete + VaultRow)"
```

---

### Task 3: `Vault` 门面 + `list` + `get_field`

**Files:**
- Modify: `crates/lumo-storage/src/vault.rs`(追加)
- Modify: `crates/lumo-storage/src/lib.rs`
- Test: `crates/lumo-storage/tests/vault.rs`(追加)

- [ ] **Step 1: 追加失败测试** —— `crates/lumo-storage/tests/vault.rs` 末尾

```rust
fn one_field(key: &str, val: &str) -> BTreeMap<String, String> {
    let mut f = BTreeMap::new();
    f.insert(key.to_string(), val.to_string());
    f
}

#[test]
fn facade_put_get_roundtrip() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    let vault = Vault::new(&repo, &id);
    let mut fields = one_field("user", "alice");
    fields.insert("pass".to_string(), "s3cr3t".to_string());
    vault.put("smtp", &fields).unwrap();

    let got = vault.get("smtp").unwrap().expect("present");
    assert_eq!(got.get("user").map(String::as_str), Some("alice"));
    assert_eq!(got.get("pass").map(String::as_str), Some("s3cr3t"));
}

#[test]
fn facade_get_missing_is_none() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    assert!(Vault::new(&repo, &id).get("nope").unwrap().is_none());
}

#[test]
fn facade_metadata_has_no_plaintext_and_list_shows_keys() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    Vault::new(&repo, &id).put("smtp", &one_field("user", "alice")).unwrap();

    // The metadata column must not leak the secret value.
    let row = repo.vault_get("smtp").unwrap().unwrap();
    assert!(!row.metadata.contains("alice"), "metadata leaked plaintext");

    let listed = vault::list(&repo).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "smtp");
    assert_eq!(listed[0].keys, vec!["user".to_string()]);
}

#[test]
fn facade_get_field_hits_and_misses() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    Vault::new(&repo, &id).put("smtp", &one_field("user", "alice")).unwrap();

    assert_eq!(
        vault::get_field(&repo, &id, "smtp", "user").unwrap(),
        Some("alice".to_string())
    );
    assert_eq!(vault::get_field(&repo, &id, "smtp", "missing").unwrap(), None);
    assert_eq!(vault::get_field(&repo, &id, "noitem", "user").unwrap(), None);
}

#[test]
fn facade_delete_removes_item() {
    let repo = Repo::open_in_memory().unwrap();
    let id = VaultIdentity::generate();
    let vault = Vault::new(&repo, &id);
    vault.put("smtp", &one_field("user", "alice")).unwrap();
    vault.delete("smtp").unwrap();
    assert!(vault.get("smtp").unwrap().is_none());
}
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p lumo-storage --test vault facade_`
Expected: 编译错误 —— `Vault` / `Vault::new` / `vault::list` / `vault::get_field` 未定义。

- [ ] **Step 3: 追加门面到 `crates/lumo-storage/src/vault.rs`**

文件顶部 import 段加(在 `use age::secrecy::ExposeSecret;` 之后):

```rust
use crate::repo::Repo;
use chrono::Utc;
use std::collections::BTreeMap;
```

文件末尾追加:

```rust
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
    pub repo: &'a Repo,
    pub identity: &'a VaultIdentity,
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
        self.repo.vault_put(name, &ciphertext, &metadata, updated_at)
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
        let meta: serde_json::Value = serde_json::from_str(&r.metadata)?;
        let keys = meta
            .get("keys")
            .and_then(|k| k.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        out.push(VaultListed {
            name: r.name,
            keys,
            updated_at: r.updated_at,
        });
    }
    Ok(out)
}

/// Decrypt a single field for runtime resolution (called by `lumo-core`'s
/// `${{ vault.* }}` resolver). `Ok(None)` if the item or key is absent.
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
```

- [ ] **Step 4: 重导出门面** —— `crates/lumo-storage/src/lib.rs`

把 Task 1 加的:

```rust
pub use vault::VaultIdentity;
```

改为:

```rust
pub use vault::{Vault, VaultIdentity, VaultListed};
```

- [ ] **Step 5: 运行,确认通过(并跑全文件回归)**

Run: `cargo test -p lumo-storage --test vault`
Expected: PASS —— Task 1/2/3 全部测试绿(crypto_* + repo_* + facade_*)。

- [ ] **Step 6: 提交**

```bash
git add crates/lumo-storage/src/vault.rs crates/lumo-storage/src/lib.rs \
        crates/lumo-storage/tests/vault.rs
git commit -m "feat(P1-3): Vault 门面 + list + get_field(JSON 对象加密往返)"
```

---

### Task 4: `StepCtx` 持身份 + `${{ vault.* }}` 走 env→store→err

**Files:**
- Modify: `crates/lumo-core/src/ctx.rs`(struct 字段 + `with_vault` + `fork` + 解析器重构)
- Test: `crates/lumo-core/tests/vault_resolve.rs`

- [ ] **Step 1: 写失败测试** —— 新建 `crates/lumo-core/tests/vault_resolve.rs`

```rust
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
```

> 各测试使用互不相同的 vault 名 → 互不相同的 `LUMO_VAULT_*` 键,因此并行线程不会互相污染进程级环境变量。

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p lumo-core --test vault_resolve`
Expected: 编译错误 —— `StepCtx` 无 `with_vault` 方法。

- [ ] **Step 3: `StepCtx` 加字段** —— `crates/lumo-core/src/ctx.rs`

在 struct `StepCtx` 内 `flow_ai: Option<FlowAi>,`(约 line 78)之后加:

```rust
    /// P1-3: optional age identity for decrypting `${{ vault.* }}` from the
    /// encrypted store when an env var isn't set. `None` ⇒ env-only (graceful
    /// degrade when no identity file exists).
    vault_identity: Option<Arc<lumo_storage::VaultIdentity>>,
```

- [ ] **Step 4: `new` 初始化字段** —— `crates/lumo-core/src/ctx.rs`

在 `StepCtx::new` 的结构体字面量里 `flow_ai: None,` 之后加:

```rust
            vault_identity: None,
```

- [ ] **Step 5: 加 `with_vault` builder** —— `crates/lumo-core/src/ctx.rs`

在 `with_ai`(约 line 169-177)之后加:

```rust
    /// Attach the age identity used to decrypt `${{ vault.* }}` from the
    /// encrypted store (P1-3). Seeded by the VM from `FlowVm::with_vault`.
    /// `None` keeps resolution env-only.
    pub fn with_vault(mut self, identity: Option<Arc<lumo_storage::VaultIdentity>>) -> Self {
        self.vault_identity = identity;
        self
    }
```

- [ ] **Step 6: `fork` 传递字段** —— `crates/lumo-core/src/ctx.rs`

在 `fork` 的结构体字面量里 `flow_ai: self.flow_ai.clone(),`(约 line 323)之后加:

```rust
            vault_identity: self.vault_identity.clone(),
```

- [ ] **Step 7: 改 `resolve_vault_placeholders` 入口** —— `crates/lumo-core/src/ctx.rs`(约 line 466-468)

把:

```rust
    pub fn resolve_vault_placeholders(&self, value: &Value) -> Result<Value, StepError> {
        resolve_vault_value(value, &self.vault_names)
    }
```

改为:

```rust
    pub fn resolve_vault_placeholders(&self, value: &Value) -> Result<Value, StepError> {
        VaultResolver {
            names: &self.vault_names,
            repo: self.repo.as_ref(),
            identity: self.vault_identity.as_deref(),
        }
        .resolve_value(value)
    }
```

- [ ] **Step 8: 用 `VaultResolver` 替换 3 个自由函数** —— `crates/lumo-core/src/ctx.rs`(约 line 768-831)

删除 `resolve_vault_value` / `resolve_vault_string` / `resolve_vault_expr` 三个自由函数(它们仅被 `resolve_vault_placeholders` 调用,无其他 caller),替换为:

```rust
/// Resolves `${{ vault.NAME.KEY }}` placeholders left intact through template
/// rendering. Env vars win (back-compat / CI override); the encrypted store is
/// the fallback when both a repo and an identity are present (P1-3).
struct VaultResolver<'a> {
    names: &'a [String],
    repo: Option<&'a Repo>,
    identity: Option<&'a lumo_storage::VaultIdentity>,
}

impl VaultResolver<'_> {
    fn resolve_value(&self, value: &Value) -> Result<Value, StepError> {
        match value {
            Value::String(s) => Ok(Value::String(self.resolve_string(s)?)),
            Value::Array(items) => Ok(Value::Array(
                items
                    .iter()
                    .map(|v| self.resolve_value(v))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Value::Object(map) => {
                let mut out = Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k.clone(), self.resolve_value(v)?);
                }
                Ok(Value::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    fn resolve_string(&self, src: &str) -> Result<String, StepError> {
        let mut out = String::new();
        let mut rest = src;
        while let Some(start) = rest.find("${{ vault.") {
            out.push_str(&rest[..start]);
            let token_rest = &rest[start + 4..];
            let Some(end) = token_rest.find("}}") else {
                out.push_str(&rest[start..]);
                return Ok(out);
            };
            let expr = token_rest[..end].trim();
            out.push_str(&self.resolve_expr(expr)?);
            rest = &token_rest[end + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn resolve_expr(&self, expr: &str) -> Result<String, StepError> {
        let path = expr
            .strip_prefix("vault.")
            .ok_or_else(|| StepError::msg(format!("invalid vault placeholder `{expr}`")))?;
        let mut parts = path.split('.');
        let name = parts
            .next()
            .ok_or_else(|| StepError::msg(format!("invalid vault placeholder `{expr}`")))?;
        if !self.names.iter().any(|n| n == name) {
            return Err(StepError::msg(format!(
                "vault `{name}` is not declared in spec.vault"
            )));
        }
        let key = parts.collect::<Vec<_>>().join("_");

        // 1) Env wins: LUMO_VAULT_<NAME>[_<KEY>] (back-compat + CI override).
        let env_key = if key.is_empty() {
            format!("LUMO_VAULT_{}", sanitize_env(name))
        } else {
            format!("LUMO_VAULT_{}_{}", sanitize_env(name), sanitize_env(&key))
        };
        if let Ok(v) = std::env::var(&env_key) {
            return Ok(v);
        }

        // 2) Encrypted store fallback (only when both repo + identity present).
        if let (Some(repo), Some(identity)) = (self.repo, self.identity) {
            match lumo_storage::vault::get_field(repo, identity, name, &key) {
                Ok(Some(v)) => return Ok(v),
                Ok(None) => {}
                Err(e) => {
                    return Err(StepError::msg(format!(
                        "vault `{name}` could not be decrypted: {e}"
                    )))
                }
            }
        }

        // 3) Neither env nor store had it.
        Err(StepError::msg(format!(
            "vault value `{expr}` is missing; set {env_key} or run `lumo vault add {name}`"
        )))
    }
}
```

> `Repo` 已因 `repo: Option<Repo>` 字段在 `ctx.rs` 内导入;`lumo_storage::vault::get_field` 与 `lumo_storage::VaultIdentity` 用全限定路径(lumo-core 已依赖 lumo-storage),无需新 `use`。`Map`(serde_json)在本文件已使用。

- [ ] **Step 9: 运行,确认通过 + ctx 既有测试不回归**

Run: `cargo test -p lumo-core --test vault_resolve`
Expected: PASS —— 6 个测试全绿。

Run: `cargo test -p lumo-core`
Expected: PASS —— 既有 ctx/vm/control_flow 等测试不回归。

- [ ] **Step 10: 提交**

```bash
git add crates/lumo-core/src/ctx.rs crates/lumo-core/tests/vault_resolve.rs
git commit -m "feat(P1-3): StepCtx 持 vault 身份 + {{vault.*}} env 优先 store 回退"
```

---

### Task 5: `FlowVm::with_vault` 注入 + VM 端到端

**Files:**
- Modify: `crates/lumo-core/src/vm.rs`(struct 字段 + `new` + `with_vault` + `run` 注入)
- Test: `crates/lumo-core/tests/vault_resolve.rs`(追加端到端)

- [ ] **Step 1: 追加失败的端到端测试** —— `crates/lumo-core/tests/vault_resolve.rs` 末尾

```rust
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
```

- [ ] **Step 2: 运行,确认失败**

Run: `cargo test -p lumo-core --test vault_resolve vm_resolves_vault_from_store`
Expected: 编译错误 —— `FlowVm` 无 `with_vault` 方法。

- [ ] **Step 3: `FlowVm` 加字段** —— `crates/lumo-core/src/vm.rs`

在 struct `FlowVm` 内 `step_timeout: Option<Duration>,`(约 line 78)之后加:

```rust
    /// P1-3: optional age identity, threaded into each run's `StepCtx` so
    /// `${{ vault.* }}` can fall back to the encrypted store.
    vault_identity: Option<Arc<lumo_storage::VaultIdentity>>,
```

- [ ] **Step 4: `new` 初始化** —— `crates/lumo-core/src/vm.rs`

在 `FlowVm::new` 的字面量里 `step_timeout: None,`(约 line 90)之后加:

```rust
            vault_identity: None,
```

- [ ] **Step 5: 加 `with_vault` builder** —— `crates/lumo-core/src/vm.rs`

在 `with_step_timeout`(约 line 123-126)之后加:

```rust
    /// Attach the age identity for `${{ vault.* }}` store decryption (P1-3).
    /// `None` keeps resolution env-only.
    pub fn with_vault(mut self, identity: Option<Arc<lumo_storage::VaultIdentity>>) -> Self {
        self.vault_identity = identity;
        self
    }
```

- [ ] **Step 6: `run` 注入 `StepCtx`** —— `crates/lumo-core/src/vm.rs`(约 line 180)

把 `StepCtx::new(...)` 链尾的:

```rust
        .with_step_timeout(self.step_timeout);
```

改为:

```rust
        .with_step_timeout(self.step_timeout)
        .with_vault(self.vault_identity.clone());
```

> `Arc` 已在 vm.rs 导入(`ai_provider: Option<Arc<dyn AiHookProvider>>`)。`lumo_storage::VaultIdentity` 用全限定路径。

- [ ] **Step 7: 运行,确认通过**

Run: `cargo test -p lumo-core --test vault_resolve`
Expected: PASS —— 含新增 `vm_resolves_vault_from_store_at_action_exec`,共 7 个绿。

- [ ] **Step 8: 提交**

```bash
git add crates/lumo-core/src/vm.rs crates/lumo-core/tests/vault_resolve.rs
git commit -m "feat(P1-3): FlowVm::with_vault 注入身份 + VM 端到端 vault 解析测试"
```

---

### Task 6: CLI `lumo vault`(init/add/get/list/rm/path)

**Files:**
- Modify: `crates/lumo-cli/Cargo.toml`
- Modify: `crates/lumo-cli/src/cmd/mod.rs`
- Create: `crates/lumo-cli/src/cmd/vault.rs`
- Modify: `crates/lumo-cli/src/main.rs`
- Test: `crates/lumo-cli/src/cmd/vault.rs`(inline `#[cfg(test)]`,纯逻辑)

- [ ] **Step 1: 加 CLI 依赖** —— `crates/lumo-cli/Cargo.toml`

在 `[dependencies]` 段(`comfy-table.workspace = true` 附近)加:

```toml
rpassword.workspace = true
```

- [ ] **Step 2: 加路径/加载助手** —— `crates/lumo-cli/src/cmd/mod.rs`

模块声明区(与 `pub mod providers;` 同段,按字母序)加:

```rust
pub mod vault;
```

在 `skills_root` 函数之后加(仅路径解析;运行时加载助手 `load_vault_identity` 在 Task 7 引入,届时才有 caller,避免二进制 crate 的 `dead_code` 告警):

```rust
pub(crate) fn vault_identity_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_VAULT_IDENTITY")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("age-identity.txt"))
}
```

> `Path` / `PathBuf` 已在 `mod.rs` 导入。`vault_identity_path` 在 Task 6 即被 `vault.rs` 使用(无 dead_code)。

- [ ] **Step 3: 写失败测试(纯逻辑)** —— 在 `crates/lumo-cli/src/cmd/vault.rs` 先写测试模块占位

新建文件,先只放测试(impl 在 Step 5 补):

```rust
//! `lumo vault` subcommand — age-encrypted secret store (P1-3).

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn mask_hides_value_and_length() {
        assert_eq!(mask(), "********");
        assert_eq!(mask(), mask(), "mask is constant — never leaks length");
    }

    #[test]
    fn upsert_inserts_then_overwrites() {
        let mut f = BTreeMap::new();
        upsert_field(&mut f, "user", "a".to_string());
        assert_eq!(f.get("user").map(String::as_str), Some("a"));
        upsert_field(&mut f, "user", "b".to_string());
        assert_eq!(f.get("user").map(String::as_str), Some("b"));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn remove_reports_emptiness() {
        let mut f = BTreeMap::new();
        upsert_field(&mut f, "user", "a".to_string());
        upsert_field(&mut f, "pass", "b".to_string());
        assert!(!remove_field(&mut f, "user"), "pass still present");
        assert!(remove_field(&mut f, "pass"), "now empty");
    }
}
```

- [ ] **Step 4: 运行,确认失败**

Run: `cargo test -p lumo-cli vault::tests`
Expected: 编译错误 —— `mask` / `upsert_field` / `remove_field` 未定义(且 `Cmd::Vault` 尚未接,需先完成 Step 5/6 才能整体编译)。

- [ ] **Step 5: 实现 `crates/lumo-cli/src/cmd/vault.rs`(测试模块上方)**

在文件顶部(`#[cfg(test)] mod tests` 之前)写:

```rust
use clap::{Args as ClapArgs, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, Table};
use lumo_storage::{vault, Repo, Vault, VaultIdentity};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::vault_identity_path;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    sub: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// Generate the age identity file (your master private key)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Add or update a secret field. Value is read from a hidden prompt, or
    /// `--stdin`. NEVER pass the value on the command line.
    Add {
        name: String,
        #[arg(long)]
        key: Option<String>,
        /// Read the value from stdin instead of a hidden prompt
        #[arg(long)]
        stdin: bool,
    },
    /// Show a stored item — masked unless `--reveal`
    Get {
        name: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        reveal: bool,
    },
    /// List stored item names + field keys (never reveals values)
    List,
    /// Remove a whole item, or a single field with `--key`
    Rm {
        name: String,
        #[arg(long)]
        key: Option<String>,
    },
    /// Print the identity file path and DB path
    Path,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let id_path = vault_identity_path(&home);
    match args.sub {
        Sub::Path => {
            println!("identity: {}", id_path.display());
            println!("db:       {}", home.join("lumo.db").display());
            Ok(())
        }

        Sub::Init { force } => {
            if id_path.exists() && !force {
                anyhow::bail!(
                    "{} already exists. Use --force to overwrite (DESTROYS access to existing secrets).",
                    id_path.display()
                );
            }
            let identity = VaultIdentity::generate();
            identity.save(&id_path)?;
            println!("{} wrote identity to {}", "✓".green().bold(), id_path.display());
            println!("  public key: {}", identity.public_string());
            println!(
                "  {} add `{}` to .gitignore — it is your master key.",
                "!".yellow().bold(),
                id_path.display()
            );
            Ok(())
        }

        Sub::Add { name, key, stdin } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let value = read_secret_value(stdin)?;
            let vault = Vault::new(&repo, &identity);
            let mut fields = vault.get(&name)?.unwrap_or_default();
            upsert_field(&mut fields, &key.unwrap_or_default(), value);
            vault.put(&name, &fields)?;
            println!("{} stored secret `{}`", "✓".green().bold(), name);
            Ok(())
        }

        Sub::Get { name, key, reveal } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let fields = Vault::new(&repo, &identity)
                .get(&name)?
                .ok_or_else(|| anyhow::anyhow!("no vault item `{name}`"))?;
            match key {
                Some(k) => {
                    let v = fields
                        .get(&k)
                        .ok_or_else(|| anyhow::anyhow!("item `{name}` has no key `{k}`"))?;
                    println!("{}", if reveal { v.clone() } else { mask() });
                }
                None => {
                    for (k, v) in &fields {
                        let shown = if reveal { v.clone() } else { mask() };
                        let label = if k.is_empty() { "(scalar)" } else { k.as_str() };
                        println!("{label} = {shown}");
                    }
                }
            }
            Ok(())
        }

        Sub::List => {
            let repo = open_repo(&home)?;
            let items = vault::list(&repo)?;
            if items.is_empty() {
                println!("(vault is empty — add one with `lumo vault add <name> --key <k>`)");
                return Ok(());
            }
            let mut t = Table::new();
            t.load_preset(UTF8_FULL)
                .set_header(vec!["name", "keys", "updated_at"]);
            for it in items {
                t.add_row(vec![it.name, it.keys.join(", "), it.updated_at.to_string()]);
            }
            println!("{t}");
            Ok(())
        }

        Sub::Rm { name, key } => {
            let identity = load_identity(&id_path)?;
            let repo = open_repo(&home)?;
            let vault = Vault::new(&repo, &identity);
            match key {
                None => {
                    vault.delete(&name)?;
                    println!("{} removed `{}`", "✓".green().bold(), name);
                }
                Some(k) => {
                    let mut fields = vault
                        .get(&name)?
                        .ok_or_else(|| anyhow::anyhow!("no vault item `{name}`"))?;
                    let now_empty = remove_field(&mut fields, &k);
                    if now_empty {
                        vault.delete(&name)?;
                    } else {
                        vault.put(&name, &fields)?;
                    }
                    println!("{} removed `{}.{}`", "✓".green().bold(), name, k);
                }
            }
            Ok(())
        }
    }
}

fn open_repo(home: &Path) -> anyhow::Result<Repo> {
    std::fs::create_dir_all(home)?;
    Ok(Repo::open(home.join("lumo.db"))?)
}

fn load_identity(id_path: &Path) -> anyhow::Result<VaultIdentity> {
    if !id_path.exists() {
        anyhow::bail!(
            "vault identity not found at {}; run `lumo vault init` first",
            id_path.display()
        );
    }
    Ok(VaultIdentity::load(id_path)?)
}

fn read_secret_value(stdin: bool) -> anyhow::Result<String> {
    if stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(buf.trim_end_matches(['\n', '\r']).to_string())
    } else {
        Ok(rpassword::prompt_password("secret value: ")?)
    }
}

/// Fixed-width mask — never leaks the value or its length.
fn mask() -> String {
    "********".to_string()
}

/// Insert or overwrite a single field. Pure (testable) core of `add`.
fn upsert_field(fields: &mut BTreeMap<String, String>, key: &str, value: String) {
    fields.insert(key.to_string(), value);
}

/// Remove a field; returns true when the item is now empty (caller deletes it).
fn remove_field(fields: &mut BTreeMap<String, String>, key: &str) -> bool {
    fields.remove(key);
    fields.is_empty()
}
```

- [ ] **Step 6: 接进 CLI** —— `crates/lumo-cli/src/main.rs`

`Cmd` 枚举里 `Providers(...)` 附近加:

```rust
    /// Manage the encrypted secret vault (age)
    Vault(cmd::vault::Args),
```

`match cli.cmd` 里 `Cmd::Providers(a) => ...` 附近加:

```rust
        Cmd::Vault(a) => cmd::vault::run(home, a).await,
```

- [ ] **Step 7: 运行单测,确认通过**

Run: `cargo test -p lumo-cli vault::tests`
Expected: PASS —— `mask_hides_value_and_length` / `upsert_inserts_then_overwrites` / `remove_reports_emptiness` 三绿。

- [ ] **Step 8: 提交**

```bash
git add crates/lumo-cli/Cargo.toml crates/lumo-cli/src/cmd/mod.rs \
        crates/lumo-cli/src/cmd/vault.rs crates/lumo-cli/src/main.rs
git commit -m "feat(P1-3): lumo vault CLI(init/add/get/list/rm/path,密文走 prompt/stdin)"
```

---

### Task 7: 运行器注入身份 + 路线图勾选 + 全量校验

**Files:**
- Modify: `crates/lumo-cli/src/cmd/run.rs`
- Modify: `crates/lumo-cli/src/cmd/serve.rs`(3 处)
- Modify: `crates/lumo-cli/src/cmd/hotkey.rs`
- Modify: `crates/lumo-cli/src/cmd/mcp.rs`
- Modify: `docs/04-优化与补充开发-路线图.md`

> 注入规则:每个站点把 `super::attach_ai_hooks(FlowVm::new(registry, repo), <HOME>, &flow)` 返回的 `FlowVm` 链上 `.with_vault(super::load_vault_identity(<HOME>))`,`<HOME>` 用该站点传给 `attach_ai_hooks` 的同一个 home 引用。子流程运行器 `skills.rs`(repo=None)保持纯 env,不注入。

- [ ] **Step 1: 加运行时加载助手 + `run.rs` 注入**

先在 `crates/lumo-cli/src/cmd/mod.rs` 的 `vault_identity_path` 之后加(本步引入首个 caller,故无 dead_code):

```rust
/// Load the vault identity for a run if one exists. Missing file ⇒ `None`
/// (env-only resolution); a present-but-corrupt file is warned about and also
/// degrades to `None` so a run never hard-fails on vault wiring alone (P1-3).
pub(crate) fn load_vault_identity(home: &Path) -> Option<Arc<lumo_storage::VaultIdentity>> {
    let path = vault_identity_path(home);
    if !path.exists() {
        return None;
    }
    match lumo_storage::VaultIdentity::load(&path) {
        Ok(id) => Some(Arc::new(id)),
        Err(e) => {
            tracing::warn!("vault identity at {} unreadable: {e}", path.display());
            None
        }
    }
}
```

> `Arc` 已在 `mod.rs` 导入;`lumo_storage` 是 lumo-cli 既有依赖,用全限定路径。

接着改 `crates/lumo-cli/src/cmd/run.rs`(约 line 79-80),把:

```rust
    let vm =
        super::attach_ai_hooks(FlowVm::new(registry, repo), &home, &flow).with_cancel(cancel);
```

改为:

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &home, &flow)
        .with_vault(super::load_vault_identity(&home))
        .with_cancel(cancel);
```

- [ ] **Step 2: `serve.rs` 注入(3 处)** —— `crates/lumo-cli/src/cmd/serve.rs`

line ~207(home 表达式为 `&state.home`):

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &state.home, &flow)
        .with_vault(super::load_vault_identity(&state.home));
```

line ~381 与 line ~581(home 表达式为 `home`)各自把:

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), home, &flow);
```

改为:

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), home, &flow)
        .with_vault(super::load_vault_identity(home));
```

- [ ] **Step 3: `hotkey.rs` 注入** —— `crates/lumo-cli/src/cmd/hotkey.rs`(约 line 257,home 表达式为 `home`)

把:

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), home, &flow);
```

改为:

```rust
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), home, &flow)
        .with_vault(super::load_vault_identity(home));
```

- [ ] **Step 4: `mcp.rs` 注入** —— `crates/lumo-cli/src/cmd/mcp.rs`(约 line 347,home 表达式为 `&self.home`)

把:

```rust
        let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &self.home, &flow);
```

改为:

```rust
        let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &self.home, &flow)
            .with_vault(super::load_vault_identity(&self.home));
```

- [ ] **Step 5: 全量校验(fmt + clippy + test)**

Run:
```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LUMO_SKIP_BROWSER_TESTS=1 cargo test --workspace --exclude lumorpa-desktop
```
Expected: clippy 零告警;全工作区测试通过(含新增 `lumo-storage` vault 集成测试、`lumo-core` vault_resolve 7 例、`lumo-cli` vault 单测 3 例)。

- [ ] **Step 6: CLI 烟测(真实身份文件 + stdin 往返)**

Run:
```bash
HOME_TMP="$(mktemp -d)"
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault init
printf 'alice@example.com' | cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault add smtp --key user --stdin
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault list
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault get smtp --key user            # 期望掩码 ********
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault get smtp --key user --reveal    # 期望明文 alice@example.com
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault rm smtp --key user
cargo run -p lumo-cli --locked -- --home "$HOME_TMP" vault list                            # 期望空
ls -l "$HOME_TMP/age-identity.txt"                                                          # 期望 -rw------- (0600)
```
Expected: init 写出 0600 身份并打印公钥;add 经 stdin 落库;list 显示 `smtp` / `user`;get 默认掩码、`--reveal` 出明文;rm 删字段后 list 为空。

- [ ] **Step 7: 路线图勾选** —— `docs/04-优化与补充开发-路线图.md`

把第 36 行:

```markdown
- [ ] **P1-3 加密 vault 仅有表无实现**(已核实:`vault_items`/`age_ciphertext` 仅在 `schema.rs:70/72`)— 实现 age 加解密 repo 方法 + `lumo vault` 命令 + `{{vault.*}}` 走 vault 解析。
```

改为:

```markdown
- [x] **P1-3 加密 vault 仅有表无实现** ✅ 已修复
  - 新增 `lumo-storage::vault` 模块:`VaultIdentity`(age X25519 身份,generate/load/save-0600/recipient)+ `encrypt`/`decrypt`(age 一次性 API)+ `Vault<'a>` 门面(put/get/delete,JSON 对象整体加密)+ `list`(仅元数据,永不解密)+ `get_field`(运行时单字段解密)。`Repo` 加 `vault_put/get/list/delete` 裸行方法(`vault_items` 表已在 baseline DDL)。
  - `lumo-core`:`StepCtx`/`FlowVm` 经 `with_vault` 携带不透明 `Arc<VaultIdentity>`(core 不直依赖 age);`${{ vault.* }}` 解析重构为 `VaultResolver`——**env 优先**(`LUMO_VAULT_*`,向后兼容/CI 覆盖)、**加密 store 回退**(repo+identity 均在时调 `get_field`)、都无则报错;身份缺失则优雅降级为纯 env。
  - CLI 新增 `lumo vault`(init/add/get/list/rm/path):密文仅经隐藏 prompt(rpassword)或 `--stdin`,**绝不**上 argv;`get` 默认掩码、`--reveal` 出明文;`list` 永不出明文。run/serve/hotkey/mcp 运行器从 `$LUMO_HOME/age-identity.txt`(可经 `LUMO_VAULT_IDENTITY` 覆盖)加载身份注入 VM。
  - 依赖:`age = "0.11"`(纯 Rust,无 C,利好交叉编译)+ `rpassword = "7"`。测试:lumo-storage 加解密/CRUD/门面往返 + lumo-core 解析优先级 6 例 + VM 端到端 + lumo-cli 纯逻辑单测。
```

- [ ] **Step 8: 提交**

```bash
git add crates/lumo-cli/src/cmd/mod.rs crates/lumo-cli/src/cmd/run.rs \
        crates/lumo-cli/src/cmd/serve.rs crates/lumo-cli/src/cmd/hotkey.rs \
        crates/lumo-cli/src/cmd/mcp.rs docs/04-优化与补充开发-路线图.md
git commit -m "feat(P1-3): run/serve/hotkey/mcp 注入 vault 身份 + 路线图勾选 P1-3"
```

---

## 完成定义(Definition of Done)

- `lumo-storage`:age 加解密 + `Repo` CRUD + `Vault` 门面 + `list`/`get_field` 全绿;错误身份解密必败;metadata 不含密文;身份文件 0600。
- `lumo-core`:`${{ vault.* }}` env 优先、store 回退、都无报错、未声明报错、身份缺失降级;VM 端到端从 store 解出秘密。
- `lumo-cli`:`lumo vault init/add/get/list/rm/path` 可用;密文不上 argv;`get` 默认掩码;`list` 不出明文;身份文件 0600 + `.gitignore` 提示。
- 运行器 run/serve/hotkey/mcp 注入身份;`cargo clippy --workspace --all-targets -- -D warnings` 零告警;全工作区测试通过。
- 路线图 P1-3 勾选并附实现摘要。
