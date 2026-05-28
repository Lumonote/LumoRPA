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
use lumo_core::{FlowVm, RunOptions};
use lumo_storage::Repo;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::net::TcpListener;
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

#[derive(Clone)]
struct AppState {
    flows_dir: PathBuf,
    home: PathBuf,
    token: Option<String>,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    std::fs::create_dir_all(&home)?;
    if !args.flows.exists() {
        anyhow::bail!("--flows directory {} does not exist", args.flows.display());
    }
    let state = AppState {
        flows_dir: args.flows.clone(),
        home: home.clone(),
        token: args.token,
    };
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
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn webhook(
    State(state): State<AppState>,
    AxumPath(flow_name): AxumPath<String>,
    headers: HeaderMap,
    Json(inputs): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
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
    let registry = build_action_registry(&state.home, Some(&path));
    let repo = Some(
        Repo::open(state.home.join("lumo.db"))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    );
    let vm = FlowVm::new(registry, repo);
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
    let vm = FlowVm::new(registry, repo);
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
    let vm = FlowVm::new(registry, repo);
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
        AppState {
            flows_dir: flows.path().to_path_buf(),
            home: home.path().to_path_buf(),
            token,
        }
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
