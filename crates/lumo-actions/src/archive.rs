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
}
