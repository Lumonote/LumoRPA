//! File system actions.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::cmp::Ordering;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn register(r: &mut ActionRegistry) {
    r.register(ReadAction);
    r.register(WriteAction);
    r.register(ExistsAction);
    r.register(ListAction);
    r.register(MkdirAction);
    r.register(CopyAction);
    r.register(MoveAction);
    r.register(RenameAction);
    r.register(DeleteAction);
    r.register(MetadataAction);
    r.register(AppendAction);
}

pub struct ReadAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadIn {
    path: PathBuf,
}

#[async_trait]
impl Action for ReadAction {
    fn id(&self) -> &'static str {
        "file.read"
    }
    fn summary(&self) -> &'static str {
        "Read a text file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ReadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ReadIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.read input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| StepError::msg(format!("read {}: {e}", path.display())))?;
        Ok(ActionResult::from(Value::String(content)))
    }
}

pub struct WriteAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteIn {
    path: PathBuf,
    content: String,
    #[serde(default)]
    append: bool,
}

#[async_trait]
impl Action for WriteAction {
    fn id(&self) -> &'static str {
        "file.write"
    }
    fn summary(&self) -> &'static str {
        "Write a text file (create or overwrite)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WriteIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WriteIn {
            path,
            content,
            append,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.write input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if append {
            use tokio::io::AsyncWriteExt;
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| StepError::msg(format!("open append {}: {e}", path.display())))?;
            f.write_all(content.as_bytes())
                .await
                .map_err(|e| StepError::msg(format!("write {}: {e}", path.display())))?;
        } else {
            tokio::fs::write(&path, content.as_bytes())
                .await
                .map_err(|e| StepError::msg(format!("write {}: {e}", path.display())))?;
        }
        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

pub struct ExistsAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExistsIn {
    path: PathBuf,
}

#[async_trait]
impl Action for ExistsAction {
    fn id(&self) -> &'static str {
        "file.exists"
    }
    fn summary(&self) -> &'static str {
        "Test whether a path exists"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ExistsIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExistsIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.exists input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        Ok(ActionResult::from(Value::Bool(
            tokio::fs::try_exists(&path).await.unwrap_or(false),
        )))
    }
}

pub struct ListAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListIn {
    path: PathBuf,
    #[serde(default)]
    recursive: bool,
    #[serde(default = "default_list_kind")]
    kind: ListKind,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default = "default_list_sort_by")]
    sort_by: ListSortBy,
    #[serde(default)]
    descending: bool,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ListKind {
    All,
    Files,
    Directories,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ListSortBy {
    Path,
    Name,
    Type,
    Len,
    Modified,
}

#[async_trait]
impl Action for ListAction {
    fn id(&self) -> &'static str {
        "file.list"
    }
    fn summary(&self) -> &'static str {
        "List directory entries; optional recursive traversal"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ListIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ListIn {
            path,
            recursive,
            kind,
            pattern,
            include_hidden,
            sort_by,
            descending,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.list input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let entries = list_entries(
            ctx,
            &path,
            recursive,
            kind,
            pattern.as_deref(),
            include_hidden,
            sort_by,
            descending,
        )
        .await?;
        let count = entries.len();
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "count": count,
            "entries": entries,
        })))
    }
}

pub struct MkdirAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MkdirIn {
    path: PathBuf,
    #[serde(default = "default_true")]
    recursive: bool,
}

#[async_trait]
impl Action for MkdirAction {
    fn id(&self) -> &'static str {
        "file.mkdir"
    }
    fn summary(&self) -> &'static str {
        "Create a directory"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MkdirIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MkdirIn { path, recursive } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.mkdir input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        if recursive {
            tokio::fs::create_dir_all(&path)
                .await
                .map_err(|e| StepError::msg(format!("file.mkdir {}: {e}", path.display())))?;
        } else {
            tokio::fs::create_dir(&path)
                .await
                .map_err(|e| StepError::msg(format!("file.mkdir {}: {e}", path.display())))?;
        }
        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

pub struct CopyAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CopyIn {
    from: PathBuf,
    to: PathBuf,
    #[serde(default)]
    overwrite: bool,
}

#[async_trait]
impl Action for CopyAction {
    fn id(&self) -> &'static str {
        "file.copy"
    }
    fn summary(&self) -> &'static str {
        "Copy a file"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<CopyIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CopyIn {
            from,
            to,
            overwrite,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.copy input invalid: {e}")))?;
        ctx.ensure_fs_read(&from)?;
        ctx.ensure_fs_write(&to)?;
        let meta = tokio::fs::symlink_metadata(&from)
            .await
            .map_err(|e| StepError::msg(format!("file.copy stat {}: {e}", from.display())))?;
        if meta.is_dir() {
            return Err(StepError::msg(
                "file.copy supports files only; use archive.zip/archive.unzip for directories",
            ));
        }
        if meta.file_type().is_symlink() {
            return Err(StepError::msg(
                "file.copy refuses to follow symlink sources",
            ));
        }
        prepare_destination(ctx, &to, overwrite, "file.copy", false).await?;
        let bytes = tokio::fs::copy(&from, &to).await.map_err(|e| {
            StepError::msg(format!(
                "file.copy {} -> {}: {e}",
                from.display(),
                to.display()
            ))
        })?;
        Ok(ActionResult::from(serde_json::json!({
            "from": from,
            "to": to,
            "bytes": bytes,
        })))
    }
}

pub struct MoveAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveIn {
    from: PathBuf,
    to: PathBuf,
    #[serde(default)]
    overwrite: bool,
}

#[async_trait]
impl Action for MoveAction {
    fn id(&self) -> &'static str {
        "file.move"
    }
    fn summary(&self) -> &'static str {
        "Move or rename a file or directory"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MoveIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MoveIn {
            from,
            to,
            overwrite,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.move input invalid: {e}")))?;
        ensure_tree(ctx, &from, true, true, "file.move").await?;
        ctx.ensure_fs_write(&to)?;
        prepare_destination(ctx, &to, overwrite, "file.move", true).await?;
        tokio::fs::rename(&from, &to).await.map_err(|e| {
            StepError::msg(format!(
                "file.move {} -> {}: {e}",
                from.display(),
                to.display()
            ))
        })?;
        Ok(ActionResult::from(serde_json::json!({
            "from": from,
            "to": to,
        })))
    }
}

pub struct RenameAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameIn {
    path: PathBuf,
    new_name: String,
    #[serde(default)]
    overwrite: bool,
}

#[async_trait]
impl Action for RenameAction {
    fn id(&self) -> &'static str {
        "file.rename"
    }
    fn summary(&self) -> &'static str {
        "Rename a file or directory within its current parent"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<RenameIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let RenameIn {
            path,
            new_name,
            overwrite,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.rename input invalid: {e}")))?;
        let target_name = clean_file_name(&new_name)?;
        ensure_tree(ctx, &path, true, true, "file.rename").await?;
        let parent = path.parent().unwrap_or_else(|| Path::new(""));
        let to = parent.join(target_name);
        ctx.ensure_fs_write(&to)?;
        prepare_destination(ctx, &to, overwrite, "file.rename", true).await?;
        tokio::fs::rename(&path, &to).await.map_err(|e| {
            StepError::msg(format!(
                "file.rename {} -> {}: {e}",
                path.display(),
                to.display()
            ))
        })?;
        Ok(ActionResult::from(serde_json::json!({
            "from": path,
            "to": to,
        })))
    }
}

pub struct DeleteAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteIn {
    path: PathBuf,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    missing_ok: bool,
}

#[async_trait]
impl Action for DeleteAction {
    fn id(&self) -> &'static str {
        "file.delete"
    }
    fn summary(&self) -> &'static str {
        "Delete a file, symlink, or directory"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<DeleteIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DeleteIn {
            path,
            recursive,
            missing_ok,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.delete input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => {
                if recursive && meta.is_dir() && !meta.file_type().is_symlink() {
                    ensure_tree(ctx, &path, false, true, "file.delete").await?;
                }
                remove_path(&path, recursive, "file.delete").await?;
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "deleted": true,
                })))
            }
            Err(e) if e.kind() == ErrorKind::NotFound && missing_ok => {
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "deleted": false,
                })))
            }
            Err(e) => Err(StepError::msg(format!(
                "file.delete stat {}: {e}",
                path.display()
            ))),
        }
    }
}

pub struct MetadataAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MetadataIn {
    path: PathBuf,
}

#[async_trait]
impl Action for MetadataAction {
    fn id(&self) -> &'static str {
        "file.metadata"
    }
    fn summary(&self) -> &'static str {
        "Read file metadata without following symlinks"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MetadataIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MetadataIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.metadata input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let meta = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|e| StepError::msg(format!("file.metadata {}: {e}", path.display())))?;
        Ok(ActionResult::from(metadata_value(path, meta)))
    }
}

pub struct AppendAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AppendIn {
    path: PathBuf,
    content: String,
    /// Append a trailing newline after `content`.
    #[serde(default)]
    newline: bool,
}

#[async_trait]
impl Action for AppendAction {
    fn id(&self) -> &'static str {
        "file.append"
    }
    fn summary(&self) -> &'static str {
        "Append text to a file (create if missing), optionally adding a newline"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<AppendIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let AppendIn {
            path,
            content,
            newline,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.append input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        use tokio::io::AsyncWriteExt;
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| StepError::msg(format!("file.append open {}: {e}", path.display())))?;
        f.write_all(content.as_bytes())
            .await
            .map_err(|e| StepError::msg(format!("file.append write {}: {e}", path.display())))?;
        if newline {
            f.write_all(b"\n")
                .await
                .map_err(|e| StepError::msg(format!("file.append write {}: {e}", path.display())))?;
        }
        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

fn default_true() -> bool {
    true
}

fn default_list_kind() -> ListKind {
    ListKind::All
}

fn default_list_sort_by() -> ListSortBy {
    ListSortBy::Path
}

#[allow(clippy::too_many_arguments)]
async fn list_entries(
    ctx: &StepCtx,
    root: &Path,
    recursive: bool,
    kind: ListKind,
    pattern: Option<&str>,
    include_hidden: bool,
    sort_by: ListSortBy,
    descending: bool,
) -> Result<Vec<Value>, StepError> {
    let root_meta = tokio::fs::symlink_metadata(root)
        .await
        .map_err(|e| StepError::msg(format!("file.list stat {}: {e}", root.display())))?;
    if root_meta.file_type().is_symlink() {
        return Err(StepError::msg("file.list refuses to list a symlink path"));
    }
    if !root_meta.is_dir() {
        return Err(StepError::msg(format!(
            "file.list path is not a directory: {}",
            root.display()
        )));
    }

    let mut dirs = vec![root.to_path_buf()];
    let mut entries = Vec::new();
    while let Some(dir) = dirs.pop() {
        ctx.ensure_fs_read(&dir)?;
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| StepError::msg(format!("file.list read_dir {}: {e}", dir.display())))?;
        while let Some(ent) = rd
            .next_entry()
            .await
            .map_err(|e| StepError::msg(format!("file.list dir entry {}: {e}", dir.display())))?
        {
            let path = ent.path();
            ctx.ensure_fs_read(&path)?;
            let meta = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|e| StepError::msg(format!("file.list stat {}: {e}", path.display())))?;
            if recursive && meta.is_dir() && !meta.file_type().is_symlink() {
                dirs.push(path.clone());
            }
            if include_hidden || !is_hidden(&path) {
                let name = file_name(&path);
                if matches_kind(&meta, kind) && pattern_matches(pattern, &name) {
                    entries.push(metadata_value_with_name(path, name, meta));
                }
            }
        }
    }
    entries.sort_by(|a, b| compare_entries(a, b, sort_by));
    if descending {
        entries.reverse();
    }
    Ok(entries)
}

async fn prepare_destination(
    ctx: &StepCtx,
    path: &Path,
    overwrite: bool,
    action: &str,
    allow_directory_overwrite: bool,
) -> Result<(), StepError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(_) if !overwrite => Err(StepError::msg(format!(
            "{action}: destination exists: {}",
            path.display()
        ))),
        Ok(meta) => {
            if meta.is_dir() && !meta.file_type().is_symlink() {
                if !allow_directory_overwrite {
                    return Err(StepError::msg(format!(
                        "{action}: destination is a directory: {}",
                        path.display()
                    )));
                }
                ensure_tree(ctx, path, false, true, action).await?;
            }
            remove_path(path, true, action).await
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        StepError::msg(format!("{action} mkdir {}: {e}", parent.display()))
                    })?;
                }
            }
            Ok(())
        }
        Err(e) => Err(StepError::msg(format!(
            "{action} stat {}: {e}",
            path.display()
        ))),
    }
}

async fn remove_path(path: &Path, recursive: bool, action: &str) -> Result<(), StepError> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| StepError::msg(format!("{action} stat {}: {e}", path.display())))?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        if recursive {
            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                StepError::msg(format!("{action} remove_dir_all {}: {e}", path.display()))
            })
        } else {
            tokio::fs::remove_dir(path)
                .await
                .map_err(|e| StepError::msg(format!("{action} remove_dir {}: {e}", path.display())))
        }
    } else {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| StepError::msg(format!("{action} remove_file {}: {e}", path.display())))
    }
}

async fn ensure_tree(
    ctx: &StepCtx,
    root: &Path,
    need_read: bool,
    need_write: bool,
    action: &str,
) -> Result<(), StepError> {
    if need_read {
        ctx.ensure_fs_read(root)?;
    }
    if need_write {
        ctx.ensure_fs_write(root)?;
    }
    let root_meta = tokio::fs::symlink_metadata(root)
        .await
        .map_err(|e| StepError::msg(format!("{action} stat {}: {e}", root.display())))?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Ok(());
    }

    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| StepError::msg(format!("{action} read_dir {}: {e}", dir.display())))?;
        while let Some(ent) = rd
            .next_entry()
            .await
            .map_err(|e| StepError::msg(format!("{action} dir entry {}: {e}", dir.display())))?
        {
            let path = ent.path();
            if need_read {
                ctx.ensure_fs_read(&path)?;
            }
            if need_write {
                ctx.ensure_fs_write(&path)?;
            }
            let meta = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|e| StepError::msg(format!("{action} stat {}: {e}", path.display())))?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                dirs.push(path);
            }
        }
    }
    Ok(())
}

fn metadata_value(path: PathBuf, meta: std::fs::Metadata) -> Value {
    let name = file_name(&path);
    metadata_value_with_name(path, name, meta)
}

fn metadata_value_with_name(path: PathBuf, name: String, meta: std::fs::Metadata) -> Value {
    serde_json::json!({
        "path": path,
        "name": name,
        "type": file_type(&meta),
        "len": meta.len(),
        "readonly": meta.permissions().readonly(),
        "modified_ms": time_ms(meta.modified()),
        "created_ms": time_ms(meta.created()),
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn is_hidden(path: &Path) -> bool {
    file_name(path).starts_with('.')
}

fn matches_kind(meta: &std::fs::Metadata, kind: ListKind) -> bool {
    match kind {
        ListKind::All => true,
        ListKind::Files => meta.is_file() || meta.file_type().is_symlink(),
        ListKind::Directories => meta.is_dir() && !meta.file_type().is_symlink(),
    }
}

fn pattern_matches(pattern: Option<&str>, name: &str) -> bool {
    pattern
        .filter(|p| !p.trim().is_empty())
        .is_none_or(|p| wildcard_match(p, name))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    fn inner(p: &[char], v: &[char]) -> bool {
        match (p.split_first(), v.split_first()) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some((&'*', rest)), _) => inner(rest, v) || (!v.is_empty() && inner(p, &v[1..])),
            (Some((&'?', rest)), Some((_c, tail))) => inner(rest, tail),
            (Some((&pc, rest)), Some((vc, tail))) if pc == *vc => inner(rest, tail),
            _ => false,
        }
    }
    inner(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
    )
}

fn compare_entries(a: &Value, b: &Value, sort_by: ListSortBy) -> Ordering {
    match sort_by {
        ListSortBy::Path => cmp_str(a, b, "path"),
        ListSortBy::Name => cmp_str(a, b, "name"),
        ListSortBy::Type => cmp_str(a, b, "type").then_with(|| cmp_str(a, b, "path")),
        ListSortBy::Len => cmp_u64(a, b, "len").then_with(|| cmp_str(a, b, "path")),
        ListSortBy::Modified => cmp_u64(a, b, "modified_ms").then_with(|| cmp_str(a, b, "path")),
    }
}

fn cmp_str(a: &Value, b: &Value, key: &str) -> Ordering {
    a.get(key)
        .and_then(Value::as_str)
        .cmp(&b.get(key).and_then(Value::as_str))
}

fn cmp_u64(a: &Value, b: &Value, key: &str) -> Ordering {
    a.get(key)
        .and_then(Value::as_u64)
        .cmp(&b.get(key).and_then(Value::as_u64))
}

fn clean_file_name(name: &str) -> Result<&str, StepError> {
    let path = Path::new(name);
    let mut comps = path.components();
    let Some(first) = comps.next() else {
        return Err(StepError::msg(
            "file.rename requires a non-empty `new_name`",
        ));
    };
    if comps.next().is_some() || !matches!(first, std::path::Component::Normal(_)) {
        return Err(StepError::msg(
            "file.rename `new_name` must be a single file name, not a path",
        ));
    }
    Ok(name)
}

fn file_type(meta: &std::fs::Metadata) -> &'static str {
    let ft = meta.file_type();
    if ft.is_symlink() {
        "symlink"
    } else if meta.is_dir() {
        "directory"
    } else if meta.is_file() {
        "file"
    } else {
        "other"
    }
}

fn time_ms(time: std::io::Result<SystemTime>) -> Option<u64> {
    time.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
}
