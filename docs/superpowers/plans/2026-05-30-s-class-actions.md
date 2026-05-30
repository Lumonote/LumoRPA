# S 类动作批次 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 给 `lumo-actions` 增加 6 个标准库/实用工具动作(`archive.zip`/`archive.unzip`、`http.download`/`http.upload`、`notify.send`、`clipboard.get`/`clipboard.set`、`browser.wait`),并给 `http.request` 补响应大小上限,全部复用既有能力沙箱。

**Architecture:** 3 个新模块(`archive.rs`/`notify.rs`/`clipboard.rs`),扩展既有 `http.rs`/`browser.rs`;同步阻塞库(`zip`/`arboard`)一律走 `tokio::task::spawn_blocking`,async 段持 `ctx` 做能力校验;所有 fs/网络访问经 `ensure_fs_read`/`ensure_fs_write`/`ensure_network_url`;TDD,每动作一提交。

**Tech Stack:** Rust 1.83 / tokio / reqwest(`stream`+`multipart` 已启用)/ 新增 `zip 2.2`、`hmac 0.12`、`arboard 3`;测试 `wiremock 0.6` + `tempfile`。对应设计文档 `docs/superpowers/specs/2026-05-30-s-class-actions-design.md`。

---

## 文件结构

| 文件 | 动作 | 职责 |
|---|---|---|
| `crates/lumo-actions/src/archive.rs`(新建) | `archive.zip` / `archive.unzip` | ZIP 打包/解压;zip-slip + zip-bomb 防护;`spawn_blocking` 压解 |
| `crates/lumo-actions/src/http.rs`(扩展) | `http.download` / `http.upload` + `http.request` 增强 | 流式下载落盘;multipart/body 上传;响应大小上限 |
| `crates/lumo-actions/src/notify.rs`(新建) | `notify.send` | 4 provider 通知 + 钉钉/飞书 HMAC 加签 |
| `crates/lumo-actions/src/clipboard.rs`(新建) | `clipboard.get` / `clipboard.set` | 文本剪贴板;`spawn_blocking` 包 `arboard` |
| `crates/lumo-actions/src/browser.rs`(扩展) | `browser.wait` | present/visible/clickable/hidden/text 轮询等待 |
| `crates/lumo-actions/src/lib.rs`(扩展) | — | `register_all` 新增 `archive`/`notify`/`clipboard` 三处声明与注册 |
| `crates/lumo-actions/Cargo.toml`(扩展) | — | 增 `zip`/`hmac`/`arboard` 依赖,`zip` 同时入 dev-deps |
| `crates/lumo-actions/tests/archive_ops.rs`(新建) | — | archive 往返/zip-slip/越权/超限/空输入 |
| `crates/lumo-actions/tests/http_download_upload.rs`(新建) | — | download/upload + request max_bytes(wiremock+tempfile) |
| `crates/lumo-actions/tests/notify_ops.rs`(新建) | — | 4 provider body 形状 + 加签字段 + errcode 失败 + 越权 |
| `crates/lumo-actions/tests/clipboard_ops.rs`(新建) | — | 输入校验(CI)+ 真实往返(`#[ignore]`) |
| `crates/lumo-actions/tests/browser_wait.rs`(新建) | — | 输入校验(CI)+ 真实行为(`#[ignore]`) |
| `docs/04-优化与补充开发-路线图.md`(扩展) | — | F-5/F-7/F-8/F-9/F-11 勾选 |

## 通用约定(摘自既有代码,实现时照搬)

- **动作范式**:单元结构体 `impl lumo_core::Action`;`id()`/`summary()`/`schema()`(返回 `&'static Value`,`once_cell::sync::Lazy` + `additionalProperties:false`)/`async execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError>`。输入经 `serde_json::from_value` 进 `#[derive(Deserialize)]` 结构,失败 → `StepError::msg(format!("<id> input invalid: {e}"))`。
- **结果**:`ActionResult::from(serde_json::json!({...}))` 或 `ActionResult::null()`(`pub output: Value`)。
- **能力**:`ctx.ensure_fs_read(&Path)?` / `ctx.ensure_fs_write(&Path)?` / `ctx.ensure_network_url(&str)?`;越权返回 `StepError::CapabilityDenied`,其 `Display` 含 `capability denied`。
- **测试夹具**(`tests/common/mod.rs`,各测试文件 `mod common;`):`run(id, input)`(无能力)、`run_with(id, input, caps)`、`ok(id, input)`、`ok_with(id, input, caps)`、`fs_caps(dir)`(授 `{dir}/**` 读写)。网络用本地 `net()` 助手:`Capabilities { network: vec![host.into()], ..Default::default() }`。

---

## Task 1: `archive.zip` — 新建 `archive.rs` 打包

**Files:**
- Modify: `crates/lumo-actions/Cargo.toml`(加 `zip` 到 `[dependencies]` 与 `[dev-dependencies]`)
- Create: `crates/lumo-actions/src/archive.rs`
- Modify: `crates/lumo-actions/src/lib.rs`(声明 + 注册)
- Test: `crates/lumo-actions/tests/archive_ops.rs`

- [ ] **Step 1: 加依赖**

在 `crates/lumo-actions/Cargo.toml` 的 `[dependencies]` 末尾(`sha1 = "0.10"` 之后)加:

```toml
zip = { version = "2.2", default-features = false, features = ["deflate"] }
```

在 `[dev-dependencies]` 末尾(`wiremock = "0.6"` 之后)加(集成测试要直接用 `zip` 造 zip-slip 归档):

```toml
zip = { version = "2.2", default-features = false, features = ["deflate"] }
```

- [ ] **Step 2: 声明并注册模块**

`crates/lumo-actions/src/lib.rs`:在 `pub mod browser;` 之后按字母序插入 `pub mod archive;`,并在 `register_all` 内 `file::register(registry);` 之后加 `archive::register(registry);`。

```rust
pub mod archive;
```

```rust
    archive::register(registry);
```

- [ ] **Step 3: 写失败测试(往返 + 空输入)**

创建 `crates/lumo-actions/tests/archive_ops.rs`:

```rust
//! Integration coverage for `archive.zip` / `archive.unzip` (S-class F-7).
//! Hermetic: everything runs under a tempdir granted via `fs_caps`.

mod common;
use common::{fs_caps, ok_with, run, run_with};
use serde_json::json;

#[tokio::test]
async fn zip_then_unzip_round_trips_content() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("note.txt");
    std::fs::write(&src, "hello-zip").unwrap();
    let archive = dir.path().join("out.zip");
    let caps = fs_caps(dir.path());

    let zres = ok_with(
        "archive.zip",
        json!({"paths": [src], "dest": archive}),
        caps.clone(),
    )
    .await;
    assert_eq!(zres["entries"], json!(1));

    let out = dir.path().join("unpacked");
    let ures = ok_with(
        "archive.unzip",
        json!({"src": archive, "dest": out}),
        caps,
    )
    .await;
    assert_eq!(ures["entries"], json!(1));
    assert_eq!(
        std::fs::read_to_string(out.join("note.txt")).unwrap(),
        "hello-zip"
    );
}

#[tokio::test]
async fn zip_requires_at_least_one_path() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("empty.zip");
    let err = run_with(
        "archive.zip",
        json!({"paths": [], "dest": archive}),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("at least one path"), "got: {err}");
}

#[tokio::test]
async fn zip_outside_sandbox_is_denied() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("a.txt");
    std::fs::write(&src, "x").unwrap();
    // No caps at all → read of the source is denied.
    let err = run(
        "archive.zip",
        json!({"paths": [src], "dest": dir.path().join("a.zip")}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
```

- [ ] **Step 4: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test archive_ops`
Expected: 编译失败 / `action 'archive.zip' is not registered`(模块尚未实现)。

- [ ] **Step 5: 实现 `archive.rs`(zip 部分)**

创建 `crates/lumo-actions/src/archive.rs`:

```rust
//! Archive actions — ZIP only (`archive.zip` / `archive.unzip`).
//!
//! ZIP is the single supported format (design 2026-05-30); tar.gz and friends
//! are deliberately out of scope. The `zip` crate is synchronous, so the actual
//! compress/extract runs inside `spawn_blocking`; capability checks and path
//! enumeration stay on the async side where `ctx` lives.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub fn register(r: &mut ActionRegistry) {
    r.register(ZipAction);
    r.register(UnzipAction);
}

/// zip-bomb backstop: refuse to extract more than this many uncompressed bytes
/// unless the caller raises `max_total_bytes`.
const DEFAULT_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB

// ─── archive.zip ──────────────────────────────────────────────────────────────

pub struct ZipAction;

#[derive(Deserialize)]
struct ZipIn {
    paths: Vec<String>,
    dest: String,
    #[serde(default)]
    base_dir: Option<String>,
}

/// Entry name for a top-level input path. With `base_dir`, the name is the path
/// relative to it; without, the path is flattened to its final component.
fn root_entry_name(p: &Path, base_dir: Option<&Path>) -> Result<String, StepError> {
    if let Some(base) = base_dir {
        let rel = p.strip_prefix(base).map_err(|_| {
            StepError::msg(format!(
                "archive.zip: `{}` is not under base_dir `{}`",
                p.display(),
                base.display()
            ))
        })?;
        Ok(rel.to_string_lossy().replace('\\', "/"))
    } else {
        let name = p.file_name().ok_or_else(|| {
            StepError::msg(format!("archive.zip: path `{}` has no file name", p.display()))
        })?;
        Ok(name.to_string_lossy().to_string())
    }
}

/// Walk `src` (file or dir) collecting `(absolute file path, archive entry name)`.
/// Directories recurse; only files become entries (empty dirs are not archived).
fn collect_entries(
    src: &Path,
    entry_prefix: &str,
    out: &mut Vec<(PathBuf, String)>,
) -> Result<(), StepError> {
    let meta = std::fs::metadata(src)
        .map_err(|e| StepError::msg(format!("archive.zip stat {}: {e}", src.display())))?;
    if meta.is_dir() {
        let rd = std::fs::read_dir(src)
            .map_err(|e| StepError::msg(format!("archive.zip read_dir {}: {e}", src.display())))?;
        for ent in rd {
            let ent = ent.map_err(|e| StepError::msg(format!("archive.zip dir entry: {e}")))?;
            let name = ent.file_name().to_string_lossy().to_string();
            let child_prefix = if entry_prefix.is_empty() {
                name
            } else {
                format!("{entry_prefix}/{name}")
            };
            collect_entries(&ent.path(), &child_prefix, out)?;
        }
    } else {
        out.push((src.to_path_buf(), entry_prefix.to_string()));
    }
    Ok(())
}

/// Synchronous compress step (runs in `spawn_blocking`). Returns `(entries, bytes)`.
fn write_zip(dest: &Path, entries: Vec<(PathBuf, String)>) -> Result<(u64, u64), StepError> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StepError::msg(format!("archive.zip mkdir {}: {e}", parent.display()))
            })?;
        }
    }
    let file = std::fs::File::create(dest)
        .map_err(|e| StepError::msg(format!("archive.zip create {}: {e}", dest.display())))?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut total_bytes: u64 = 0;
    let mut count: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for (abs, name) in entries {
        zw.start_file(name.clone(), opts)
            .map_err(|e| StepError::msg(format!("archive.zip start `{name}`: {e}")))?;
        let mut f = std::fs::File::open(&abs)
            .map_err(|e| StepError::msg(format!("archive.zip open {}: {e}", abs.display())))?;
        loop {
            let n = f
                .read(&mut buf)
                .map_err(|e| StepError::msg(format!("archive.zip read {}: {e}", abs.display())))?;
            if n == 0 {
                break;
            }
            zw.write_all(&buf[..n])
                .map_err(|e| StepError::msg(format!("archive.zip write `{name}`: {e}")))?;
            total_bytes += n as u64;
        }
        count += 1;
    }
    zw.finish()
        .map_err(|e| StepError::msg(format!("archive.zip finish: {e}")))?;
    Ok((count, total_bytes))
}

#[async_trait]
impl Action for ZipAction {
    fn id(&self) -> &'static str {
        "archive.zip"
    }
    fn summary(&self) -> &'static str {
        "Compress files/directories into a ZIP archive"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["paths", "dest"],
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "dest": { "type": "string" },
                    "base_dir": { "type": "string" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ZipIn {
            paths,
            dest,
            base_dir,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("archive.zip input invalid: {e}")))?;
        if paths.is_empty() {
            return Err(StepError::msg("archive.zip requires at least one path"));
        }
        let base = base_dir.as_ref().map(PathBuf::from);
        let mut entries: Vec<(PathBuf, String)> = Vec::new();
        for p in &paths {
            let pp = PathBuf::from(p);
            let root_name = root_entry_name(&pp, base.as_deref())?;
            collect_entries(&pp, &root_name, &mut entries)?;
        }
        for (abs, _) in &entries {
            ctx.ensure_fs_read(abs)?;
        }
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_write(&dest_path)?;

        let (count, bytes) = tokio::task::spawn_blocking(move || write_zip(&dest_path, entries))
            .await
            .map_err(|e| StepError::msg(format!("archive.zip task: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "dest": dest,
            "entries": count,
            "bytes": bytes,
        })))
    }
}

// ─── archive.unzip lands in Task 2 (same file) ─────────────────────────────────

pub struct UnzipAction;

#[async_trait]
impl Action for UnzipAction {
    fn id(&self) -> &'static str {
        "archive.unzip"
    }
    fn summary(&self) -> &'static str {
        "Extract a ZIP archive into a directory"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["src", "dest"],
                "properties": {
                    "src": { "type": "string" },
                    "dest": { "type": "string" },
                    "max_total_bytes": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        // Implemented in Task 2.
        Err(StepError::msg("archive.unzip not yet implemented"))
    }
}

#[allow(dead_code)]
fn _silence_unused(_: Component) {}
```

> 注:`UnzipAction` 先放一个占位 `execute`(返回错误)以便 Task 1 编译通过且 `archive.zip` 测试可独立验证;Task 2 用真实实现替换。`_silence_unused`/`Component` 的 import 供 Task 2 的 `safe_join` 使用,Task 2 会删除该桩函数。

- [ ] **Step 6: 跑测试,确认 zip 相关用例通过**

Run: `cargo test -p lumo-actions --test archive_ops zip_`
Expected: `zip_then_unzip_round_trips_content` 失败(unzip 仍是桩),但 `zip_requires_at_least_one_path` 与 `zip_outside_sandbox_is_denied` 通过。先只验证 zip 行为:
Run: `cargo test -p lumo-actions --test archive_ops zip_requires_at_least_one_path zip_outside_sandbox_is_denied`
Expected: PASS(2 passed)。

- [ ] **Step 7: clippy + fmt**

Run: `cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions`
Expected: 无警告;格式干净。

- [ ] **Step 8: 提交**

```bash
git add crates/lumo-actions/Cargo.toml crates/lumo-actions/src/archive.rs crates/lumo-actions/src/lib.rs crates/lumo-actions/tests/archive_ops.rs Cargo.lock
git commit -m "feat(F-7): archive.zip 打包动作(+zip 依赖,spawn_blocking 压缩)"
```

---

## Task 2: `archive.unzip` — 解压 + zip-slip/zip-bomb 防护

**Files:**
- Modify: `crates/lumo-actions/src/archive.rs`(替换 `UnzipAction` 桩 + 加 `safe_join`/`extract_zip`,删 `_silence_unused`)
- Test: `crates/lumo-actions/tests/archive_ops.rs`(加 zip-slip + 超限用例)

- [ ] **Step 1: 加失败测试(zip-slip + zip-bomb)**

在 `crates/lumo-actions/tests/archive_ops.rs` 末尾追加:

```rust
#[tokio::test]
async fn unzip_rejects_zip_slip_entries() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("evil.zip");
    // Hand-craft a zip whose entry name escapes the destination.
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        zw.start_file("../escaped.txt", opts).unwrap();
        use std::io::Write;
        zw.write_all(b"pwned").unwrap();
        zw.finish().unwrap();
    }
    let out = dir.path().join("out");
    let err = run_with(
        "archive.unzip",
        json!({"src": archive, "dest": out}),
        fs_caps(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("zip-slip"), "got: {err}");
    assert!(
        !dir.path().join("escaped.txt").exists(),
        "no file may escape the sandbox"
    );
}

#[tokio::test]
async fn unzip_rejects_when_total_exceeds_limit() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("big.txt");
    std::fs::write(&src, "hello world").unwrap(); // 11 bytes
    let archive = dir.path().join("big.zip");
    let caps = fs_caps(dir.path());
    ok_with(
        "archive.zip",
        json!({"paths": [src], "dest": archive}),
        caps.clone(),
    )
    .await;

    let out = dir.path().join("unpacked");
    let err = run_with(
        "archive.unzip",
        json!({"src": archive, "dest": out, "max_total_bytes": 5}),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("exceeds limit"), "got: {err}");
}
```

- [ ] **Step 2: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test archive_ops unzip_`
Expected: FAIL(`archive.unzip not yet implemented`)。

- [ ] **Step 3: 实现 unzip**

在 `crates/lumo-actions/src/archive.rs` 中:删除 `_silence_unused` 桩函数;在 `UnzipAction` 之前加 `safe_join` + `extract_zip`;用下面的实现替换 `UnzipAction::execute`。

加在 `// ─── archive.unzip lands in Task 2` 注释下方、`pub struct UnzipAction;` 之前:

```rust
#[derive(Deserialize)]
struct UnzipIn {
    src: String,
    dest: String,
    #[serde(default)]
    max_total_bytes: Option<u64>,
}

/// Resolve a zip entry name to a path under `dest`, rejecting any entry that
/// escapes via `..` or absolute components (zip-slip). Mirrors the lexical-clean
/// approach the capability sandbox uses (P0-2): fold `.`/`..` lexically and
/// verify the result stays prefixed by `dest`.
fn safe_join(dest: &Path, entry_name: &str) -> Result<PathBuf, StepError> {
    let mut out = dest.to_path_buf();
    for comp in Path::new(entry_name).components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() || !out.starts_with(dest) {
                    return Err(StepError::msg(format!(
                        "archive.unzip: entry `{entry_name}` escapes destination (zip-slip)"
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(StepError::msg(format!(
                    "archive.unzip: entry `{entry_name}` is absolute (zip-slip)"
                )));
            }
        }
    }
    if !out.starts_with(dest) {
        return Err(StepError::msg(format!(
            "archive.unzip: entry `{entry_name}` escapes destination (zip-slip)"
        )));
    }
    Ok(out)
}

/// Synchronous extract step (runs in `spawn_blocking`). Returns the file count.
/// Aborts (and removes the partial file) once uncompressed bytes exceed `max_total`.
fn extract_zip(src: &Path, dest: &Path, max_total: u64) -> Result<u64, StepError> {
    let file = std::fs::File::open(src)
        .map_err(|e| StepError::msg(format!("archive.unzip open {}: {e}", src.display())))?;
    let mut zr = zip::ZipArchive::new(file)
        .map_err(|e| StepError::msg(format!("archive.unzip read {}: {e}", src.display())))?;
    let mut total: u64 = 0;
    let mut count: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for i in 0..zr.len() {
        let mut entry = zr
            .by_index(i)
            .map_err(|e| StepError::msg(format!("archive.unzip entry {i}: {e}")))?;
        let raw = entry.name().to_string();
        let target = safe_join(dest, &raw)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| {
                StepError::msg(format!("archive.unzip mkdir {}: {e}", target.display()))
            })?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StepError::msg(format!("archive.unzip mkdir {}: {e}", parent.display()))
            })?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|e| StepError::msg(format!("archive.unzip create {}: {e}", target.display())))?;
        loop {
            let n = entry
                .read(&mut buf)
                .map_err(|e| StepError::msg(format!("archive.unzip read entry: {e}")))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > max_total {
                drop(out);
                let _ = std::fs::remove_file(&target);
                return Err(StepError::msg(format!(
                    "archive.unzip: uncompressed size exceeds limit ({max_total} bytes)"
                )));
            }
            out.write_all(&buf[..n])
                .map_err(|e| StepError::msg(format!("archive.unzip write {}: {e}", target.display())))?;
        }
        count += 1;
    }
    Ok(count)
}
```

用下面替换 `UnzipAction::execute` 的桩实现:

```rust
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UnzipIn {
            src,
            dest,
            max_total_bytes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("archive.unzip input invalid: {e}")))?;
        let src_path = PathBuf::from(&src);
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_read(&src_path)?;
        let max_total = max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES);

        // Pre-scan: validate every entry path (zip-slip) and cap-check writes
        // before extracting a single byte.
        {
            let file = std::fs::File::open(&src_path).map_err(|e| {
                StepError::msg(format!("archive.unzip open {}: {e}", src_path.display()))
            })?;
            let mut zr = zip::ZipArchive::new(file).map_err(|e| {
                StepError::msg(format!("archive.unzip read {}: {e}", src_path.display()))
            })?;
            for i in 0..zr.len() {
                let entry = zr
                    .by_index(i)
                    .map_err(|e| StepError::msg(format!("archive.unzip entry {i}: {e}")))?;
                let name = entry.name().to_string();
                let is_dir = entry.is_dir();
                let target = safe_join(&dest_path, &name)?;
                if !is_dir {
                    ctx.ensure_fs_write(&target)?;
                }
            }
        }

        let dp = dest_path.clone();
        let count = tokio::task::spawn_blocking(move || extract_zip(&src_path, &dp, max_total))
            .await
            .map_err(|e| StepError::msg(format!("archive.unzip task: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "dest": dest,
            "entries": count,
        })))
    }
```

- [ ] **Step 4: 跑全部 archive 测试,确认通过**

Run: `cargo test -p lumo-actions --test archive_ops`
Expected: PASS(5 passed:往返、空输入、越权、zip-slip、超限)。

- [ ] **Step 5: clippy + fmt**

Run: `cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions`
Expected: 无警告。

- [ ] **Step 6: 提交**

```bash
git add crates/lumo-actions/src/archive.rs crates/lumo-actions/tests/archive_ops.rs
git commit -m "feat(F-7): archive.unzip 解压动作(zip-slip 折叠校验 + zip-bomb 字节上限)"
```

---

## Task 3: `http.download` — 流式下载落盘

**Files:**
- Modify: `crates/lumo-actions/src/http.rs`(加 import、`default_max_bytes`、`DownloadAction`、注册)
- Test: `crates/lumo-actions/tests/http_download_upload.rs`(新建)

- [ ] **Step 1: 写失败测试**

创建 `crates/lumo-actions/tests/http_download_upload.rs`:

```rust
//! Integration coverage for `http.download` / `http.upload` and the
//! `http.request` size cap (S-class F-11). Hermetic via `wiremock` + tempdir.

mod common;
use common::{run, run_with, Capabilities};
use serde_json::json;
use wiremock::matchers::{body_string, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

/// Grant a tempdir for writes/reads AND localhost for the network.
fn net_fs(host: &str, dir: &std::path::Path) -> Capabilities {
    let glob = format!("{}/**", dir.display());
    Capabilities {
        network: vec![host.to_string()],
        fs_read: vec![glob.clone()],
        fs_write: vec![glob],
        ..Default::default()
    }
}

#[tokio::test]
async fn download_writes_file_and_reports_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello-dl"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("got.bin");
    let out = common::ok_with(
        "http.download",
        json!({"url": format!("{}/file", server.uri()), "dest": dest}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["bytes"], json!(8));
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello-dl");
}

#[tokio::test]
async fn download_rejects_oversize_and_leaves_no_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(100)))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("toobig.bin");
    let err = run_with(
        "http.download",
        json!({"url": format!("{}/big", server.uri()), "dest": dest, "max_bytes": 10}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("max_bytes"), "got: {err}");
    assert!(!dest.exists(), "rejected download must not leave a partial file");
}

#[tokio::test]
async fn download_denied_without_network_grant() {
    let dir = tempfile::tempdir().unwrap();
    let err = run("http.download", json!({"url": "https://example.com/x", "dest": dir.path().join("x")}))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
```

- [ ] **Step 2: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test http_download_upload download_`
Expected: FAIL(`action 'http.download' is not registered`)。

- [ ] **Step 3: 实现 `http.download`**

`crates/lumo-actions/src/http.rs` 顶部 import 区(`use std::collections::HashMap;` 之后)加:

```rust
use std::path::PathBuf;
```

`register` 改为:

```rust
pub fn register(r: &mut ActionRegistry) {
    r.register(RequestAction);
    r.register(DownloadAction);
    r.register(UploadAction);
}
```

在 `default_timeout_ms` 之后加默认上限助手:

```rust
fn default_max_bytes() -> u64 {
    100 * 1024 * 1024 // 100 MiB
}
```

在文件末尾追加 `DownloadAction`:

```rust
// ─── http.download ────────────────────────────────────────────────────────────

pub struct DownloadAction;

#[derive(Deserialize)]
struct DownloadIn {
    url: String,
    dest: String,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for DownloadAction {
    fn id(&self) -> &'static str {
        "http.download"
    }
    fn summary(&self) -> &'static str {
        "Stream an HTTP GET response to a file, capped at max_bytes"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url", "dest"],
                "properties": {
                    "url": { "type": "string" },
                    "dest": { "type": "string" },
                    "max_bytes": { "type": "integer" },
                    "headers": { "type": "object" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DownloadIn {
            url,
            dest,
            max_bytes,
            headers,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.download input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_write(&dest_path)?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| StepError::msg(format!("http client: {e}")))?;
        let mut req = client.get(&url);
        for (k, v) in &headers {
            req = req.header(k, v);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| StepError::msg(format!("http.download send: {e}")))?;
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Cheap pre-check: refuse before opening the file if the server declares
        // a length over the cap.
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(StepError::msg(format!(
                    "http.download: Content-Length {len} exceeds max_bytes {max_bytes}"
                )));
            }
        }

        if let Some(parent) = dest_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let mut file = tokio::fs::File::create(&dest_path)
            .await
            .map_err(|e| StepError::msg(format!("http.download create {}: {e}", dest_path.display())))?;

        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| StepError::msg(format!("http.download stream: {e}")))?;
            downloaded += chunk.len() as u64;
            // Streaming guard: also catches chunked / unknown-length responses
            // that the Content-Length pre-check can't see.
            if downloaded > max_bytes {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(StepError::msg(format!(
                    "http.download: response exceeds max_bytes {max_bytes}"
                )));
            }
            file.write_all(&chunk)
                .await
                .map_err(|e| StepError::msg(format!("http.download write: {e}")))?;
        }
        file.flush()
            .await
            .map_err(|e| StepError::msg(format!("http.download flush: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "dest": dest,
            "bytes": downloaded,
            "status": status,
            "content_type": content_type,
        })))
    }
}
```

> 说明:文件在 Content-Length 预检 **之后** 才创建,故预检拒绝时不留残件(对应测试 `!dest.exists()`)。流式 guard 是对 chunked/无长度响应的兜底,CI 测试以预检路径为准(wiremock 总会带 Content-Length)。

- [ ] **Step 4: 跑测试,确认通过**

Run: `cargo test -p lumo-actions --test http_download_upload download_`
Expected: PASS(3 passed)。

- [ ] **Step 5: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/src/http.rs crates/lumo-actions/tests/http_download_upload.rs
git commit -m "feat(F-11): http.download 流式下载落盘(Content-Length 预检 + 流式 max_bytes 兜底)"
```

---

## Task 4: `http.upload` — multipart / body 上传

**Files:**
- Modify: `crates/lumo-actions/src/http.rs`(加 `UploadAction`)
- Test: `crates/lumo-actions/tests/http_download_upload.rs`(加 upload 用例)

- [ ] **Step 1: 写失败测试**

在 `tests/http_download_upload.rs` 末尾追加:

```rust
#[tokio::test]
async fn upload_multipart_sends_file_part() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/upload"))
        .and(body_string_contains("hello-up"))
        .and(body_string_contains("name=\"file\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("payload.txt");
    std::fs::write(&src, "hello-up").unwrap();

    let out = common::ok_with(
        "http.upload",
        json!({"url": format!("{}/upload", server.uri()), "src": src, "mode": "multipart"}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await;
    assert_eq!(out["status"], json!(200));
    assert_eq!(out["json"], json!({"ok": true}));
}

#[tokio::test]
async fn upload_body_put_sends_raw_contents() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/object"))
        .and(body_string("raw-bytes"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("obj.bin");
    std::fs::write(&src, "raw-bytes").unwrap();

    let out = common::ok_with(
        "http.upload",
        json!({"url": format!("{}/object", server.uri()), "src": src, "mode": "body"}),
        net_fs("127.0.0.1", dir.path()),
    )
    .await;
    assert_eq!(out["status"], json!(200));
}

#[tokio::test]
async fn upload_denied_without_network_grant() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("x.txt");
    std::fs::write(&src, "x").unwrap();
    // fs grant only, no network → denied at the network gate.
    let err = run_with(
        "http.upload",
        json!({"url": "https://example.com/u", "src": src, "mode": "body"}),
        {
            let glob = format!("{}/**", dir.path().display());
            Capabilities { fs_read: vec![glob], ..Default::default() }
        },
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
```

- [ ] **Step 2: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test http_download_upload upload_`
Expected: FAIL(`action 'http.upload' is not registered`)。

- [ ] **Step 3: 实现 `http.upload`**

在 `crates/lumo-actions/src/http.rs` 末尾追加:

```rust
// ─── http.upload ──────────────────────────────────────────────────────────────

pub struct UploadAction;

#[derive(Deserialize)]
struct UploadIn {
    url: String,
    src: String,
    mode: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    field: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for UploadAction {
    fn id(&self) -> &'static str {
        "http.upload"
    }
    fn summary(&self) -> &'static str {
        "Upload a local file via multipart form or raw request body"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url", "src", "mode"],
                "properties": {
                    "url": { "type": "string" },
                    "src": { "type": "string" },
                    "mode": { "type": "string", "enum": ["multipart", "body"] },
                    "method": { "type": "string" },
                    "field": { "type": "string" },
                    "filename": { "type": "string" },
                    "headers": { "type": "object" },
                    "max_bytes": { "type": "integer" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UploadIn {
            url,
            src,
            mode,
            method,
            field,
            filename,
            headers,
            max_bytes,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("http.upload input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let src_path = PathBuf::from(&src);
        ctx.ensure_fs_read(&src_path)?;

        let meta = tokio::fs::metadata(&src_path)
            .await
            .map_err(|e| StepError::msg(format!("http.upload stat {}: {e}", src_path.display())))?;
        if meta.len() > max_bytes {
            return Err(StepError::msg(format!(
                "http.upload: file size {} exceeds max_bytes {max_bytes}",
                meta.len()
            )));
        }
        let bytes = tokio::fs::read(&src_path)
            .await
            .map_err(|e| StepError::msg(format!("http.upload read {}: {e}", src_path.display())))?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| StepError::msg(format!("http client: {e}")))?;

        let resp = match mode.as_str() {
            "multipart" => {
                let field = field.unwrap_or_else(|| "file".into());
                let filename = filename.unwrap_or_else(|| {
                    src_path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "file".into())
                });
                let part = reqwest::multipart::Part::bytes(bytes).file_name(filename);
                let form = reqwest::multipart::Form::new().part(field, part);
                let m = method.unwrap_or_else(|| "POST".into());
                let mut req = client
                    .request(
                        m.parse()
                            .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                        &url,
                    )
                    .multipart(form);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                req.send()
                    .await
                    .map_err(|e| StepError::msg(format!("http.upload send: {e}")))?
            }
            "body" => {
                let m = method.unwrap_or_else(|| "PUT".into());
                let mut req = client
                    .request(
                        m.parse()
                            .map_err(|e| StepError::msg(format!("bad method: {e}")))?,
                        &url,
                    )
                    .body(bytes);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                req.send()
                    .await
                    .map_err(|e| StepError::msg(format!("http.upload send: {e}")))?
            }
            other => {
                return Err(StepError::msg(format!(
                    "http.upload: mode must be `multipart` or `body`, got `{other}`"
                )))
            }
        };

        let status = resp.status().as_u16();
        let resp_headers: HashMap<_, _> = resp
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http.upload body: {e}")))?;
        let body_json: Option<Value> = serde_json::from_str(&text).ok();

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "headers": resp_headers,
            "text": text,
            "json": body_json,
        })))
    }
}
```

- [ ] **Step 4: 跑测试,确认通过**

Run: `cargo test -p lumo-actions --test http_download_upload upload_`
Expected: PASS(3 passed)。

- [ ] **Step 5: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/src/http.rs crates/lumo-actions/tests/http_download_upload.rs
git commit -m "feat(F-11): http.upload 上传动作(multipart/body 双模式 + max_bytes 防 OOM)"
```

---

## Task 5: `http.request` 响应大小上限

**Files:**
- Modify: `crates/lumo-actions/src/http.rs`(`ReqIn` 加 `max_bytes`,schema + 执行体加上限校验)
- Test: `crates/lumo-actions/tests/http_download_upload.rs`(加 request max_bytes 用例)

- [ ] **Step 1: 写失败测试**

在 `tests/http_download_upload.rs` 末尾追加:

```rust
#[tokio::test]
async fn request_rejects_body_over_max_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/huge"))
        .respond_with(ResponseTemplate::new(200).set_body_string("y".repeat(100)))
        .mount(&server)
        .await;

    let err = run_with(
        "http.request",
        json!({"url": format!("{}/huge", server.uri()), "max_bytes": 10}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("max_bytes"), "got: {err}");
}
```

- [ ] **Step 2: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test http_download_upload request_rejects_body_over_max_bytes`
Expected: FAIL(目前 `http.request` 忽略 `max_bytes`,`additionalProperties:false` 还会让带 `max_bytes` 的输入解析报错)。

- [ ] **Step 3: 实现增强(向后兼容,默认 100 MiB)**

`crates/lumo-actions/src/http.rs` 的 `ReqIn` 结构在 `timeout_ms` 字段后加:

```rust
    #[serde(default = "default_max_bytes")]
    max_bytes: u64,
```

解构 `let ReqIn { ... }` 同步加上 `max_bytes,`。

`RequestAction::schema` 的 `properties` 里(`"timeout_ms"` 之后)加:

```rust
                    "max_bytes": { "type": "integer" },
```

执行体中,把读取响应体的那段:

```rust
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http body: {e}")))?;
        let body_json: Option<Value> = serde_json::from_str(&text).ok();
```

替换为(前置 Content-Length 预检 + 读后长度兜底):

```rust
        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(StepError::msg(format!(
                    "http.request: response Content-Length {len} exceeds max_bytes {max_bytes}"
                )));
            }
        }
        let text = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("http body: {e}")))?;
        if text.len() as u64 > max_bytes {
            return Err(StepError::msg(format!(
                "http.request: response body {} bytes exceeds max_bytes {max_bytes}",
                text.len()
            )));
        }
        let body_json: Option<Value> = serde_json::from_str(&text).ok();
```

- [ ] **Step 4: 跑测试(含既有 http_ops 回归),确认通过**

Run: `cargo test -p lumo-actions --test http_download_upload --test http_ops`
Expected: PASS(新用例通过;既有 `http_ops.rs` 3 个用例仍绿——默认上限 100 MiB 不影响)。

- [ ] **Step 5: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/src/http.rs crates/lumo-actions/tests/http_download_upload.rs
git commit -m "feat(F-11): http.request 响应大小上限 max_bytes(默认 100 MiB,向后兼容)"
```

---

## Task 6: `notify.send` — 新建 `notify.rs`(4 provider + 加签)

**Files:**
- Modify: `crates/lumo-actions/Cargo.toml`(加 `hmac`)
- Create: `crates/lumo-actions/src/notify.rs`
- Modify: `crates/lumo-actions/src/lib.rs`(声明 + 注册)
- Test: `crates/lumo-actions/tests/notify_ops.rs`

- [ ] **Step 1: 加依赖**

`crates/lumo-actions/Cargo.toml` 的 `[dependencies]` 加(`sha2` 已在树、`base64 = "0.22"` 已有):

```toml
hmac = "0.12"
```

- [ ] **Step 2: 声明并注册模块**

`crates/lumo-actions/src/lib.rs`:加 `pub mod notify;`(按字母序,放 `pub mod math_ops;` 之后),并在 `register_all` 的 `db_ops::register(registry);` 之后加 `notify::register(registry);`。

```rust
pub mod notify;
```

```rust
    notify::register(registry);
```

- [ ] **Step 3: 写失败测试**

创建 `crates/lumo-actions/tests/notify_ops.rs`:

```rust
//! Integration coverage for `notify.send` (S-class F-8). `wiremock` captures the
//! outgoing request so we assert provider body shape + signing fields offline.

mod common;
use common::{ok_with, run, Capabilities};
use serde_json::{json, Value};
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn net(host: &str) -> Capabilities {
    Capabilities {
        network: vec![host.to_string()],
        ..Default::default()
    }
}

#[tokio::test]
async fn dingtalk_text_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .and(body_json(json!({"msgtype": "text", "text": {"content": "hi"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errcode": 0})))
        .mount(&server)
        .await;

    let out = ok_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["status"], json!(200));
}

#[tokio::test]
async fn feishu_text_body_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .and(body_json(json!({"msg_type": "text", "content": {"text": "hi"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .mount(&server)
        .await;

    let out = ok_with(
        "notify.send",
        json!({"provider": "feishu", "url": format!("{}/hook", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await;
    assert_eq!(out["ok"], json!(true));
}

#[tokio::test]
async fn dingtalk_secret_appends_timestamp_and_sign_to_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errcode": 0})))
        .mount(&server)
        .await;

    ok_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi", "secret": "S3CRET"}),
        net("127.0.0.1"),
    )
    .await;

    let reqs = server.received_requests().await.unwrap();
    let query = reqs[0].url.query().unwrap_or("");
    assert!(query.contains("timestamp="), "signed URL has timestamp: {query}");
    assert!(query.contains("sign="), "signed URL has sign: {query}");
}

#[tokio::test]
async fn feishu_secret_adds_timestamp_and_sign_to_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"code": 0})))
        .mount(&server)
        .await;

    ok_with(
        "notify.send",
        json!({"provider": "feishu", "url": format!("{}/hook", server.uri()), "text": "hi", "secret": "S3CRET"}),
        net("127.0.0.1"),
    )
    .await;

    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(body.get("timestamp").is_some(), "body has timestamp: {body}");
    assert!(body.get("sign").is_some(), "body has sign: {body}");
}

#[tokio::test]
async fn provider_errcode_nonzero_fails_the_step() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/robot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"errcode": 310000, "errmsg": "bad token"})))
        .mount(&server)
        .await;

    let err = run(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
    )
    .await;
    // No network grant on `run`, so this would be denied — use ok_with path:
    let _ = err;
    let err2 = common::run_with(
        "notify.send",
        json!({"provider": "dingtalk", "url": format!("{}/robot", server.uri()), "text": "hi"}),
        net("127.0.0.1"),
    )
    .await
    .unwrap_err();
    assert!(err2.contains("failed"), "got: {err2}");
}

#[tokio::test]
async fn notify_denied_without_network_grant() {
    let err = run(
        "notify.send",
        json!({"provider": "webhook", "url": "https://example.com/h", "text": "hi"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
}
```

> 注:`provider_errcode_nonzero_fails_the_step` 里第一个 `run(...)` 仅为说明无授权会被拒,真正断言用 `run_with` + `net`。实现时若觉得冗余可删掉前半段,只留 `err2` 断言。

- [ ] **Step 4: 跑测试,确认失败**

Run: `cargo test -p lumo-actions --test notify_ops`
Expected: FAIL(`action 'notify.send' is not registered`)。

- [ ] **Step 5: 实现 `notify.rs`**

创建 `crates/lumo-actions/src/notify.rs`:

```rust
//! Notification action — `notify.send` (S-class F-8).
//!
//! One unified action over four providers (DingTalk / Feishu / WeCom / generic
//! webhook). DingTalk and Feishu support HMAC-SHA256 request signing; the
//! `secret` arrives already resolved from `${{ vault.* }}` (P1-3), so it never
//! touches argv or run snapshots. A non-2xx HTTP status or a provider error code
//! fails the step so flows surface delivery failures instead of swallowing them.

use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn register(r: &mut ActionRegistry) {
    r.register(SendAction);
}

pub struct SendAction;

#[derive(Deserialize)]
struct SendIn {
    provider: String,
    url: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default = "default_msgtype")]
    msgtype: String,
    #[serde(default)]
    secret: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}
fn default_msgtype() -> String {
    "text".into()
}
fn default_timeout_ms() -> u64 {
    30_000
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn dingtalk_body(text: &str, title: Option<&str>, msgtype: &str) -> Value {
    if msgtype == "markdown" {
        serde_json::json!({
            "msgtype": "markdown",
            "markdown": { "title": title.unwrap_or("notification"), "text": text }
        })
    } else {
        serde_json::json!({ "msgtype": "text", "text": { "content": text } })
    }
}
fn feishu_body(text: &str) -> Value {
    serde_json::json!({ "msg_type": "text", "content": { "text": text } })
}
fn wecom_body(text: &str, msgtype: &str) -> Value {
    if msgtype == "markdown" {
        serde_json::json!({ "msgtype": "markdown", "markdown": { "content": text } })
    } else {
        serde_json::json!({ "msgtype": "text", "text": { "content": text } })
    }
}

/// DingTalk: `sign = base64(HMAC_SHA256(key=secret, msg="{ts}\n{secret}"))`.
fn dingtalk_sign(ts: u64, secret: &str) -> String {
    let string_to_sign = format!("{ts}\n{secret}");
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(string_to_sign.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Feishu: `sign = base64(HMAC_SHA256(key="{ts_s}\n{secret}", msg=""))`.
fn feishu_sign(ts_s: u64, secret: &str) -> String {
    let key = format!("{ts_s}\n{secret}");
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(b"");
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Compose `(final_url, body)` for a provider, applying signing when a secret is
/// present. A caller-supplied `payload` is sent verbatim (advanced escape hatch).
fn build_request(
    provider: &str,
    url: &str,
    text: Option<&str>,
    payload: Option<Value>,
    title: Option<&str>,
    msgtype: &str,
    secret: Option<&str>,
) -> Result<(String, Value), StepError> {
    let base_body = match provider {
        "dingtalk" => payload.unwrap_or_else(|| dingtalk_body(text.unwrap_or(""), title, msgtype)),
        "feishu" => payload.unwrap_or_else(|| feishu_body(text.unwrap_or(""))),
        "wecom" => payload.unwrap_or_else(|| wecom_body(text.unwrap_or(""), msgtype)),
        "webhook" => {
            payload.unwrap_or_else(|| serde_json::json!({ "text": text.unwrap_or("") }))
        }
        other => return Err(StepError::msg(format!("notify.send: unknown provider `{other}`"))),
    };

    match provider {
        "dingtalk" => {
            if let Some(secret) = secret {
                let ts = now_ms();
                let sign = dingtalk_sign(ts, secret);
                let mut u = reqwest::Url::parse(url)
                    .map_err(|e| StepError::msg(format!("notify.send bad url: {e}")))?;
                u.query_pairs_mut()
                    .append_pair("timestamp", &ts.to_string())
                    .append_pair("sign", &sign);
                Ok((u.to_string(), base_body))
            } else {
                Ok((url.to_string(), base_body))
            }
        }
        "feishu" => {
            if let Some(secret) = secret {
                let ts_s = now_ms() / 1000;
                let sign = feishu_sign(ts_s, secret);
                let mut body = base_body;
                if let Value::Object(m) = &mut body {
                    m.insert("timestamp".into(), Value::String(ts_s.to_string()));
                    m.insert("sign".into(), Value::String(sign));
                }
                Ok((url.to_string(), body))
            } else {
                Ok((url.to_string(), base_body))
            }
        }
        _ => Ok((url.to_string(), base_body)),
    }
}

/// Provider-level success: DingTalk/WeCom use `errcode`, Feishu uses `code`
/// (older webhooks `StatusCode`). Absent ⇒ assume success (rely on HTTP status).
fn provider_success(provider: &str, response: &Value) -> bool {
    match provider {
        "dingtalk" | "wecom" => response
            .get("errcode")
            .and_then(Value::as_i64)
            .map(|c| c == 0)
            .unwrap_or(true),
        "feishu" => {
            if let Some(c) = response.get("code").and_then(Value::as_i64) {
                return c == 0;
            }
            if let Some(c) = response.get("StatusCode").and_then(Value::as_i64) {
                return c == 0;
            }
            true
        }
        _ => true,
    }
}

#[async_trait]
impl Action for SendAction {
    fn id(&self) -> &'static str {
        "notify.send"
    }
    fn summary(&self) -> &'static str {
        "Send a notification (dingtalk/feishu/wecom/webhook), with optional HMAC signing"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["provider", "url"],
                "properties": {
                    "provider": { "type": "string", "enum": ["dingtalk", "feishu", "wecom", "webhook"] },
                    "url": { "type": "string" },
                    "text": { "type": "string" },
                    "payload": {},
                    "title": { "type": "string" },
                    "msgtype": { "type": "string", "enum": ["text", "markdown"] },
                    "secret": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SendIn {
            provider,
            url,
            text,
            payload,
            title,
            msgtype,
            secret,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("notify.send input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        if text.is_none() && payload.is_none() {
            return Err(StepError::msg("notify.send requires `text` or `payload`"));
        }

        let (final_url, body) = build_request(
            &provider,
            &url,
            text.as_deref(),
            payload.clone(),
            title.as_deref(),
            &msgtype,
            secret.as_deref(),
        )?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| StepError::msg(format!("http client: {e}")))?;
        let resp = client
            .post(&final_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| StepError::msg(format!("notify.send send: {e}")))?;
        let status = resp.status().as_u16();
        let text_resp = resp
            .text()
            .await
            .map_err(|e| StepError::msg(format!("notify.send body: {e}")))?;
        let response: Value =
            serde_json::from_str(&text_resp).unwrap_or(Value::String(text_resp.clone()));

        let ok = (200..300).contains(&status) && provider_success(&provider, &response);
        if !ok {
            return Err(StepError::msg(format!(
                "notify.send `{provider}` failed: status={status} response={response}"
            )));
        }

        Ok(ActionResult::from(serde_json::json!({
            "status": status,
            "ok": ok,
            "response": response,
        })))
    }
}
```

- [ ] **Step 6: 跑测试,确认通过**

Run: `cargo test -p lumo-actions --test notify_ops`
Expected: PASS(6 passed)。

- [ ] **Step 7: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/Cargo.toml crates/lumo-actions/src/notify.rs crates/lumo-actions/src/lib.rs crates/lumo-actions/tests/notify_ops.rs Cargo.lock
git commit -m "feat(F-8): notify.send 通知动作(4 provider + 钉钉/飞书 HMAC-SHA256 加签)"
```

---

## Task 7: arboard 交叉编译/许可面验证(风险闸门,带回退分支)

> 这是本批次最高风险点(设计文档 §风险)。**先**单独引入 `arboard` 验证交叉编译与许可,再写实现,避免污染前面已绿的提交。

**Files:**
- Modify: `crates/lumo-actions/Cargo.toml`(加 `arboard`)

- [ ] **Step 1: 加依赖**

`crates/lumo-actions/Cargo.toml` 的 `[dependencies]` 加:

```toml
arboard = { version = "3", default-features = false }
```

- [ ] **Step 2: 本机编译 + 查看传递依赖**

Run:
```bash
cargo check -p lumo-actions 2>&1 | tail -5
cargo tree -p lumo-actions -i arboard 2>&1 | head -30
cargo tree -p lumo-actions 2>&1 | grep -iE 'x11|wayland|xcb|smithay' | sort -u
```
Expected: 本机 `cargo check` 通过。记录 arboard 拉入的 Linux 后端传递依赖(x11rb / wayland-* / smithay-clipboard 等)。

- [ ] **Step 3: 许可面核对(deny.toml 白名单)**

Run(本机有 cargo-deny 时):
```bash
cargo deny check licenses 2>&1 | tail -20
```
无 cargo-deny 时,人工核 Step 2 列出的新传递依赖许可是否都在 `deny.toml` 的 `allow` 列表(MIT/Apache-2.0/BSD/ISC/Zlib/Unicode-3.0/MPL-2.0/...)。
Expected: 无 `rejected`/非白名单许可。**若发现非白名单许可** → 进入 Step 5 回退分支。

- [ ] **Step 4: aarch64-linux 交叉编译闸门(P1-9 硬门禁)**

Run(本机已装该 target 与交叉链接器时):
```bash
rustup target add aarch64-unknown-linux-gnu 2>/dev/null || true
cargo check -p lumo-actions --target aarch64-unknown-linux-gnu 2>&1 | tail -15
```
Expected: 通过。**若本机无交叉链接器**,跳过本步、改由 CI 的 `cross-check` job 把关(在 Task 10 推送后观察);CI 红即进入 Step 5。

- [ ] **Step 5(条件):回退分支 — 仅当 Step 3 或 Step 4 失败时执行**

把 `arboard` 改为按目标条件引入:删 `[dependencies]` 里那行,改在 `Cargo.toml` 末尾加(排除交叉门禁目标):

```toml
[target.'cfg(not(all(target_os = "linux", target_arch = "aarch64")))'.dependencies]
arboard = { version = "3", default-features = false }
```

Task 8 实现里的 `clipboard_get_text`/`clipboard_set_text` 已用 `#[cfg]` 双分支写法(见 Task 8 Step 3 的「回退桩」注释),不支持目标上返回 `clipboard unavailable on this target`,动作仍始终注册,schema 跨目标一致。

- [ ] **Step 6: 不单独提交**

本任务仅验证 + 落 Cargo.toml 一行(及可能的回退条目)。依赖行随 Task 8 的实现一起提交,保持「依赖 + 用法」原子。

---

## Task 8: `clipboard.get` / `clipboard.set` — 新建 `clipboard.rs`

**Files:**
- Create: `crates/lumo-actions/src/clipboard.rs`
- Modify: `crates/lumo-actions/src/lib.rs`(声明 + 注册)
- Test: `crates/lumo-actions/tests/clipboard_ops.rs`

- [ ] **Step 1: 声明并注册模块**

`crates/lumo-actions/src/lib.rs`:加 `pub mod clipboard;`(按字母序,放 `pub mod browser;` 之后、`pub mod control;` 之前),并在 `register_all` 的 `notify::register(registry);` 之后加 `clipboard::register(registry);`。

```rust
pub mod clipboard;
```

```rust
    clipboard::register(registry);
```

- [ ] **Step 2: 写失败测试(CI 可跑校验 + 真实往返 ignore)**

创建 `crates/lumo-actions/tests/clipboard_ops.rs`:

```rust
//! Coverage for `clipboard.get` / `clipboard.set` (S-class F-5). Input-validation
//! cases run in CI (they never touch the clipboard); the real round-trip needs a
//! display/clipboard backend and is `#[ignore]`d, run locally with `--ignored`.

mod common;
use common::{ok, run};
use serde_json::json;

#[tokio::test]
async fn set_requires_text() {
    // Reaches input parsing (and proves the action is registered) without
    // touching the clipboard, so it is CI-safe even headless.
    let err = run("clipboard.set", json!({})).await.unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
#[ignore = "needs a real display/clipboard; run with --ignored"]
async fn set_then_get_round_trips() {
    ok("clipboard.set", json!({"text": "lumo-clip-test"})).await;
    let out = ok("clipboard.get", json!({})).await;
    assert_eq!(out["text"], json!("lumo-clip-test"));
}
```

- [ ] **Step 3: 跑 CI 用例,确认失败**

Run: `cargo test -p lumo-actions --test clipboard_ops set_requires_text`
Expected: FAIL(`action 'clipboard.set' is not registered`)。

- [ ] **Step 4: 实现 `clipboard.rs`**

创建 `crates/lumo-actions/src/clipboard.rs`:

```rust
//! Clipboard actions — `clipboard.get` / `clipboard.set` (S-class F-5).
//!
//! Plain-text clipboard via `arboard`. Each call builds a short-lived
//! `Clipboard` inside `spawn_blocking` (arboard's handle is `!Send` and the call
//! is blocking). Headless / no-display environments surface a clear
//! `clipboard unavailable: …` error instead of panicking.
//!
//! No capability gate — these are local, info-only actions like `system.env_get`.
//! Two caveats flow authors should know: (1) reading the clipboard can expose
//! sensitive data (e.g. a password manager's last copy); (2) on Linux/X11 the
//! contents written may not persist after this process exits.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;

pub fn register(r: &mut ActionRegistry) {
    r.register(GetAction);
    r.register(SetAction);
}

// Indirection so the arboard backend can be swapped for a per-target stub
// (Task 7 fallback) without touching the action bodies.
fn clipboard_get_text() -> Result<String, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.get_text().map_err(|e| format!("clipboard read: {e}"))
}
fn clipboard_set_text(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    cb.set_text(text).map_err(|e| format!("clipboard write: {e}"))
}

pub struct GetAction;

#[async_trait]
impl Action for GetAction {
    fn id(&self) -> &'static str {
        "clipboard.get"
    }
    fn summary(&self) -> &'static str {
        "Read text from the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let text = tokio::task::spawn_blocking(clipboard_get_text)
            .await
            .map_err(|e| StepError::msg(format!("clipboard.get task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "text": text })))
    }
}

pub struct SetAction;

#[derive(Deserialize)]
struct SetIn {
    text: String,
}

#[async_trait]
impl Action for SetAction {
    fn id(&self) -> &'static str {
        "clipboard.set"
    }
    fn summary(&self) -> &'static str {
        "Write text to the system clipboard"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text"],
                "properties": { "text": { "type": "string" } },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("clipboard.set input invalid: {e}")))?;
        tokio::task::spawn_blocking(move || clipboard_set_text(text))
            .await
            .map_err(|e| StepError::msg(format!("clipboard.set task: {e}")))?
            .map_err(StepError::msg)?;
        Ok(ActionResult::from(serde_json::json!({ "ok": true })))
    }
}
```

> **Task 7 回退桩**(仅当 Task 7 Step 5 触发时):把上面两个 `clipboard_*` 助手替换为 `#[cfg]` 双分支——
> ```rust
> #[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
> fn clipboard_get_text() -> Result<String, String> {
>     let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
>     cb.get_text().map_err(|e| format!("clipboard read: {e}"))
> }
> #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
> fn clipboard_get_text() -> Result<String, String> {
>     Err("clipboard unavailable on this target".into())
> }
> ```
> `clipboard_set_text` 同理(不支持目标返回同样错误)。动作始终注册,schema 跨目标一致。

- [ ] **Step 5: 跑 CI 用例,确认通过**

Run: `cargo test -p lumo-actions --test clipboard_ops set_requires_text`
Expected: PASS(1 passed;`set_then_get_round_trips` 标记 ignored 被跳过)。

- [ ] **Step 6: 本机真实往返自检(可选,有显示环境)**

Run: `cargo test -p lumo-actions --test clipboard_ops -- --ignored`
Expected: macOS 桌面下 PASS;headless 下报 `clipboard unavailable`(预期,不计入 CI)。

- [ ] **Step 7: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/Cargo.toml crates/lumo-actions/src/clipboard.rs crates/lumo-actions/src/lib.rs crates/lumo-actions/tests/clipboard_ops.rs Cargo.lock
git commit -m "feat(F-5): clipboard.get/set 文本剪贴板(arboard + spawn_blocking,无显示优雅报错)"
```

---

## Task 9: `browser.wait` — 扩展 `browser.rs`

> 复用 `build_selector` / `session_for_run` / `current_page`,但**不**调 `resolve_element`:轮询里反复调用会向 `SelectorStats` 写入大量「失败」记录、污染选择器学习。改用一段自包含 JS,每个 poll 单次 `evaluate` 返回 `matched` 布尔。

**Files:**
- Modify: `crates/lumo-actions/src/browser.rs`(`register` 加注册 + 末尾追加 `WaitAction` 及 JS 常量)
- Test: `crates/lumo-actions/tests/browser_wait.rs`

- [ ] **Step 1: 写失败测试(CI 校验 + 真实行为 ignore)**

创建 `crates/lumo-actions/tests/browser_wait.rs`:

```rust
//! Coverage for `browser.wait` (S-class F-9). Input/condition validation runs in
//! CI (it errors before any browser session is needed); behavioural waits need a
//! real Chrome and are `#[ignore]`d alongside the other browser e2e tests.

mod common;
use common::run;
use serde_json::json;

#[tokio::test]
async fn wait_requires_selector_or_text() {
    let err = run("browser.wait", json!({})).await.unwrap_err();
    assert!(err.contains("requires"), "got: {err}");
}

#[tokio::test]
async fn wait_rejects_unknown_condition() {
    let err = run(
        "browser.wait",
        json!({"selector": "#x", "condition": "bogus"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("condition"), "got: {err}");
}

#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn wait_visible_resolves_after_open() {
    // Sketch for local e2e: browser.open a data: URL with a visible element,
    // then browser.wait { selector, condition: "visible" } returns matched.
    // Requires the VM/registry browser-session plumbing; left as a manual e2e.
}
```

- [ ] **Step 2: 跑 CI 用例,确认失败**

Run: `cargo test -p lumo-actions --test browser_wait wait_requires_selector_or_text wait_rejects_unknown_condition`
Expected: FAIL(`action 'browser.wait' is not registered`)。

- [ ] **Step 3: 注册 + 实现**

`crates/lumo-actions/src/browser.rs` 的 `register` 末尾(`r.register(ExtractAction);` 之后、`r.register_teardown(...)` 之前)加:

```rust
    r.register(WaitAction);
```

在 `browser.rs` 文件末尾追加:

```rust
// ─── browser.wait (F-9) ───────────────────────────────────────────────────────

/// Per-poll JS-eval budget. The query itself is cheap; this only bounds a
/// pathological evaluate() call, not the overall wait (that's `timeout_ms`).
const WAIT_EVAL_TIMEOUT_MS: u64 = 2_000;

const WAIT_CONDITIONS: &[&str] = &["present", "visible", "clickable", "hidden"];

/// Self-contained matcher: locates the element by the same strategy order as the
/// resolver, then returns a single boolean for the requested condition. Kept
/// separate from `resolve_element` so the poll loop never writes SelectorStats.
const WAIT_JS_TEMPLATE: &str = r#"
((spec, condition, needle) => {
  const escape = (s) => (window.CSS && CSS.escape) ? CSS.escape(String(s)) : String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  const find = () => {
    if (spec.id) { const e = document.getElementById(spec.id); if (e) return e; }
    if (spec.data_testid) { const e = document.querySelector(`[data-testid="${escape(spec.data_testid)}"]`); if (e) return e; }
    if (spec.css) { const e = document.querySelector(spec.css); if (e) return e; }
    if (spec.aria_label) {
      const e = document.querySelector(`[aria-label="${escape(spec.aria_label)}"]`);
      if (e) return e;
      const m = Array.from(document.querySelectorAll('*')).find((el) => el.getAttribute && el.getAttribute('aria-label') === spec.aria_label);
      if (m) return m;
    }
    if (spec.text_includes) {
      const t = String(spec.text_includes).trim();
      const cands = document.querySelectorAll('button, a, span, label, div, li, td, th, h1, h2, h3, h4, h5, h6, p');
      for (const el of cands) { if ((el.innerText || '').trim().includes(t)) return el; }
    }
    if (spec.xpath) {
      try { const r = document.evaluate(spec.xpath, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null); if (r.singleNodeValue) return r.singleNodeValue; } catch (_) {}
    }
    return null;
  };
  const hasSpec = !!(spec.id || spec.data_testid || spec.css || spec.aria_label || spec.text_includes || spec.xpath);
  const visible = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    if (!(r.width > 0 && r.height > 0)) return false;
    const st = window.getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none' || parseFloat(st.opacity) === 0) return false;
    return true;
  };
  const clickable = (el) => visible(el) && !el.disabled && el.getAttribute('aria-disabled') !== 'true';
  const containsText = (el, n) => !!el && (el.innerText || '').includes(n);
  if (!hasSpec) {
    return !!(document.body && (document.body.innerText || '').includes(needle || ''));
  }
  const el = find();
  switch (condition) {
    case 'present': return !!el;
    case 'visible': return visible(el) && (needle ? containsText(el, needle) : true);
    case 'clickable': return clickable(el) && (needle ? containsText(el, needle) : true);
    case 'hidden': return !el || !visible(el);
    default: return false;
  }
})(__SPEC__, "__COND__", __NEEDLE__)
"#;

pub struct WaitAction;

#[derive(Deserialize)]
struct WaitIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    #[serde(default = "default_condition")]
    condition: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default = "default_wait_timeout_ms")]
    timeout_ms: u64,
}
fn default_condition() -> String {
    "visible".into()
}
fn default_wait_timeout_ms() -> u64 {
    30_000
}

async fn wait_matches(
    page: &Page,
    spec: Option<&MultiSelector>,
    condition: &str,
    text: Option<&str>,
) -> Result<bool, StepError> {
    let spec_json = match spec {
        Some(s) => serde_json::json!({
            "id": s.id, "data_testid": s.data_testid, "css": s.css,
            "aria_label": s.aria_label, "text_includes": s.text_includes, "xpath": s.xpath,
        }),
        None => serde_json::json!({}),
    };
    let needle_json = serde_json::to_string(text.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    let js = WAIT_JS_TEMPLATE
        .replace("__SPEC__", &spec_json.to_string())
        .replace("__COND__", condition)
        .replace("__NEEDLE__", &needle_json);
    let val = tokio::time::timeout(
        Duration::from_millis(WAIT_EVAL_TIMEOUT_MS),
        page.evaluate(js),
    )
    .await
    .map_err(|_| StepError::msg("browser.wait: page eval timed out"))?
    .map_err(|e| StepError::msg(format!("browser.wait eval: {e}")))?;
    Ok(val.into_value::<bool>().unwrap_or(false))
}

#[async_trait]
impl Action for WaitAction {
    fn id(&self) -> &'static str {
        "browser.wait"
    }
    fn summary(&self) -> &'static str {
        "Wait until an element is present/visible/clickable/hidden, or text appears"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "selectors": multi_selector_schema(),
                    "condition": { "type": "string", "enum": ["present", "visible", "clickable", "hidden"] },
                    "text": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WaitIn {
            selector,
            selectors,
            condition,
            text,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.wait input invalid: {e}")))?;

        // Validate before needing a browser session, so bad input fails fast
        // (and unit-testably) without launching Chrome.
        if !WAIT_CONDITIONS.contains(&condition.as_str()) {
            return Err(StepError::msg(format!(
                "browser.wait: unknown condition `{condition}` (present/visible/clickable/hidden)"
            )));
        }
        let has_selector = selector.as_ref().is_some_and(|s| !s.is_empty())
            || selectors.as_ref().is_some_and(|s| !s.is_empty());
        if !has_selector && text.is_none() {
            return Err(StepError::msg(
                "browser.wait requires `selector`/`selectors` or `text`",
            ));
        }
        let spec = if has_selector {
            Some(build_selector(selector, selectors)?)
        } else {
            None
        };

        let s = session_for_run(ctx.run_id())?;
        let page = current_page(&s)?;

        let deadline = Duration::from_millis(timeout_ms);
        let start = std::time::Instant::now();
        let poll = Duration::from_millis(100);
        loop {
            if wait_matches(&page, spec.as_ref(), &condition, text.as_deref()).await? {
                let matched = spec
                    .as_ref()
                    .map(|s| s.first_hint())
                    .unwrap_or_else(|| format!("text:{}", text.as_deref().unwrap_or("")));
                return Ok(ActionResult::from(serde_json::json!({
                    "condition": condition,
                    "matched": matched,
                    "waited_ms": start.elapsed().as_millis() as u64,
                })));
            }
            if start.elapsed() >= deadline {
                let what = spec
                    .as_ref()
                    .map(|s| s.first_hint())
                    .unwrap_or_else(|| format!("text `{}`", text.as_deref().unwrap_or("")));
                return Err(StepError::msg(format!(
                    "browser.wait: condition `{condition}` not met within {timeout_ms}ms for {what}"
                )));
            }
            tokio::time::sleep(poll).await;
        }
    }
}
```

> 依赖说明:`Duration`、`MultiSelector`、`build_selector`、`session_for_run`、`current_page`、`Page`、`Lazy`、`Value`、`multi_selector_schema` 均已在 `browser.rs` 现有 import / 定义中;无需新增 use。

- [ ] **Step 4: 跑 CI 用例,确认通过**

Run: `cargo test -p lumo-actions --test browser_wait wait_requires_selector_or_text wait_rejects_unknown_condition`
Expected: PASS(2 passed;`wait_visible_resolves_after_open` 被 ignore 跳过)。

- [ ] **Step 5: clippy + fmt + 提交**

```bash
cargo clippy -p lumo-actions --all-targets -- -D warnings && cargo fmt -p lumo-actions
git add crates/lumo-actions/src/browser.rs crates/lumo-actions/tests/browser_wait.rs
git commit -m "feat(F-9): browser.wait 显式等待(present/visible/clickable/hidden/text 轮询)"
```

---

## Task 10: 整批验证 + 路线图勾选

**Files:**
- Modify: `docs/04-优化与补充开发-路线图.md`(F-5/F-7/F-8/F-9/F-11 勾选)

- [ ] **Step 1: 整 crate 测试(CI 路径,排除真实环境用例)**

Run: `LUMO_SKIP_BROWSER_TESTS=1 cargo test -p lumo-actions 2>&1 | grep -E "test result|error|FAILED" | tail -30`
Expected: 各测试文件 `test result: ok`;无 `FAILED`。`#[ignore]` 的 clipboard/browser 真实用例被跳过。

- [ ] **Step 2: 全工作区编译 + clippy(确认未碰坏下游 crate)**

Run:
```bash
cargo build -p lumo-cli 2>&1 | tail -3
cargo clippy -p lumo-actions --all-targets -- -D warnings 2>&1 | tail -3
```
Expected: 编译通过;clippy 无警告。

- [ ] **Step 3: 供应链/许可门禁(本机有 cargo-deny 时)**

Run: `cargo deny check 2>&1 | tail -20`
Expected: licenses/advisories/bans/sources 全 `ok`。无 cargo-deny 时,记一句「待 CI cargo-deny job 把关」。

- [ ] **Step 4: 勾选路线图**

`docs/04-优化与补充开发-路线图.md` 五处改 `- [ ]` 为 `- [x]`(行号以当前文件为准:69/71/72/73/75):

```
- [x] **F-5 剪贴板 clipboard.get/set**(S)
```
```
- [x] **F-7 ZIP/归档**(S)
```
```
- [x] **F-8 通知(钉钉/飞书/企微/webhook)**(S~M)
```
```
- [x] **F-9 显式 `browser.wait`(visible/clickable/text)**(S)
```
```
- [x] **F-11 http.download/上传 + 响应大小上限**(S)
```

- [ ] **Step 5: 提交**

```bash
git add docs/04-优化与补充开发-路线图.md
git commit -m "docs(S-class): 路线图勾选 F-5/F-7/F-8/F-9/F-11(标准库/实用工具动作批次完成)"
```

- [ ] **Step 6: 整体 code review**

请求 `superpowers:requesting-code-review`,聚焦:能力沙箱无绕过、`archive.unzip` zip-slip/zip-bomb、`notify` secret 不入日志、`spawn_blocking` 用法、新依赖许可面。

---

## 自审(writing-plans Self-Review)

**1. Spec 覆盖**:F-7(Task 1-2)、F-11 download/upload/request(Task 3-5)、F-8(Task 6)、F-5(Task 7-8)、F-9(Task 9)、横切的依赖/许可/交叉编译(Task 7 + Task 10 Step 3)、测试策略(各任务 CI 用例 + `#[ignore]`)、落地顺序(Task 编号即设计的递增顺序)、路线图勾选(Task 10)。设计稿各节均有对应任务,无遗漏。

**2. Placeholder 扫描**:无 "TBD/TODO/类似上文"。唯一两处刻意的「桩」均给了完整可编译代码 + 替换时机:① Task 1 的 `UnzipAction` 占位 `execute`(Task 2 Step 3 给真实实现)、② Task 7/8 的 arboard 回退分支(给了完整 `#[cfg]` 双分支代码)。`browser_wait` 的 e2e 用例是空 `#[ignore]` 草图(行为验证须真实 Chrome,与既有 `browser_teardown.rs` 一致),已说明留作手动 e2e。

**3. 类型一致性**:`StepCtx::ensure_fs_read/ensure_fs_write(&Path)`、`ensure_network_url(&str)`、`ActionResult::from(Value)`/`null()`、`StepError::msg(impl Into<String>)`、`Action` 四方法签名、测试夹具 `run/run_with/ok/ok_with/fs_caps`、`Capabilities { network/fs_read/fs_write }`、`MultiSelector`/`build_selector`/`session_for_run`/`current_page`/`Page` —— 均与已读源码逐一核对一致。`default_max_bytes`/`default_timeout_ms` 在 `http.rs` 全局复用;`SimpleFileOptions`/`ZipArchive`/`by_index`/`name`/`is_dir` 对应 `zip 2.2` API。

> 实现期唯一需现场确认的外部 API 是 `zip 2.2` 的 `SimpleFileOptions`/`start_file` 形态(已按 2.1+ 稳定形态写;若 `cargo build` 报签名不符,`cargo doc -p zip --open` 查 `write::SimpleFileOptions`)。TDD 的「跑测试看失败/通过」会立即暴露任何偏差。

---

## 执行交接

**Plan complete and saved to `docs/superpowers/plans/2026-05-30-s-class-actions.md`. 两种执行方式:**

**1. Subagent-Driven(推荐)** —— 每个 Task 派一个全新 subagent 实现,任务之间我做两段式 review,迭代快。

**2. Inline Execution** —— 在本会话内用 executing-plans 批量执行,带 checkpoint 复核。

**选哪种?**

