//! `lumo serve` — webhook HTTP server + cron scheduler (T-01 / T-04).
//!
//! Listens on `--bind` and dispatches `POST /webhook/<flow-name>` to flows
//! living in `--flows`. The body's JSON object becomes `inputs:`. Only flows
//! that declare a `webhook` trigger are accepted; everything else returns
//! `403 Forbidden`. An optional shared secret (`--token` / `LUMO_WEBHOOK_TOKEN`
//! env) gates access via the `X-Lumo-Token` header.
//!
//! At startup the same process also scans `--flows` for `cron` triggers; each
//! one gets its own background task that sleeps until the next scheduled time
//! and runs the flow. Runs are persisted to `$LUMO_HOME/lumo.db` for both
//! triggers so Studio's "运行历史" and `lumo runs list` see them.

use axum::{
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use clap::Args as ClapArgs;
use cron::Schedule;
use lumo_core::{CancelToken, RunOptions};
use lumo_storage::Repo;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower_http::trace::TraceLayer;

use super::build_action_registry;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Directory containing flow YAML files. Webhook URLs map flow names to
    /// `<flows>/<name>.lumoflow.yaml`.
    #[arg(long, default_value = "./flows")]
    pub flows: PathBuf,
    /// HTTP bind address. Default `127.0.0.1:8787` (localhost only — set
    /// `0.0.0.0:port` to accept LAN traffic).
    #[arg(long, default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,
    /// Optional shared secret. When set, requests must include
    /// `X-Lumo-Token: <value>`. Recommended for any non-localhost bind.
    #[arg(long, env = "LUMO_WEBHOOK_TOKEN")]
    pub token: Option<String>,
}

/// P1-1：运行中 webhook run 的取消表（run_id → CancelToken）。运行**前**登记
/// （run_id 由宿主预生成 —— 引擎迟生成的话，取消窗口开头有缝），结束后经
/// [`CancelGuard`] RAII 摘除；`POST /runs/:id/cancel` 查表触发协作取消。
type CancelMap = Arc<Mutex<HashMap<String, CancelToken>>>;

#[derive(Clone)]
struct AppState {
    flows_dir: PathBuf,
    home: PathBuf,
    token: Option<String>,
    cancels: CancelMap,
    /// P1-1：webhook 并发上限（`LUMO_SERVE_MAX_CONCURRENCY`，默认 8）。超额
    /// 请求在信号量上排队而不是被拒 —— 上游背压由 HTTP 客户端超时兜底。
    permits: Arc<Semaphore>,
}

impl AppState {
    fn new(
        flows_dir: PathBuf,
        home: PathBuf,
        token: Option<String>,
        max_concurrency: usize,
    ) -> Self {
        Self {
            flows_dir,
            home,
            token,
            cancels: Arc::new(Mutex::new(HashMap::new())),
            permits: Arc::new(Semaphore::new(max_concurrency)),
        }
    }
}

/// `LUMO_SERVE_MAX_CONCURRENCY` 解析：默认 8；解析失败或填 0 一律回退默认
/// （0 会永久堵死全部 webhook，按配置错误处理）。
fn serve_max_concurrency_from(raw: Option<&str>) -> usize {
    const DEFAULT: usize = 8;
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT)
}

fn serve_max_concurrency_from_env() -> usize {
    serve_max_concurrency_from(std::env::var("LUMO_SERVE_MAX_CONCURRENCY").ok().as_deref())
}

/// 登记/摘除 run_id → CancelToken 的 RAII 守卫：webhook handler 无论从哪条
/// 错误路径返回，表项都随 drop 摘除，让取消接口对「已结束」的 run 稳定返回
/// `ok=false`（幂等），也杜绝表项泄漏。
struct CancelGuard {
    cancels: CancelMap,
    run_id: String,
}

impl CancelGuard {
    fn register(cancels: &CancelMap, run_id: &str, token: CancelToken) -> Self {
        cancels.lock().insert(run_id.to_string(), token);
        Self {
            cancels: cancels.clone(),
            run_id: run_id.to_string(),
        }
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.cancels.lock().remove(&self.run_id);
    }
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    std::fs::create_dir_all(&home)?;
    if !args.flows.exists() {
        anyhow::bail!("--flows directory {} does not exist", args.flows.display());
    }
    let state = AppState::new(
        args.flows.clone(),
        home.clone(),
        args.token,
        serve_max_concurrency_from_env(),
    );
    // T-01: scan `--flows` for cron triggers and spawn one task per flow.
    // Errors are logged but never abort the server: a single bad cron string
    // shouldn't take the webhook lane down.
    let scheduled = scan_cron_triggers(&args.flows);
    if !scheduled.is_empty() {
        println!(
            "◉ cron scheduler  ·  {} flow(s) registered",
            scheduled.len()
        );
        for sf in &scheduled {
            println!("  · {} @ {}", sf.name, sf.schedule);
            tokio::spawn(schedule_loop(sf.clone(), home.clone()));
        }
    }
    // T-02: scan `--flows` for file-system triggers and spawn one watcher per flow.
    let watched = scan_file_triggers(&args.flows);
    if !watched.is_empty() {
        println!("◉ file watcher    ·  {} flow(s) registered", watched.len());
        for wf in &watched {
            println!(
                "  · {} ← {} [{}]",
                wf.name,
                wf.watch_path.display(),
                wf.events.join(",")
            );
            tokio::spawn(watch_loop(wf.clone(), home.clone()));
        }
    }
    // T-05: scan `--flows` for hotkey triggers and spawn one OS listener per flow.
    // Permissions (macOS Accessibility / Linux uinput) are surfaced via the
    // listener's `permission_status` — we log a warning when the listener
    // can't actually bind so the user sees why hotkeys aren't firing.
    let hotkeys = super::hotkey::scan_hotkey_triggers(&args.flows);
    if !hotkeys.is_empty() {
        let hub = super::hotkey::default_hub();
        let status = hub.permission_status();
        println!(
            "◉ hotkey listener ·  {} flow(s) registered  ·  {}",
            hotkeys.len(),
            match status {
                super::hotkey::PermissionStatus::Ready => "backend=ready",
                super::hotkey::PermissionStatus::NeedsAccessibility =>
                    "backend=needs-accessibility (grant in System Settings)",
                super::hotkey::PermissionStatus::Unsupported =>
                    "backend=unsupported (hotkeys disabled this session)",
            }
        );
        for hf in &hotkeys {
            println!("  · {} ⌨ {}", hf.name, hf.keys.label());
            let listener = hub.register(hf.keys.clone());
            tokio::spawn(super::hotkey::dispatch_loop(
                hf.clone(),
                home.clone(),
                listener,
            ));
        }
    }
    let app = build_app(state);
    let listener = TcpListener::bind(args.bind).await?;
    let bound = listener.local_addr()?;
    println!(
        "◉ lumo serve  ·  POST http://{}/webhook/<flow-name>  ·  flows={}{}",
        bound,
        args.flows.display(),
        if std::env::var("LUMO_WEBHOOK_TOKEN").is_ok() {
            "  ·  token=set"
        } else {
            ""
        }
    );
    axum::serve(listener, app).await?;
    Ok(())
}

fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/webhook/:flow_name", post(webhook))
        // P1-1：协作取消一个运行中的 webhook run（幂等，见 `cancel_run`）。
        .route("/runs/:run_id/cancel", post(cancel_run))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

/// 共享的 `X-Lumo-Token` 门禁：webhook 与取消路由同一把锁 —— 取消是改变
/// 运行状态的操作，不能比触发更宽松。
fn check_token(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    if let Some(expected) = &state.token {
        let provided = headers
            .get("x-lumo-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != expected {
            return Err((
                StatusCode::UNAUTHORIZED,
                "missing or invalid x-lumo-token".into(),
            ));
        }
    }
    Ok(())
}

/// P1-1：`POST /runs/:run_id/cancel` —— 协作取消一个运行中的 webhook run。
/// 幂等语义（对齐桌面端 human_respond 的迟到回执处理）：
///   * 表中有键（运行中）→ 触发取消，`ok=true`；重复调用仍 `true`
///     （CancelToken 自身幂等）；
///   * 无键（run 不存在 / 已结束）→ `ok=false`，HTTP 仍 200，不报错。
async fn cancel_run(
    State(state): State<AppState>,
    AxumPath(run_id): AxumPath<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, (StatusCode, String)> {
    check_token(&state, &headers)?;
    let token = state.cancels.lock().get(&run_id).cloned();
    let ok = match token {
        Some(t) => {
            t.cancel();
            true
        }
        None => false,
    };
    Ok(Json(serde_json::json!({ "ok": ok, "run_id": run_id })))
}

async fn webhook(
    State(state): State<AppState>,
    AxumPath(flow_name): AxumPath<String>,
    headers: HeaderMap,
    Json(inputs): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    check_token(&state, &headers)?;
    if !valid_flow_name(&flow_name) {
        return Err((StatusCode::BAD_REQUEST, "invalid flow name".into()));
    }
    let path = resolve_flow(&state.flows_dir, &flow_name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!(
                "flow `{flow_name}` not found in {}",
                state.flows_dir.display()
            ),
        )
    })?;
    let flow = lumo_dsl::parse_file(&path).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    lumo_dsl::validate(&flow).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    if !flow.spec.triggers.iter().any(|t| t.kind == "webhook") {
        return Err((
            StatusCode::FORBIDDEN,
            format!("flow `{flow_name}` does not declare a webhook trigger"),
        ));
    }
    let inputs = if inputs.is_object() {
        inputs
    } else if inputs.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "webhook body must be a JSON object (or empty/null)".into(),
        ));
    };
    // P1-1：并发门 —— 4xx 级校验都过了才排队拿许可，坏请求不占并发额度。
    // 许可持有到 handler 返回（含错误路径），排队等待可被客户端断连中止。
    let _permit = state.permits.clone().acquire_owned().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("concurrency gate: {e}"),
        )
    })?;
    let registry = build_action_registry(&state.home, Some(&path));
    let repo = Some(
        Repo::open(state.home.join("lumo.db"))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    );
    // P1-1：宿主预生成 run_id，先在取消表建键再启动（引擎迟生成的话，
    // `POST /runs/:id/cancel` 在运行初期查不到键）；RAII 守卫保证任何返回
    // 路径都摘除表项。
    let run_id = ulid::Ulid::new().to_string();
    let cancel = CancelToken::new();
    let _cancel_guard = CancelGuard::register(&state.cancels, &run_id, cancel.clone());
    // 架构 P1-1:统一走宿主组装(step_timeout/artifacts/cancel/vault/AI
    // hooks)。webhook 是 headless 宿主:不注入 prompter(human.* 显式报错)。
    let vm = super::host_vm(&state.home, &flow, registry, repo, cancel).with_run_id(Some(run_id));
    let report = vm
        .run(
            &flow,
            RunOptions {
                inputs,
                trigger_kind: "webhook".into(),
            },
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let body = serde_json::json!({
        "run_id": report.run_id,
        "success": report.success,
        "steps_total": report.steps_total,
        "steps_ok": report.steps_ok,
        "steps_failed": report.steps_failed,
        "duration_ms": report.duration_ms,
        "outputs": report.outputs,
    });
    if report.success {
        Ok(Json(body))
    } else {
        Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::to_string(&body).unwrap_or_else(|_| "{}".into()),
        ))
    }
}

fn valid_flow_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name.len() < 256
}

fn resolve_flow(dir: &Path, name: &str) -> Option<PathBuf> {
    for ext in ["lumoflow.yaml", "lumoflow.yml"] {
        let p = dir.join(format!("{name}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

// ─── T-01: cron scheduler ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct ScheduledFlow {
    /// Display name (file stem) used in startup banner + logs.
    name: String,
    /// Path on disk; re-parsed on every fire so an edited flow updates without
    /// restarting the server.
    path: PathBuf,
    /// Raw schedule string from the trigger spec (kept for the banner).
    schedule: String,
    /// Pre-parsed schedule the loop consumes.
    parsed: Schedule,
}

/// Scan `--flows` for flows that declare a `cron` trigger and return one
/// `ScheduledFlow` per (flow × cron trigger). Flows that fail to parse or
/// validate are surfaced to stderr but never abort the scan.
fn scan_cron_triggers(flows_dir: &Path) -> Vec<ScheduledFlow> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(flows_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_flow_path(&path) {
            continue;
        }
        let flow = match lumo_dsl::parse_file(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ! cron skip: {} parse: {e}", path.display());
                continue;
            }
        };
        if let Err(e) = lumo_dsl::validate(&flow) {
            eprintln!("  ! cron skip: {} validate: {e}", path.display());
            continue;
        }
        for trigger in &flow.spec.triggers {
            if trigger.kind != "cron" {
                continue;
            }
            let Some(schedule_str) = cron_schedule_from(&trigger.with) else {
                eprintln!(
                    "  ! cron skip: {} trigger missing `schedule` string",
                    path.display()
                );
                continue;
            };
            let parsed = match Schedule::from_str(&schedule_str) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "  ! cron skip: {} invalid schedule `{schedule_str}`: {e}",
                        path.display()
                    );
                    continue;
                }
            };
            let name = flow_display_name(&path);
            out.push(ScheduledFlow {
                name,
                path: path.clone(),
                schedule: schedule_str,
                parsed,
            });
        }
    }
    out
}

/// Strip the double-extension `.lumoflow.{yaml,yml}` to get a clean banner
/// name. Falls back to whatever `file_stem` gives if the suffix doesn't match.
fn flow_display_name(path: &Path) -> String {
    let raw = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    raw.strip_suffix(".lumoflow.yaml")
        .or_else(|| raw.strip_suffix(".lumoflow.yml"))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"))
        .to_string()
}

fn cron_schedule_from(with: &serde_yaml::Value) -> Option<String> {
    let s = with.get("schedule").and_then(|v| v.as_str())?;
    if s.trim().is_empty() {
        return None;
    }
    Some(s.to_string())
}

fn is_flow_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".lumoflow.yaml") || n.ends_with(".lumoflow.yml"))
        .unwrap_or(false)
}

/// Long-running task per scheduled flow. Sleeps until the next fire time
/// according to the cron schedule, then dispatches the flow. Re-parses the
/// flow on every fire so edits hot-reload without a server restart.
async fn schedule_loop(sf: ScheduledFlow, home: PathBuf) {
    loop {
        let Some(next) = sf.parsed.upcoming(chrono::Utc).next() else {
            tracing::warn!(
                "cron {} has no upcoming fire time; scheduler loop exiting",
                sf.name
            );
            return;
        };
        let now = chrono::Utc::now();
        let wait = (next - now)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(1));
        tokio::time::sleep(wait).await;
        if let Err(e) = run_cron_flow(&sf.path, &home).await {
            tracing::error!("cron run {}: {e}", sf.name);
        }
    }
}

async fn run_cron_flow(flow_path: &Path, home: &Path) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(flow_path)?;
    lumo_dsl::validate(&flow)?;
    let registry = build_action_registry(home, Some(flow_path));
    let repo = Some(Repo::open(home.join("lumo.db"))?);
    // 架构 P1-1:cron 是 headless 触发 —— 走宿主组装,不注入 prompter。取消
    // 令牌每次新建(cron 无外部取消入口,step_timeout 兜底)。
    let vm = super::host_vm(home, &flow, registry, repo, CancelToken::new());
    vm.run(
        &flow,
        RunOptions {
            inputs: Value::Object(serde_json::Map::new()),
            trigger_kind: "cron".into(),
        },
    )
    .await?;
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct WatchedFlow {
    name: String,
    flow_path: PathBuf,
    watch_path: PathBuf,
    events: Vec<String>,
    pattern: Option<String>,
}

/// Scan `--flows` for flows that declare a `file` trigger and return one
/// `WatchedFlow` per (flow × file trigger). Mirror of `scan_cron_triggers`.
pub(crate) fn scan_file_triggers(flows_dir: &Path) -> Vec<WatchedFlow> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(flows_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_flow_path(&path) {
            continue;
        }
        let flow = match lumo_dsl::parse_file(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ! file skip: {} parse: {e}", path.display());
                continue;
            }
        };
        if let Err(e) = lumo_dsl::validate(&flow) {
            eprintln!("  ! file skip: {} validate: {e}", path.display());
            continue;
        }
        for trigger in &flow.spec.triggers {
            if trigger.kind != "file" {
                continue;
            }
            let Some(watch_path_str) = trigger.with.get("path").and_then(|v| v.as_str()) else {
                eprintln!(
                    "  ! file skip: {} trigger missing `path` string",
                    path.display()
                );
                continue;
            };
            let watch_path = PathBuf::from(watch_path_str);
            let events = trigger
                .with
                .get("events")
                .and_then(|v| v.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_lowercase()))
                        .collect::<Vec<_>>()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec!["create".into(), "modify".into()]);
            let pattern = trigger
                .with
                .get("pattern")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            out.push(WatchedFlow {
                name: flow_display_name(&path),
                flow_path: path.clone(),
                watch_path,
                events,
                pattern,
            });
        }
    }
    out
}

/// Drive a `notify` watcher in a blocking thread and forward sync events into
/// an async channel. Each matching event re-parses the flow and dispatches a
/// run with `inputs = { trigger: { path, kind } }`.
async fn watch_loop(wf: WatchedFlow, home: PathBuf) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<notify::Result<Event>>();
    let watch_path = wf.watch_path.clone();
    std::thread::spawn(move || {
        let (sync_tx, sync_rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = sync_tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("watcher init failed: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
            eprintln!("watcher start ({}): {e}", watch_path.display());
            return;
        }
        for msg in sync_rx {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });

    while let Some(msg) = rx.recv().await {
        match msg {
            Ok(event) => {
                let Some(kind_label) = classify_event(&event.kind) else {
                    continue;
                };
                if !wf.events.iter().any(|e| e == &kind_label) {
                    continue;
                }
                let Some(matched_path) = event
                    .paths
                    .iter()
                    .find(|p| matches_pattern(p, wf.pattern.as_deref()))
                else {
                    continue;
                };
                if let Err(e) = run_file_flow(&wf.flow_path, &home, matched_path, &kind_label).await
                {
                    tracing::error!("file-trigger run {}: {e}", wf.name);
                }
            }
            Err(e) => tracing::warn!("watcher error for {}: {e}", wf.name),
        }
    }
}

pub(crate) fn classify_event(kind: &EventKind) -> Option<String> {
    match kind {
        EventKind::Create(_) => Some("create".into()),
        EventKind::Modify(_) => Some("modify".into()),
        EventKind::Remove(_) => Some("remove".into()),
        _ => None,
    }
}

pub(crate) fn matches_pattern(path: &Path, pattern: Option<&str>) -> bool {
    let Some(pat) = pattern else {
        return true;
    };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    glob_match(name, pat)
}

/// Minimal glob matcher (handles `*` only — enough for filename patterns
/// like `*.csv` / `report_*.json`).
pub(crate) fn glob_match(candidate: &str, pattern: &str) -> bool {
    if !pattern.contains('*') {
        return candidate == pattern;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = candidate;
    if let Some(first) = parts.first() {
        if !first.is_empty() {
            let Some(stripped) = rest.strip_prefix(first) else {
                return false;
            };
            rest = stripped;
        }
    }
    for part in parts.iter().skip(1).take(parts.len().saturating_sub(2)) {
        if part.is_empty() {
            continue;
        }
        let Some(pos) = rest.find(part) else {
            return false;
        };
        rest = &rest[pos + part.len()..];
    }
    if let Some(last) = parts.last() {
        last.is_empty() || rest.ends_with(last)
    } else {
        true
    }
}

async fn run_file_flow(
    flow_path: &Path,
    home: &Path,
    event_path: &Path,
    event_kind: &str,
) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(flow_path)?;
    lumo_dsl::validate(&flow)?;
    let registry = build_action_registry(home, Some(flow_path));
    let repo = Some(Repo::open(home.join("lumo.db"))?);
    // 架构 P1-1:file 触发同 cron —— headless 宿主组装,无 prompter,取消
    // 令牌每次新建。
    let vm = super::host_vm(home, &flow, registry, repo, CancelToken::new());
    let mut inputs = serde_json::Map::new();
    inputs.insert(
        "trigger".into(),
        serde_json::json!({
            "path": event_path.display().to_string(),
            "kind": event_kind,
        }),
    );
    vm.run(
        &flow,
        RunOptions {
            inputs: Value::Object(inputs),
            trigger_kind: "file".into(),
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn write_flow(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.lumoflow.yaml")), body.trim_start()).unwrap();
    }

    fn flow_with_webhook(id: &str) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  triggers:
    - {{ kind: webhook }}
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "hello from {id}" }} }}
"#,
        )
    }

    fn flow_without_webhook(id: &str) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "hello" }} }}
"#,
        )
    }

    fn test_state(flows: &TempDir, home: &TempDir, token: Option<String>) -> AppState {
        AppState::new(
            flows.path().to_path_buf(),
            home.path().to_path_buf(),
            token,
            8,
        )
    }

    #[tokio::test]
    async fn healthz_responds_ok() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_triggers_flow_and_returns_run_id() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_flow(flows.path(), "ping", &flow_with_webhook("ping"));
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/ping")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["success"], true);
        assert!(body["run_id"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(body["steps_ok"], 1);
    }

    #[tokio::test]
    async fn webhook_rejects_flow_without_webhook_trigger() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_flow(flows.path(), "nope", &flow_without_webhook("nope"));
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/nope")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn webhook_404_on_missing_flow() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/ghost")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn webhook_token_gate_requires_header() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_flow(flows.path(), "secret", &flow_with_webhook("secret"));
        let app = build_app(test_state(&flows, &home, Some("s3cret".into())));
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/secret")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp_ok = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/secret")
                    .header("content-type", "application/json")
                    .header("x-lumo-token", "s3cret")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn webhook_rejects_path_traversal() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/..%2Fetc%2Fpasswd")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        // axum decodes the URL → the handler sees `../etc/passwd`, which the
        // path-traversal guard rejects with 400.
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ─── P1-1: cancel 路由 + webhook 并发门 ──────────────────────────────

    fn flow_with_webhook_sleep(id: &str, ms: u64) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  triggers:
    - {{ kind: webhook }}
  steps:
    - {{ id: nap, action: control.sleep, with: {{ ms: {ms} }} }}
"#,
        )
    }

    fn post_json(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("x-lumo-token", t);
        }
        b.body(Body::from("{}")).unwrap()
    }

    async fn json_body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn cancel_unknown_run_returns_ok_false() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let app = build_app(test_state(&flows, &home, None));
        let resp = app
            .oneshot(post_json("/runs/ghost/cancel", None))
            .await
            .unwrap();
        // 幂等语义:不存在(或已结束)的 run 返回 200 + ok=false,不报错。
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["run_id"], "ghost");
    }

    #[tokio::test]
    async fn cancel_route_honors_token_gate() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let app = build_app(test_state(&flows, &home, Some("s3cret".into())));
        let resp = app
            .clone()
            .oneshot(post_json("/runs/x/cancel", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let resp_ok = app
            .oneshot(post_json("/runs/x/cancel", Some("s3cret")))
            .await
            .unwrap();
        assert_eq!(resp_ok.status(), StatusCode::OK);
        assert_eq!(json_body(resp_ok).await["ok"], false);
    }

    /// 全链路:webhook 启动一个 30s 长睡 run → 取消表出现键(宿主预生成
    /// run_id,启动前登记)→ POST cancel 返回 ok=true → 原请求以 500 +
    /// "cancelled" 收场 → 表项随 RAII 守卫摘除,再取消返回 ok=false(幂等)。
    #[tokio::test]
    async fn cancel_running_webhook_run_is_cooperative_and_idempotent() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_flow(
            flows.path(),
            "sleepy",
            &flow_with_webhook_sleep("sleepy", 30_000),
        );
        let state = test_state(&flows, &home, None);
        let app = build_app(state.clone());

        let handle = tokio::spawn(app.clone().oneshot(post_json("/webhook/sleepy", None)));

        // 等待取消表出现键(run 已登记并启动)。上限 10s,防挂死。
        let run_id = {
            let mut found = None;
            for _ in 0..400 {
                if let Some(id) = state.cancels.lock().keys().next().cloned() {
                    found = Some(id);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            found.expect("run must register its cancel token before running")
        };

        let resp = app
            .clone()
            .oneshot(post_json(&format!("/runs/{run_id}/cancel"), None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(json_body(resp).await["ok"], true);

        // 原 webhook 请求以 500 + "cancelled" 收场(协作取消打断长睡)。
        let webhook_resp = handle.await.unwrap().unwrap();
        assert_eq!(webhook_resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(webhook_resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(text.contains("cancelled"), "got body: {text}");

        // RAII 守卫已摘表项 ⇒ 再取消同一 run 返回 ok=false(已结束幂等)。
        assert!(state.cancels.lock().is_empty(), "guard must clear the map");
        let resp_again = app
            .oneshot(post_json(&format!("/runs/{run_id}/cancel"), None))
            .await
            .unwrap();
        assert_eq!(json_body(resp_again).await["ok"], false);
    }

    /// 并发门:max_concurrency=1 时,占住唯一许可的情况下第二个请求排队
    /// (300ms 内不完成);释放许可后它继续执行并成功。
    #[tokio::test]
    async fn webhook_concurrency_gate_queues_requests() {
        let flows = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        write_flow(flows.path(), "ping", &flow_with_webhook("ping"));
        let state = AppState::new(
            flows.path().to_path_buf(),
            home.path().to_path_buf(),
            None,
            1,
        );
        let app = build_app(state.clone());

        // 手动占住唯一许可,模拟一个在跑的 webhook。
        let permit = state.permits.clone().acquire_owned().await.unwrap();

        let mut handle = tokio::spawn(app.oneshot(post_json("/webhook/ping", None)));
        let parked = tokio::time::timeout(std::time::Duration::from_millis(300), &mut handle).await;
        assert!(parked.is_err(), "request must queue while the gate is full");

        drop(permit); // 释放 ⇒ 排队请求继续
        let resp = tokio::time::timeout(std::time::Duration::from_secs(10), handle)
            .await
            .expect("queued request must proceed after release")
            .unwrap()
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn serve_max_concurrency_parses_env_shape() {
        assert_eq!(serve_max_concurrency_from(None), 8, "未设置 ⇒ 默认 8");
        assert_eq!(serve_max_concurrency_from(Some("3")), 3);
        assert_eq!(
            serve_max_concurrency_from(Some("0")),
            8,
            "0 会永久堵死 webhook,按配置错误回退默认"
        );
        assert_eq!(serve_max_concurrency_from(Some("nah")), 8);
    }

    // ─── cron scheduler tests ────────────────────────────────────────────

    fn flow_with_cron(id: &str, schedule: &str) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  triggers:
    - {{ kind: cron, with: {{ schedule: "{schedule}" }} }}
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "tick" }} }}
"#,
        )
    }

    #[test]
    fn scan_cron_finds_flows_with_cron_trigger() {
        let flows = TempDir::new().unwrap();
        write_flow(
            flows.path(),
            "hourly",
            &flow_with_cron("hourly", "0 0 * * * *"),
        );
        write_flow(
            flows.path(),
            "webhook_only",
            &flow_with_webhook("webhook_only"),
        );
        let scheduled = scan_cron_triggers(flows.path());
        assert_eq!(scheduled.len(), 1, "only hourly should be picked");
        assert_eq!(scheduled[0].name, "hourly");
        assert_eq!(scheduled[0].schedule, "0 0 * * * *");
    }

    #[test]
    fn scan_cron_skips_invalid_schedule() {
        let flows = TempDir::new().unwrap();
        write_flow(
            flows.path(),
            "broken",
            &flow_with_cron("broken", "not a cron"),
        );
        let scheduled = scan_cron_triggers(flows.path());
        assert!(scheduled.is_empty(), "broken schedule must not crash scan");
    }

    #[test]
    fn scan_cron_handles_multiple_files() {
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "a", &flow_with_cron("a", "0 */5 * * * *"));
        write_flow(flows.path(), "b", &flow_with_cron("b", "0 0 12 * * *"));
        let scheduled = scan_cron_triggers(flows.path());
        assert_eq!(scheduled.len(), 2);
        let names: Vec<_> = scheduled.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn parsed_schedule_can_compute_next_fire() {
        let flows = TempDir::new().unwrap();
        write_flow(
            flows.path(),
            "every_min",
            &flow_with_cron("every_min", "0 * * * * *"),
        );
        let scheduled = scan_cron_triggers(flows.path());
        assert_eq!(scheduled.len(), 1);
        // Next fire from now must produce a future timestamp.
        let next = scheduled[0]
            .parsed
            .upcoming(chrono::Utc)
            .next()
            .expect("at least one upcoming fire");
        assert!(next > chrono::Utc::now());
    }

    fn flow_with_file_trigger(id: &str, path: &str, events: &str, pattern: Option<&str>) -> String {
        let pat = pattern
            .map(|p| format!(", pattern: \"{p}\""))
            .unwrap_or_default();
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  triggers:
    - {{ kind: file, with: {{ path: "{path}", events: {events}{pat} }} }}
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "tick" }} }}
"#,
        )
    }

    #[test]
    fn scan_file_finds_flows_with_file_trigger() {
        let flows = TempDir::new().unwrap();
        let inbox = TempDir::new().unwrap();
        write_flow(
            flows.path(),
            "inbox",
            &flow_with_file_trigger(
                "inbox",
                &inbox.path().display().to_string(),
                "[create, modify]",
                Some("*.csv"),
            ),
        );
        write_flow(flows.path(), "wh", &flow_with_webhook("wh"));
        let watched = scan_file_triggers(flows.path());
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].name, "inbox");
        assert_eq!(watched[0].events, vec!["create", "modify"]);
        assert_eq!(watched[0].pattern.as_deref(), Some("*.csv"));
    }

    #[test]
    fn scan_file_uses_default_events_when_omitted() {
        let flows = TempDir::new().unwrap();
        let inbox = TempDir::new().unwrap();
        // Trigger without an events list defaults to [create, modify].
        let yaml = format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: x }}
spec:
  triggers:
    - {{ kind: file, with: {{ path: "{}" }} }}
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "tick" }} }}
"#,
            inbox.path().display()
        );
        write_flow(flows.path(), "x", &yaml);
        let watched = scan_file_triggers(flows.path());
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].events, vec!["create", "modify"]);
    }

    #[test]
    fn scan_file_skips_trigger_missing_path() {
        let flows = TempDir::new().unwrap();
        let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: x }
spec:
  triggers:
    - { kind: file, with: { events: [create] } }
  steps:
    - { id: hi, action: control.log, with: { message: "tick" } }
"#;
        write_flow(flows.path(), "x", yaml);
        let watched = scan_file_triggers(flows.path());
        assert!(watched.is_empty());
    }

    #[test]
    fn classify_event_maps_kinds() {
        use notify::event::{CreateKind, ModifyKind, RemoveKind};
        assert_eq!(
            classify_event(&EventKind::Create(CreateKind::File)),
            Some("create".into())
        );
        assert_eq!(
            classify_event(&EventKind::Modify(ModifyKind::Any)),
            Some("modify".into())
        );
        assert_eq!(
            classify_event(&EventKind::Remove(RemoveKind::File)),
            Some("remove".into())
        );
        assert_eq!(
            classify_event(&EventKind::Access(notify::event::AccessKind::Any)),
            None
        );
    }

    #[test]
    fn glob_match_handles_wildcard_filenames() {
        assert!(glob_match("report.csv", "*.csv"));
        assert!(glob_match("report_2026.json", "report_*.json"));
        assert!(!glob_match("report.json", "*.csv"));
        assert!(glob_match("anything", "*"));
        assert!(glob_match("exact", "exact"));
    }

    #[test]
    fn matches_pattern_accepts_none_as_always_true() {
        assert!(matches_pattern(Path::new("/tmp/x.txt"), None));
        assert!(matches_pattern(Path::new("/tmp/x.csv"), Some("*.csv")));
        assert!(!matches_pattern(Path::new("/tmp/x.json"), Some("*.csv")));
    }
}
