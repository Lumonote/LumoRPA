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
    r.register(WaitAction);

    // ---- 兼容族:dir.* 目录操作(复用 file.* 内部机制) ----
    r.register(DirIfExistAction);
    r.register(DirMakedirAction);
    r.register(DirRemoveAction);
    r.register(DirFindFilesAction);
    r.register(DirFindSubdirsAction);
    r.register(DirGetSpecialDirAction);
    r.register(DirSelectedAction);
    r.register(DirRenameAction);
    r.register(DirMoveAction);
    r.register(DirCopyAction);
    r.register(DirEmptyAction);
    r.register(DirOpenAction);
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
            .map_err(|e| StepError::io(format!("read {}: {e}", path.display())))?;
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
                .map_err(|e| StepError::io(format!("open append {}: {e}", path.display())))?;
            f.write_all(content.as_bytes())
                .await
                .map_err(|e| StepError::io(format!("write {}: {e}", path.display())))?;
        } else {
            tokio::fs::write(&path, content.as_bytes())
                .await
                .map_err(|e| StepError::io(format!("write {}: {e}", path.display())))?;
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
    #[serde(default = "crate::contracts::default_collection_limit")]
    limit: usize,
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
            limit,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.list input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("file.list", limit)?;
        ctx.ensure_fs_read(&path)?;
        let mut entries = list_entries(
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
        let truncated = crate::contracts::truncate_with_flag(&mut entries, limit);
        let count = entries.len();
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "count": count,
            "limit": limit,
            "truncated": truncated,
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
                .map_err(|e| StepError::io(format!("file.mkdir {}: {e}", path.display())))?;
        } else {
            tokio::fs::create_dir(&path)
                .await
                .map_err(|e| StepError::io(format!("file.mkdir {}: {e}", path.display())))?;
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
            .map_err(|e| StepError::io(format!("file.copy stat {}: {e}", from.display())))?;
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
            StepError::io(format!(
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
            StepError::io(format!(
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
            StepError::io(format!(
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
    #[serde(default)]
    dry_run: bool,
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
            dry_run,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.delete input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => {
                if recursive && meta.is_dir() && !meta.file_type().is_symlink() {
                    ensure_tree(ctx, &path, false, true, "file.delete").await?;
                }
                if !dry_run {
                    remove_path(&path, recursive, "file.delete").await?;
                }
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "dry_run": dry_run,
                    "would_delete": true,
                    "deleted": !dry_run,
                })))
            }
            Err(e) if e.kind() == ErrorKind::NotFound && missing_ok => {
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "dry_run": dry_run,
                    "would_delete": false,
                    "deleted": false,
                })))
            }
            Err(e) => Err(StepError::io(format!(
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
            .map_err(|e| StepError::io(format!("file.metadata {}: {e}", path.display())))?;
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
            .map_err(|e| StepError::io(format!("file.append open {}: {e}", path.display())))?;
        f.write_all(content.as_bytes())
            .await
            .map_err(|e| StepError::io(format!("file.append write {}: {e}", path.display())))?;
        if newline {
            f.write_all(b"\n")
                .await
                .map_err(|e| StepError::io(format!("file.append write {}: {e}", path.display())))?;
        }
        Ok(ActionResult::from(serde_json::json!({ "path": path })))
    }
}

pub struct WaitAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitIn {
    path: PathBuf,
    /// 总超时(毫秒),超过即报错。默认 60_000。
    #[serde(default = "default_wait_timeout_ms")]
    timeout_ms: u64,
    /// 轮询间隔(毫秒)。默认 500。
    #[serde(default = "default_wait_poll_ms")]
    poll_ms: u64,
    /// 稳定窗口(毫秒):文件存在且 size 在该时长内不变才算就绪。这是等下载场景
    /// 的关键 —— 浏览器先落 `.crdownload`/部分写入,size 持续变化时不应提前返回。
    /// 设为 0 表示文件一出现立即返回。默认 1000。
    #[serde(default = "default_wait_stable_ms")]
    stable_ms: u64,
    /// 为 true 时要求文件是**调用之后**新出现/新替换的:调用时记录起始存在性与
    /// mtime,若文件当时已存在,则其 mtime 必须发生变化才视为候选。默认 false。
    #[serde(default)]
    must_exist_new: bool,
}

fn default_wait_timeout_ms() -> u64 {
    60_000
}
fn default_wait_poll_ms() -> u64 {
    500
}
fn default_wait_stable_ms() -> u64 {
    1_000
}

#[async_trait]
impl Action for WaitAction {
    fn id(&self) -> &'static str {
        "file.wait"
    }
    fn summary(&self) -> &'static str {
        "Wait until a file exists and its size is stable (download-completion friendly)"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WaitIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WaitIn {
            path,
            timeout_ms,
            poll_ms,
            stable_ms,
            must_exist_new,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("file.wait input invalid: {e}")))?;
        // 只探测 metadata,不写盘,因此仅需 fs.read 能力。
        ctx.ensure_fs_read(&path)?;

        // 取舍说明:这里用 tokio::time 轮询而非 notify 文件系统事件(虽然 workspace
        // 已有 notify 依赖)。RPA 等下载场景下,轮询 + size 稳定窗口已经足够,且在
        // macOS/Windows/Linux 上行为完全一致;notify 的事件语义跨平台差异大(改名、
        // 截断、临时文件落盘的事件序列各不相同),反而难以可靠判定"写完了"。
        let started = tokio::time::Instant::now();
        let poll = std::time::Duration::from_millis(poll_ms.max(1));
        let stable = std::time::Duration::from_millis(stable_ms);

        // must_exist_new:记录调用时刻的存在性与 mtime,作为"新文件"的判定基线。
        let initial_mtime: Option<SystemTime> =
            match tokio::fs::metadata(&path).await {
                Ok(m) => Some(m.modified().map_err(|e| {
                    StepError::io(format!("file.wait mtime {}: {e}", path.display()))
                })?),
                Err(_) => None,
            };

        // size 稳定性追踪:记录最近一次观测到的 size 以及它首次出现的时刻。
        let mut last_size: Option<u64> = None;
        let mut size_since = started;
        // 最后一次观测到的状态,超时报错时给出明确原因。每轮探测都会先赋值再读。
        let mut last_state;

        loop {
            match tokio::fs::metadata(&path).await {
                Ok(meta) => {
                    // must_exist_new:调用前就存在且 mtime 没变 → 还是旧文件,不算。
                    let is_new = !must_exist_new
                        || match initial_mtime {
                            None => true, // 调用时不存在,现在出现的必然是新文件
                            Some(m0) => meta.modified().ok().is_some_and(|m| m != m0),
                        };
                    if is_new {
                        let size = meta.len();
                        let now = tokio::time::Instant::now();
                        if last_size != Some(size) {
                            // size 变化(或首次观测):重置稳定窗口起点。
                            last_size = Some(size);
                            size_since = now;
                        }
                        if now.duration_since(size_since) >= stable {
                            let waited_ms = started.elapsed().as_millis() as u64;
                            return Ok(ActionResult::from(serde_json::json!({
                                "path": path,
                                "size": size,
                                "waited_ms": waited_ms,
                            })));
                        }
                        last_state = "still_changing";
                    } else {
                        last_state = "exists_but_not_new";
                    }
                }
                Err(_) => {
                    // 文件消失/尚未出现:清空稳定性追踪,重新计窗。
                    last_size = None;
                    last_state = "not_found";
                }
            }
            let elapsed = started.elapsed();
            if elapsed.as_millis() as u64 >= timeout_ms {
                // typed 错误约定:动作内超时用 `StepError::Timeout`,让
                // `retry.on: [timeout]` 能捕获。诊断细节记进 run 日志。
                tracing::warn!(
                    "file.wait timeout after {}ms waiting for {} (last state: {})",
                    elapsed.as_millis(),
                    path.display(),
                    match last_state {
                        "not_found" => "file does not exist".to_string(),
                        "still_changing" => format!(
                            "file exists but size still changing within stable_ms={stable_ms}"
                        ),
                        _ => "file pre-existed and was not replaced (must_exist_new=true)"
                            .to_string(),
                    }
                );
                return Err(StepError::Timeout { ms: timeout_ms });
            }
            tokio::time::sleep(poll).await;
        }
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
        .map_err(|e| StepError::io(format!("file.list stat {}: {e}", root.display())))?;
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
            .map_err(|e| StepError::io(format!("file.list read_dir {}: {e}", dir.display())))?;
        while let Some(ent) = rd
            .next_entry()
            .await
            .map_err(|e| StepError::io(format!("file.list dir entry {}: {e}", dir.display())))?
        {
            let path = ent.path();
            ctx.ensure_fs_read(&path)?;
            let meta = tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|e| StepError::io(format!("file.list stat {}: {e}", path.display())))?;
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
                        StepError::io(format!("{action} mkdir {}: {e}", parent.display()))
                    })?;
                }
            }
            Ok(())
        }
        Err(e) => Err(StepError::io(format!(
            "{action} stat {}: {e}",
            path.display()
        ))),
    }
}

async fn remove_path(path: &Path, recursive: bool, action: &str) -> Result<(), StepError> {
    let meta = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|e| StepError::io(format!("{action} stat {}: {e}", path.display())))?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        if recursive {
            tokio::fs::remove_dir_all(path).await.map_err(|e| {
                StepError::io(format!("{action} remove_dir_all {}: {e}", path.display()))
            })
        } else {
            tokio::fs::remove_dir(path)
                .await
                .map_err(|e| StepError::io(format!("{action} remove_dir {}: {e}", path.display())))
        }
    } else {
        tokio::fs::remove_file(path)
            .await
            .map_err(|e| StepError::io(format!("{action} remove_file {}: {e}", path.display())))
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
        .map_err(|e| StepError::io(format!("{action} stat {}: {e}", root.display())))?;
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Ok(());
    }

    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let mut rd = tokio::fs::read_dir(&dir)
            .await
            .map_err(|e| StepError::io(format!("{action} read_dir {}: {e}", dir.display())))?;
        while let Some(ent) = rd
            .next_entry()
            .await
            .map_err(|e| StepError::io(format!("{action} dir entry {}: {e}", dir.display())))?
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
                .map_err(|e| StepError::io(format!("{action} stat {}: {e}", path.display())))?;
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
// ---------------------------------------------------------------------------
// 兼容族:dir.* 目录操作
// ---------------------------------------------------------------------------
//
// 把「文件」与「文件夹」拆成两个指令分类,本族是后者。实现**全部复用**上面
// 已有的 file.* 内部机制(ensure_fs_read / ensure_fs_write / list_entries /
// ensure_tree / remove_path),不重复造文件系统逻辑 —— 保证目录与文件两条路径
// 的安全语义(符号链接拒绝、dry_run、缺失容错)完全一致。

/// 目录是否存在(等同于 file.exists 的目录语义门面)。
pub struct DirIfExistAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirIfExistIn {
    path: PathBuf,
}
#[async_trait]
impl Action for DirIfExistAction {
    fn id(&self) -> &'static str {
        "dir.if_exist"
    }
    fn summary(&self) -> &'static str {
        "Whether a directory exists (false for files and missing paths)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirIfExistIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirIfExistIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.if_exist input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        // 用 symlink_metadata:符号链接本身不算目录,避免顺着链接越出沙箱判定。
        let is_dir = tokio::fs::symlink_metadata(&path)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false);
        Ok(ActionResult::from(Value::Bool(is_dir)))
    }
}

/// 递归创建目录(等同 mkdir -p)。
pub struct DirMakedirAction;
#[async_trait]
impl Action for DirMakedirAction {
    fn id(&self) -> &'static str {
        "dir.makedir"
    }
    fn summary(&self) -> &'static str {
        "Create a directory and any missing parents (mkdir -p)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirIfExistIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirIfExistIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.makedir input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        tokio::fs::create_dir_all(&path)
            .await
            .map_err(|e| StepError::io(format!("dir.makedir {}: {e}", path.display())))?;
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "created": true,
        })))
    }
}

/// 删除目录。
pub struct DirRemoveAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirRemoveIn {
    path: PathBuf,
    /// Delete a non-empty directory recursively.
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    missing_ok: bool,
    #[serde(default)]
    dry_run: bool,
}
#[async_trait]
impl Action for DirRemoveAction {
    fn id(&self) -> &'static str {
        "dir.remove"
    }
    fn summary(&self) -> &'static str {
        "Remove a directory; `recursive` is required for a non-empty one"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirRemoveIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirRemoveIn {
            path,
            recursive,
            missing_ok,
            dry_run,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.remove input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) => {
                if !meta.is_dir() {
                    return Err(StepError::msg(format!(
                        "dir.remove path is not a directory: {} (use file.delete for files)",
                        path.display()
                    )));
                }
                if recursive {
                    ensure_tree(ctx, &path, false, true, "dir.remove").await?;
                }
                if !dry_run {
                    remove_path(&path, recursive, "dir.remove").await?;
                }
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "dry_run": dry_run,
                    "would_delete": true,
                    "deleted": !dry_run,
                })))
            }
            Err(e) if e.kind() == ErrorKind::NotFound && missing_ok => {
                Ok(ActionResult::from(serde_json::json!({
                    "path": path,
                    "dry_run": dry_run,
                    "would_delete": false,
                    "deleted": false,
                })))
            }
            Err(e) => Err(StepError::io(format!(
                "dir.remove stat {}: {e}",
                path.display()
            ))),
        }
    }
}

/// 查找目录下的文件。
pub struct DirFindFilesAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirFindIn {
    path: PathBuf,
    #[serde(default)]
    recursive: bool,
    /// Glob-ish substring filter on the file name.
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default = "crate::contracts::default_collection_limit")]
    limit: usize,
}
#[async_trait]
impl Action for DirFindFilesAction {
    fn id(&self) -> &'static str {
        "dir.find_files"
    }
    fn summary(&self) -> &'static str {
        "Find files under a directory (optional recursion and pattern)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirFindIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirFindIn {
            path,
            recursive,
            pattern,
            include_hidden,
            limit,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.find_files input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("dir.find_files", limit)?;
        ctx.ensure_fs_read(&path)?;
        let mut entries = list_entries(
            ctx,
            &path,
            recursive,
            ListKind::Files,
            pattern.as_deref(),
            include_hidden,
            ListSortBy::Name,
            false,
        )
        .await?;
        let truncated = crate::contracts::truncate_with_flag(&mut entries, limit);
        let count = entries.len();
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "count": count,
            "limit": limit,
            "truncated": truncated,
            "files": entries,
        })))
    }
}

/// 查找子目录。
pub struct DirFindSubdirsAction;
#[async_trait]
impl Action for DirFindSubdirsAction {
    fn id(&self) -> &'static str {
        "dir.find_subdirs"
    }
    fn summary(&self) -> &'static str {
        "Find sub-directories under a directory"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirFindIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirFindIn {
            path,
            recursive,
            pattern,
            include_hidden,
            limit,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.find_subdirs input invalid: {e}")))?;
        let limit = crate::contracts::checked_limit("dir.find_subdirs", limit)?;
        ctx.ensure_fs_read(&path)?;
        let mut entries = list_entries(
            ctx,
            &path,
            recursive,
            ListKind::Directories,
            pattern.as_deref(),
            include_hidden,
            ListSortBy::Name,
            false,
        )
        .await?;
        let truncated = crate::contracts::truncate_with_flag(&mut entries, limit);
        let count = entries.len();
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "count": count,
            "limit": limit,
            "truncated": truncated,
            "directories": entries,
        })))
    }
}

/// 用户主目录:`$HOME`,回退 `$USERPROFILE`(Windows 不设 `HOME`)。
///
/// 与本 crate 其它地方同一约定(见 `browser::browser_profiles_root`、
/// `selector_stats::default_path`):两个变量都要看,否则 Windows 上永远解析
/// 不到主目录——这正是 `dir.get_special_dir` 之前只在 Unix 可用的原因。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 取系统特殊目录(桌面/下载/文档/临时/主目录)。
pub struct DirGetSpecialDirAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirGetSpecialDirIn {
    /// One of: home, desktop, downloads, documents, temp. Defaults to home.
    #[serde(default)]
    name: Option<String>,
}
#[async_trait]
impl Action for DirGetSpecialDirAction {
    fn id(&self) -> &'static str {
        "dir.get_special_dir"
    }
    fn summary(&self) -> &'static str {
        "Resolve a well-known directory: home | desktop | downloads | documents | temp"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirGetSpecialDirIn>);
        &S
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirGetSpecialDirIn { name } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.get_special_dir input invalid: {e}")))?;
        let home = home_dir().ok_or_else(|| {
            StepError::msg("dir.get_special_dir: neither HOME nor USERPROFILE is set on this host")
        })?;
        let key = name.as_deref().unwrap_or("home");
        let path = match key {
            "home" => home.clone(),
            // Known-user folders are *not* plain subdirectories of the profile
            // on Windows: they are redirected through the shell (OneDrive, a
            // moved Documents dir, a localized name), and `%USERPROFILE%\Desktop`
            // is frequently absent or not the folder the user sees. `dirs::`
            // resolves them through the platform API, so use it where the
            // platform actually provides an answer and keep the join as the
            // fallback for hosts where it returns nothing.
            "desktop" => dirs::desktop_dir().unwrap_or_else(|| home.join("Desktop")),
            "downloads" => dirs::download_dir().unwrap_or_else(|| home.join("Downloads")),
            "documents" => dirs::document_dir().unwrap_or_else(|| home.join("Documents")),
            "temp" => std::env::temp_dir(),
            other => {
                return Err(StepError::msg(format!(
                    "dir.get_special_dir: unknown name `{other}` (expected home | desktop | downloads | documents | temp)"
                )))
            }
        };
        Ok(ActionResult::from(serde_json::json!({
            "name": key,
            "path": path,
        })))
    }
}

/// GUI 目录/文件选择器。
///
/// 走 `human.*` 的宿主回执通道(与本 crate 的 human.rs 同一机制)。**无宿主接入时
/// 必须明确报错而不是 panic/挂死** —— CLI `--no-store` 路径下没有 UI 线程,
/// 静默返回空列表会让流程拿着空路径继续跑,比直接失败更难排查。
pub struct DirSelectedAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirSelectedIn {
    /// `files` (default) | `directories`.
    #[serde(default)]
    kind: Option<String>,
    /// Allow selecting more than one entry.
    #[serde(default)]
    multiple: bool,
}
#[async_trait]
impl Action for DirSelectedAction {
    fn id(&self) -> &'static str {
        "dir.selected_dirs_or_files"
    }
    fn summary(&self) -> &'static str {
        "Ask the host UI to pick files or directories; errors without a host"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirSelectedIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirSelectedIn { kind, multiple } = serde_json::from_value(input).map_err(|e| {
            StepError::msg(format!("dir.selected_dirs_or_files input invalid: {e}"))
        })?;
        let kind = kind.as_deref().unwrap_or("files");
        if kind != "files" && kind != "directories" {
            return Err(StepError::msg(format!(
                "dir.selected_dirs_or_files: unknown kind `{kind}` (expected files | directories)"
            )));
        }
        // 复用 human.* 的宿主交互通道:同一个 ask_host,同一套无宿主错误语义。
        match crate::human::ask_host(
            ctx,
            "dir.selected_dirs_or_files",
            serde_json::json!({ "kind": kind, "multiple": multiple }),
        )
        .await
        {
            Ok(v) => Ok(ActionResult::from(v)),
            Err(e) => Err(e),
        }
    }
}

/// 重命名目录。
pub struct DirRenameAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirRenameIn {
    path: PathBuf,
    to: PathBuf,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    dry_run: bool,
}
#[async_trait]
impl Action for DirRenameAction {
    fn id(&self) -> &'static str {
        "dir.rename"
    }
    fn summary(&self) -> &'static str {
        "Rename a directory within its parent (same as dir.move)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirRenameIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirRenameIn {
            path,
            to,
            overwrite,
            dry_run,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.rename input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        ctx.ensure_fs_write(&to)?;
        if !dry_run {
            if !overwrite && tokio::fs::try_exists(&to).await.unwrap_or(false) {
                return Err(StepError::msg(format!(
                    "dir.rename target already exists: {} (pass `overwrite: true` to replace)",
                    to.display()
                )));
            }
            tokio::fs::rename(&path, &to).await.map_err(|e| {
                StepError::io(format!(
                    "dir.rename {} -> {}: {e}",
                    path.display(),
                    to.display()
                ))
            })?;
        }
        Ok(ActionResult::from(serde_json::json!({
            "from": path,
            "to": to,
            "dry_run": dry_run,
            "renamed": !dry_run,
        })))
    }
}

/// 移动目录(与重命名同实现,提供同名指令)。
pub struct DirMoveAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirMoveIn {
    path: PathBuf,
    to: PathBuf,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    dry_run: bool,
}
#[async_trait]
impl Action for DirMoveAction {
    fn id(&self) -> &'static str {
        "dir.move"
    }
    fn summary(&self) -> &'static str {
        "Move a directory to `to`"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirMoveIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirMoveIn {
            path,
            to,
            overwrite,
            dry_run,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.move input invalid: {e}")))?;
        ctx.ensure_fs_write(&path)?;
        ctx.ensure_fs_write(&to)?;
        if !dry_run {
            if !overwrite && tokio::fs::try_exists(&to).await.unwrap_or(false) {
                return Err(StepError::msg(format!(
                    "dir.move target already exists: {} (pass `overwrite: true` to replace)",
                    to.display()
                )));
            }
            tokio::fs::rename(&path, &to).await.map_err(|e| {
                StepError::io(format!(
                    "dir.move {} -> {}: {e}",
                    path.display(),
                    to.display()
                ))
            })?;
        }
        Ok(ActionResult::from(serde_json::json!({
            "from": path,
            "to": to,
            "dry_run": dry_run,
            "moved": !dry_run,
        })))
    }
}

/// 复制目录(递归)。
pub struct DirCopyAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirCopyIn {
    path: PathBuf,
    to: PathBuf,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    dry_run: bool,
}
#[async_trait]
impl Action for DirCopyAction {
    fn id(&self) -> &'static str {
        "dir.copy"
    }
    fn summary(&self) -> &'static str {
        "Recursively copy a directory tree"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirCopyIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirCopyIn {
            path,
            to,
            overwrite,
            dry_run,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.copy input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        ctx.ensure_fs_write(&to)?;
        // 只对**已存在**的目标做覆盖判定;源自动展开成 to/<basename> 时不算冲突。
        let dest_conflict = tokio::fs::try_exists(&to).await.unwrap_or(false);
        if dest_conflict && !overwrite {
            return Err(StepError::msg(format!(
                "dir.copy target already exists: {} (pass `overwrite: true` to merge/replace)",
                to.display()
            )));
        }
        let mut files = 0u64;
        let mut dirs = 0u64;
        if !dry_run {
            copy_tree(&path, &to, overwrite, &mut files, &mut dirs).await?;
        }
        Ok(ActionResult::from(serde_json::json!({
            "from": path,
            "to": to,
            "dry_run": dry_run,
            "overwrite": overwrite,
            "files": files,
            "directories": dirs,
        })))
    }
}

/// 递归复制目录树。
fn copy_tree<'a>(
    from: &'a Path,
    to: &'a Path,
    overwrite: bool,
    files: &'a mut u64,
    dirs: &'a mut u64,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), StepError>> + Send + 'a>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(to)
            .await
            .map_err(|e| StepError::io(format!("dir.copy mkdir {}: {e}", to.display())))?;
        *dirs += 1;
        let mut rd = tokio::fs::read_dir(from)
            .await
            .map_err(|e| StepError::io(format!("dir.copy read {}: {e}", from.display())))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| StepError::io(format!("dir.copy read entry: {e}")))?
        {
            let src = entry.path();
            let dst = to.join(entry.file_name());
            let meta = entry
                .metadata()
                .await
                .map_err(|e| StepError::io(format!("dir.copy stat {}: {e}", src.display())))?;
            if meta.is_dir() {
                copy_tree(&src, &dst, overwrite, files, dirs).await?;
            } else {
                if !overwrite && tokio::fs::try_exists(&dst).await.unwrap_or(false) {
                    return Err(StepError::msg(format!(
                        "dir.copy target already exists: {} (pass `overwrite: true`)",
                        dst.display()
                    )));
                }
                tokio::fs::copy(&src, &dst).await.map_err(|e| {
                    StepError::io(format!(
                        "dir.copy {} -> {}: {e}",
                        src.display(),
                        dst.display()
                    ))
                })?;
                *files += 1;
            }
        }
        Ok(())
    })
}

/// 判定目录是否为空。
pub struct DirEmptyAction;
#[async_trait]
impl Action for DirEmptyAction {
    fn id(&self) -> &'static str {
        "dir.empty"
    }
    fn summary(&self) -> &'static str {
        "Whether a directory holds no entries (missing directories count as empty)"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirIfExistIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirIfExistIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.empty input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        let mut rd = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Ok(ActionResult::from(Value::Bool(true)))
            }
            Err(e) => return Err(StepError::io(format!("dir.empty {}: {e}", path.display()))),
        };
        let next = rd
            .next_entry()
            .await
            .map_err(|e| StepError::io(format!("dir.empty {}: {e}", path.display())))?;
        Ok(ActionResult::from(Value::Bool(next.is_none())))
    }
}

/// 在系统文件管理器中打开目录。
///
/// 无 GUI 的宿主会明确报 unsupported,而不是假装成功 —— 里这条指令是
/// 「给人看」的副作用,静默 no-op 会让流程比失败更难排查。
pub struct DirOpenAction;
#[async_trait]
impl Action for DirOpenAction {
    fn id(&self) -> &'static str {
        "dir.open"
    }
    fn summary(&self) -> &'static str {
        "Reveal a directory in the host file manager; unsupported without a GUI host"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DirIfExistIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DirIfExistIn { path } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("dir.open input invalid: {e}")))?;
        ctx.ensure_fs_read(&path)?;
        crate::human::ask_host(
            ctx,
            "dir.open",
            serde_json::json!({ "path": path, "action": "reveal" }),
        )
        .await
        .map(ActionResult::from)
    }
}
