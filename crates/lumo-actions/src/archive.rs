//! Archive actions — ZIP only (`archive.zip` / `archive.unzip`).
//!
//! ZIP is the single supported format (design 2026-05-30); tar.gz and friends
//! are deliberately out of scope. The `zip` crate is synchronous, so the actual
//! compress/extract runs inside `spawn_blocking`; capability checks and path
//! enumeration stay on the async side where `ctx` lives.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx, StepInterrupt};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
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

/// zip-bomb backstop (count axis): refuse archives with more than this many
/// entries unless the caller raises `max_entries`. Generous enough for legit
/// large archives (e.g. `node_modules`) but bounds an inode/handle bomb of many
/// tiny entries that never trips the byte cap.
const DEFAULT_MAX_ENTRIES: u64 = 1_000_000;

// ─── archive.zip ──────────────────────────────────────────────────────────────

pub struct ZipAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ZipIn {
    paths: Vec<String>,
    dest: String,
    #[serde(default)]
    base_dir: Option<String>,
    /// Optional WinZip AES-256 password. The password is never included in output.
    #[serde(default)]
    password: Option<String>,
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
            StepError::msg(format!(
                "archive.zip: path `{}` has no file name",
                p.display()
            ))
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
/// Cooperatively cancellable: the per-entry loop bails out with a clear error so
/// the blocking thread stops promptly instead of running to completion after the
/// VM has already cancelled the run **or timed out this step** (P0-2:句柄从
/// `CancelToken` 升级为 `StepInterrupt`,运行级取消 + 步级超时合一)。
fn write_zip(
    dest: &Path,
    entries: Vec<(PathBuf, String)>,
    password: Option<String>,
    interrupt: &StepInterrupt,
) -> Result<(u64, u64), StepError> {
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
        if interrupt.is_interrupted() {
            return Err(StepError::msg("archive.zip cancelled"));
        }
        let file_opts = match password.as_deref() {
            Some(password) => opts.with_aes_encryption(zip::AesMode::Aes256, password),
            None => opts,
        };
        zw.start_file(name.clone(), file_opts)
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
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ZipIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ZipIn {
            paths,
            dest,
            base_dir,
            password,
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

        let cancel = ctx.step_interrupt();
        let (count, bytes) =
            tokio::task::spawn_blocking(move || write_zip(&dest_path, entries, password, &cancel))
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnzipIn {
    src: String,
    dest: String,
    #[serde(default)]
    max_total_bytes: Option<u64>,
    #[serde(default)]
    max_entries: Option<u64>,
    /// Password for encrypted entries. Supplying it for a plain archive is harmless.
    #[serde(default)]
    password: Option<String>,
}

fn zip_entry<'a, R: Read + std::io::Seek>(
    archive: &'a mut zip::ZipArchive<R>,
    index: usize,
    password: Option<&str>,
) -> Result<zip::read::ZipFile<'a>, StepError> {
    match password {
        Some(password) => archive.by_index_decrypt(index, password.as_bytes()),
        None => archive.by_index(index),
    }
    .map_err(|e| {
        StepError::msg(format!(
            "archive.unzip entry {index}: decrypt/password error: {e}"
        ))
    })
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
/// Cooperatively cancellable: checks `interrupt` before each entry so the blocking
/// thread stops promptly on VM cancel **or step timeout** instead of extracting
/// the whole archive (P0-2:句柄从 `CancelToken` 升级为 `StepInterrupt`)。
fn extract_zip(
    src: &Path,
    dest: &Path,
    max_total: u64,
    password: Option<String>,
    interrupt: &StepInterrupt,
) -> Result<u64, StepError> {
    let file = std::fs::File::open(src)
        .map_err(|e| StepError::msg(format!("archive.unzip open {}: {e}", src.display())))?;
    let mut zr = zip::ZipArchive::new(file)
        .map_err(|e| StepError::msg(format!("archive.unzip read {}: {e}", src.display())))?;
    let mut total: u64 = 0;
    let mut count: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for i in 0..zr.len() {
        if interrupt.is_interrupted() {
            return Err(StepError::msg("archive.unzip cancelled"));
        }
        let mut entry = zip_entry(&mut zr, i, password.as_deref())?;
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
        let mut out = std::fs::File::create(&target).map_err(|e| {
            StepError::msg(format!("archive.unzip create {}: {e}", target.display()))
        })?;
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
            out.write_all(&buf[..n]).map_err(|e| {
                StepError::msg(format!("archive.unzip write {}: {e}", target.display()))
            })?;
        }
        count += 1;
    }
    Ok(count)
}

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
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<UnzipIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UnzipIn {
            src,
            dest,
            max_total_bytes,
            max_entries,
            password,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("archive.unzip input invalid: {e}")))?;
        let src_path = PathBuf::from(&src);
        let dest_path = PathBuf::from(&dest);
        ctx.ensure_fs_read(&src_path)?;
        let max_total = max_total_bytes.unwrap_or(DEFAULT_MAX_TOTAL_BYTES);
        let max_entries = max_entries.unwrap_or(DEFAULT_MAX_ENTRIES);

        // Pre-scan: validate every entry path (zip-slip) and cap-check writes
        // before extracting a single byte.
        {
            let file = std::fs::File::open(&src_path).map_err(|e| {
                StepError::msg(format!("archive.unzip open {}: {e}", src_path.display()))
            })?;
            let mut zr = zip::ZipArchive::new(file).map_err(|e| {
                StepError::msg(format!("archive.unzip read {}: {e}", src_path.display()))
            })?;
            // Count cap (mirrors the byte cap): bound entries once, cheaply,
            // before the pre-scan and extract loops touch any of them.
            if zr.len() as u64 > max_entries {
                return Err(StepError::msg(format!(
                    "archive.unzip: entry count {} exceeds max_entries {max_entries}",
                    zr.len()
                )));
            }
            for i in 0..zr.len() {
                let entry = zip_entry(&mut zr, i, password.as_deref())?;
                let name = entry.name().to_string();
                // Cap-check EVERY target (files and directories): a dir-only
                // archive must not create trees outside the fs_write grant.
                let target = safe_join(&dest_path, &name)?;
                ctx.ensure_fs_write(&target)?;
            }
        }

        let dp = dest_path.clone();
        let cancel = ctx.step_interrupt();
        let extract_password = password.clone();
        let count = tokio::task::spawn_blocking(move || {
            extract_zip(&src_path, &dp, max_total, extract_password, &cancel)
        })
        .await
        .map_err(|e| StepError::msg(format!("archive.unzip task: {e}")))??;

        Ok(ActionResult::from(serde_json::json!({
            "dest": dest,
            "entries": count,
        })))
    }
}
