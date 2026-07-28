use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter, ChatAction,
};
use lumo_core::{
    ActionRegistry, CancelToken, FlowVm, HumanPromptKind, HumanPromptRequest, HumanPrompter,
    HumanResponse, RunOptions, StepError,
};
use lumo_dsl::{Flow, IoDecl, Step};
use lumo_recorder::{
    desktop::DesktopRecorder, events_to_yaml_patch, BrowserRecorder, NoopRecorder, RawEvent,
    Recorder,
};
use lumo_skills::{register_flow_call_action, register_skill_actions, SkillRegistry};
use lumo_storage::{FlowRunRow, Repo, StepRunRow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use serde_yaml::{Mapping as YamlMapping, Value as YamlValue};
use std::{
    collections::{hash_map::DefaultHasher, BTreeMap, HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, OnceLock},
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State, WindowEvent, Wry,
};

mod agent_commands;
mod agent_management_commands;
mod agent_service;
mod desktop_update_commands;
mod dto;
mod element_library;
mod execution;
mod features_data;
mod job_commands;
mod mcp_commands;
mod mcp_supervisor;
mod paths;
mod prompter;
mod registry;
mod recording;
mod run_progress;
mod security_commands;
mod skill_commands;
mod state;
#[allow(dead_code)]
mod update_commands;
mod voice_commands;
mod voice_daemon;
mod voice_intents;
mod voice_model_commands;
mod voice_persona;

use agent_commands::{
    agent_approve, agent_cancel, agent_events, agent_pause, agent_restore, agent_resume,
    agent_run_list, agent_start, setup_agent_service, DesktopAgentRuntime,
};
use agent_management_commands::{
    approve_improvement, evaluate_improvement, list_agent_profiles, list_improvement_proposals,
    reject_improvement, rollback_improvement,
};
use agent_service::DesktopAgentService;
use desktop_update_commands::{
    desktop_update_check, desktop_update_install, desktop_update_restart, desktop_update_status,
};
use execution::{execute_flow, DebugOpts};
use features_data::{feature_map_data, FeatureSection};
use job_commands::{
    job_cancel, job_list, job_pause, job_resume, job_schedule, setup_job_worker, JobRuntime,
};
use mcp_commands::{
    apply_mcp_import, approve_mcp_schema_change, call_mcp_tool, delete_mcp_server,
    discover_mcp_tools, list_mcp_servers, mcp_oauth_callback, mcp_oauth_refresh, mcp_oauth_start,
    preview_mcp_import, set_mcp_server_enabled, set_mcp_tool_enabled, test_mcp_server,
};
use mcp_supervisor::McpSupervisorRuntime;
use dto::*;
use element_library::*;
use paths::*;
use recording::*;
use registry::*;
use prompter::{desktop_prompter, human_respond};
use security_commands::{
    security_biometric_challenge as security_biometric_challenge_at,
    security_export_audit as security_export_audit_at, security_list as security_list_at,
    security_revoke as security_revoke_at, DesktopSecurityRuntime, SecuritySnapshotDto,
};
use skill_commands::{
    skill_activate, skill_import_git, skill_import_local, skill_rollback, skill_set_enabled,
    skill_validate, skill_versions,
};
use state::{CancelMap, DesktopState, PromptMap, RecorderSession};
use voice_commands::{
    handle_global_shortcut, setup_voice_daemon, setup_voice_host, voice_command_history,
    voice_configure, voice_daemon_resume, voice_daemon_set_device, voice_daemon_set_enabled,
    voice_daemon_set_muted, voice_daemon_status, voice_daemon_suspend, voice_devices,
    voice_start_listening, voice_status, voice_stop, voice_tts_preview, VoiceRuntime,
};
use voice_daemon::VoiceDaemon;
use voice_model_commands::{
    voice_model_install, voice_model_list, voice_model_remove, VoiceModelRuntime,
};

type AppHandle = tauri::AppHandle<Wry>;

/// P2-3：进程级 AppHandle，setup 阶段注入一次。注册表缓存构建深处（无 tauri
/// State 可拿）用它把 skills 加载失败以 `lumo://toast` 事件透给前端；测试
/// 路径不经过 setup ⇒ 保持为空 ⇒ 静默跳过 emit。
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

#[tauri::command]
fn security_list(state: State<'_, DesktopState>) -> Result<SecuritySnapshotDto, String> {
    security_list_at(&state.security, chrono::Utc::now())
}

#[tauri::command]
fn security_revoke(
    state: State<'_, DesktopState>,
    request: lumo_agent::PermissionRevocationRequest,
) -> Result<lumo_agent::PermissionRevocation, String> {
    security_revoke_at(&state.security, request, chrono::Utc::now())
}

#[tauri::command]
fn security_export_audit(state: State<'_, DesktopState>) -> Result<String, String> {
    security_export_audit_at(&state.security)
}

#[tauri::command]
fn security_biometric_challenge(
    state: State<'_, DesktopState>,
    request: lumo_agent::PermissionGrantRequest,
) -> Result<lumo_agent::PermissionGrant, String> {
    security_biometric_challenge_at(&state.security, request, chrono::Utc::now())
}



// ─── Tauri commands ─────────────────────────────────────────────────────────

#[tauri::command]
fn app_info(app: AppHandle) -> Result<AppInfo, String> {
    let data_dir = app_home(&app)?;
    let resource_dir = app.path().resource_dir().ok();
    let examples_dir = examples_dir(&app);
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.display().to_string(),
        resource_dir: resource_dir.map(|p| p.display().to_string()),
        examples_dir: examples_dir.map(|p| p.display().to_string()),
        providers_path: providers_path(&data_dir).display().to_string(),
        skills_path: skills_root(&data_dir).display().to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        network_enabled: llm_network_enabled(),
    })
}

/// Bundled example flows for the (future) "Examples" gallery tab. Registered
/// API with no JS caller yet — kept intentionally: it's read-only, exposes a
/// real bundled-content capability, and is the natural backing command for an
/// examples picker. Remove only when that UI is ruled out.
#[tauri::command]
fn list_examples(app: AppHandle) -> Result<Vec<FlowSummary>, String> {
    let Some(dir) = examples_dir(&app) else {
        return Ok(Vec::new());
    };
    let mut flows = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if is_flow_file(&path) {
            let mut s = flow_summary(&path);
            s.source = "example".into();
            flows.push(s);
        }
    }
    flows.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(flows)
}

/// D-* Flow library: returns every flow Studio knows about, tagged by source
/// (`user` / `recording` / `example`). Frontend defaults to user-first, then
/// recordings, with examples folded. Empty user/recording dirs are auto-
/// created on access so this is the canonical "where do my saved flows go?"
/// answer the UI can rely on.
#[tauri::command]
fn list_flow_library(app: AppHandle) -> Result<Vec<FlowSummary>, String> {
    let mut out: Vec<FlowSummary> = Vec::new();
    // User flows (saved via `save_flow_as` / "另存为").
    out.extend(scan_flows_in(&user_flows_dir(&app)?, "user"));
    // Recorder output.
    out.extend(scan_flows_in(&recordings_dir(&app)?, "recording"));
    // Bundled examples (read-only in production builds).
    if let Some(ex) = examples_dir(&app) {
        out.extend(scan_flows_in(&ex, "example"));
    }
    Ok(out)
}

/// Copy `source` into the user flows dir under `name` (auto-suffixed with
/// `.lumoflow.yaml` if missing). Returns the new absolute path so the caller
/// can immediately load it. Used by both Studio "另存为" and the recording
/// → save flow.
#[tauri::command]
fn save_flow_as(app: AppHandle, name: String, source: String) -> Result<String, String> {
    let dir = user_flows_dir(&app)?;
    let safe = sanitize_flow_name(&name);
    if safe.is_empty() {
        return Err("flow name must not be empty".into());
    }
    let path = dir.join(
        if safe.ends_with(".lumoflow.yaml") || safe.ends_with(".lumoflow.yml") {
            safe
        } else {
            format!("{safe}.lumoflow.yaml")
        },
    );
    std::fs::write(&path, source).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn export_flow_as(app: AppHandle, name: String, source: String) -> Result<String, String> {
    let dir = exports_dir(&app)?;
    let safe = sanitize_flow_name(&name);
    if safe.is_empty() {
        return Err("export name must not be empty".into());
    }
    let file_name = if safe.ends_with(".lumoflow.yaml")
        || safe.ends_with(".lumoflow.yml")
        || safe.ends_with(".yaml")
        || safe.ends_with(".yml")
    {
        safe
    } else {
        format!("{safe}.lumoflow.yaml")
    };
    let path = dir.join(file_name);
    std::fs::write(&path, source).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// Delete a file from the library. Guards against deleting bundled examples
/// or anything outside `$LUMO_HOME` so a bug in the UI can't nuke user data
/// it isn't allowed to.
#[tauri::command]
fn delete_flow(app: AppHandle, path: String) -> Result<(), String> {
    let target = Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("resolve {path}: {e}"))?;
    let home = app_home(&app)?
        .canonicalize()
        .map_err(|e| format!("resolve LUMO_HOME: {e}"))?;
    if !target.starts_with(&home) {
        return Err(format!(
            "refused: {} is outside LUMO_HOME",
            target.display()
        ));
    }
    std::fs::remove_file(&target).map_err(|e| format!("delete {}: {e}", target.display()))?;
    Ok(())
}

/// Duplicate a flow into the user dir (works for any source — including
/// examples — and gives the copy a `-copy` suffix so the original stays put).
#[tauri::command]
fn duplicate_flow(app: AppHandle, path: String) -> Result<String, String> {
    // P0-3: only duplicate from an allowed flow directory.
    let src = resolve_within(&path, &flow_read_roots(&app))?;
    let bytes = std::fs::read(&src).map_err(|e| format!("read {}: {e}", src.display()))?;
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("flow")
        .trim_end_matches(".lumoflow")
        .to_string();
    let dir = user_flows_dir(&app)?;
    let mut candidate = dir.join(format!("{stem}-copy.lumoflow.yaml"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-copy-{n}.lumoflow.yaml"));
        n += 1;
    }
    std::fs::write(&candidate, bytes).map_err(|e| format!("write {}: {e}", candidate.display()))?;
    Ok(candidate.display().to_string())
}

fn sanitize_flow_name(name: &str) -> String {
    let trimmed = name.trim();
    trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}


/// Save the recorder's last output as a complete flow under the recordings
/// folder. Returns the new file path so the library can refresh + select it.
#[tauri::command]
fn save_recording_as_flow(
    app: AppHandle,
    name: String,
    yaml_hint: String,
) -> Result<String, String> {
    let dir = recordings_dir(&app)?;
    let stem = sanitize_flow_name(&name);
    let stem = if stem.is_empty() {
        format!("rec-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    } else {
        stem
    };
    let mut candidate = dir.join(format!("{stem}.lumoflow.yaml"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}-{n}.lumoflow.yaml"));
        n += 1;
    }
    let body = wrap_recording_fragment(&stem, &yaml_hint)?;
    std::fs::write(&candidate, body).map_err(|e| format!("write {}: {e}", candidate.display()))?;
    Ok(candidate.display().to_string())
}

#[tauri::command]
fn load_element_library(app: AppHandle) -> Result<Value, String> {
    load_element_library_value(&app)
}

#[tauri::command]
fn save_element_library(app: AppHandle, library: Value) -> Result<(), String> {
    let mut normalized = library;
    normalize_element_library(&mut normalized);
    if let Some(obj) = normalized.as_object_mut() {
        obj.insert(
            "updatedAt".into(),
            chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                .into(),
        );
    }
    save_element_library_value(&app, &normalized)
}

#[tauri::command]
fn inspect_flow(app: AppHandle, path: String) -> FlowSummary {
    // P0-3: only summarize files inside the allowed flow directories.
    match resolve_within(&path, &flow_read_roots(&app)) {
        Ok(safe) => flow_summary(&safe),
        Err(e) => refused_summary(&path, e),
    }
}

#[tauri::command]
fn read_flow_source(app: AppHandle, path: String) -> Result<String, String> {
    // P0-3: confine reads to the flow library + bundled examples.
    let safe = resolve_within(&path, &flow_read_roots(&app))?;
    std::fs::read_to_string(&safe).map_err(|e| format!("read {}: {e}", safe.display()))
}

#[tauri::command]
fn reveal_flow_file(app: AppHandle, path: String) -> Result<(), String> {
    let safe = resolve_within(&path, &flow_read_roots(&app))?;
    reveal_path(&safe)
}

#[tauri::command]
fn save_flow_source(app: AppHandle, path: String, source: String) -> Result<(), String> {
    // P0-3: confine writes to LUMO_HOME (the parent dir must already exist —
    // brand-new flows go through `save_flow_as`, which targets the user dir).
    let home = app_home(&app)?;
    let safe = resolve_write_within(&path, &home)?;
    std::fs::write(&safe, source).map_err(|e| format!("write {}: {e}", safe.display()))
}

/// Snapshot of a flow's `spec.capabilities` block. Returned to the frontend so
/// Studio can render current grants alongside MCP-03's `proposed_grant` hints
/// (Se-01 / Se-02).
#[derive(serde::Serialize, serde::Deserialize, Debug, Default)]
struct CapabilitySnapshot {
    network: Vec<String>,
    #[serde(rename = "fs.read")]
    fs_read: Vec<String>,
    #[serde(rename = "fs.write")]
    fs_write: Vec<String>,
    llm: Vec<String>,
    mcp: Vec<String>,
}

#[tauri::command]
fn get_flow_capabilities(app: AppHandle, path: String) -> Result<CapabilitySnapshot, String> {
    let safe = resolve_within(&path, &flow_read_roots(&app))?;
    let source =
        std::fs::read_to_string(&safe).map_err(|e| format!("read {}: {e}", safe.display()))?;
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&source).map_err(|e| format!("yaml parse: {e}"))?;
    let caps = doc.get("spec").and_then(|s| s.get("capabilities"));
    Ok(CapabilitySnapshot {
        network: yaml_str_list(caps, "network"),
        fs_read: yaml_str_list(caps, "fs.read"),
        fs_write: yaml_str_list(caps, "fs.write"),
        llm: yaml_str_list(caps, "llm"),
        mcp: yaml_str_list(caps, "mcp"),
    })
}

/// Append a single grant to `spec.capabilities.<kind>` and persist the file.
/// `kind` is one of `"network"`, `"fs.read"`, `"fs.write"`, `"llm"`, `"mcp"`.
/// Skips the write if the grant is already present.
#[tauri::command]
fn add_capability_grant(path: String, kind: String, grant: String) -> Result<bool, String> {
    if !is_valid_cap_kind(&kind) {
        return Err(format!("invalid capability kind `{kind}`"));
    }
    if grant.trim().is_empty() {
        return Err("grant must not be empty".into());
    }
    let source = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(&source).map_err(|e| format!("yaml parse: {e}"))?;
    let appended = upsert_capability(&mut doc, &kind, grant.trim());
    if !appended {
        return Ok(false);
    }
    let rewritten = serde_yaml::to_string(&doc).map_err(|e| format!("yaml serialize: {e}"))?;
    std::fs::write(&path, rewritten).map_err(|e| format!("write {path}: {e}"))?;
    Ok(true)
}

fn is_valid_cap_kind(kind: &str) -> bool {
    matches!(kind, "network" | "fs.read" | "fs.write" | "llm" | "mcp")
}

fn yaml_str_list(caps: Option<&serde_yaml::Value>, key: &str) -> Vec<String> {
    caps.and_then(|c| c.get(key))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Insert `grant` into `spec.capabilities[kind]`, creating any missing nodes.
/// Returns `true` when the document was actually mutated.
fn upsert_capability(doc: &mut serde_yaml::Value, kind: &str, grant: &str) -> bool {
    use serde_yaml::Value;
    if !doc.is_mapping() {
        *doc = Value::Mapping(serde_yaml::Mapping::new());
    }
    let spec_key = Value::String("spec".into());
    let spec = doc
        .as_mapping_mut()
        .expect("doc is mapping")
        .entry(spec_key)
        .or_insert_with(|| Value::Mapping(serde_yaml::Mapping::new()));
    if !spec.is_mapping() {
        *spec = Value::Mapping(serde_yaml::Mapping::new());
    }
    let cap_key = Value::String("capabilities".into());
    let caps = spec
        .as_mapping_mut()
        .expect("spec is mapping")
        .entry(cap_key)
        .or_insert_with(|| Value::Mapping(serde_yaml::Mapping::new()));
    if !caps.is_mapping() {
        *caps = Value::Mapping(serde_yaml::Mapping::new());
    }
    let kind_key = Value::String(kind.into());
    let list = caps
        .as_mapping_mut()
        .expect("caps is mapping")
        .entry(kind_key)
        .or_insert_with(|| Value::Sequence(Vec::new()));
    if !list.is_sequence() {
        *list = Value::Sequence(Vec::new());
    }
    let seq = list.as_sequence_mut().expect("list is sequence");
    if seq.iter().any(|v| v.as_str() == Some(grant)) {
        return false;
    }
    seq.push(Value::String(grant.into()));
    true
}

#[tauri::command]
fn validate_flow(app: AppHandle, path: String) -> Result<ValidationReport, String> {
    let home = app_home(&app)?;
    let flow = parse_and_validate(&home, Path::new(&path))?;
    Ok(validation_report(&path, &flow))
}

/// D-19 Flow Lint. Runs structural lint plus capability / variable-reference
/// checks. Studio surfaces the returned issues in a side panel with severity
/// tags and per-rule "+ fix" buttons (e.g. add capability, declare input).
#[tauri::command]
fn lint_flow(app: AppHandle, path: String) -> Result<Vec<lumo_dsl::LintIssue>, String> {
    let home = app_home(&app)?;
    let flow = lumo_dsl::parse_file(Path::new(&path)).map_err(|e| e.to_string())?;
    let registry = build_action_registry(&home, Some(Path::new(&path)));
    let known: Vec<String> = registry.iter_ids().collect();
    let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
    Ok(lumo_dsl::lint_flow(&flow, &known_refs))
}

#[tauri::command]
async fn run_flow(
    app: AppHandle,
    state: State<'_, DesktopState>,
    path: String,
    inputs_json: String,
    no_store: bool,
) -> Result<RunResponse, String> {
    let home = app_home(&app)?;
    let flow_path = Path::new(&path);
    let flow = parse_and_validate(&home, flow_path)?;
    let inputs = parse_inputs(&inputs_json)?;
    execute_flow(
        &home,
        Some(flow_path),
        flow,
        inputs,
        no_store,
        DebugOpts::default(),
        &state.cancels,
        Some(desktop_prompter(&app, &state)),
    )
    .await
}

/// Run a single step from the flow by `step_id`. The step is extracted from
/// the flow (including its nested children for control-flow steps) and wrapped
/// in an ad-hoc flow that preserves the original metadata + capability set so
/// validation still passes. Used by the Studio "▶ run this step" affordance —
/// one of the desktop Studio differentiators: run a selected node directly
/// instead of always starting from the top.
#[tauri::command]
async fn run_step(
    app: AppHandle,
    state: State<'_, DesktopState>,
    path: String,
    step_id: String,
    inputs_json: String,
    no_store: bool,
) -> Result<RunResponse, String> {
    let home = app_home(&app)?;
    let flow_path = Path::new(&path);
    let mut flow = parse_and_validate(&home, flow_path)?;
    let extracted = extract_step(&flow.spec.steps, &step_id)
        .ok_or_else(|| format!("step `{step_id}` not found in flow"))?
        .clone();
    flow.spec.steps = vec![extracted];
    // Rewrite the id so the persisted run is identifiable as a sub-run.
    flow.metadata.id = format!("{}::{step_id}", flow.metadata.id);
    let inputs = parse_inputs(&inputs_json)?;
    execute_flow(
        &home,
        Some(flow_path),
        flow,
        inputs,
        no_store,
        DebugOpts::default(),
        &state.cancels,
        Some(desktop_prompter(&app, &state)),
    )
    .await
}

/// F-20: run a flow under the breakpoint debugger. Pauses *before* the first
/// step whose path is in `breakpoints` (or, when `step` is set, before the very
/// next executing step) and returns a `RunResponse` whose `report.pausedAt`
/// names that step — its per-step `varsJson` (F-19) shows the variable state
/// entering the breakpoint. To advance, call again with `resumeFrom` set to the
/// paused run's id: the completed steps are replayed, the paused step is
/// "stepped off", and the run continues to the next pause point. Always
/// persists (debugging needs `step_runs` for resume), so `noStore` is implied
/// false.
#[tauri::command]
async fn debug_flow(
    app: AppHandle,
    state: State<'_, DesktopState>,
    path: String,
    inputs_json: String,
    breakpoints: Vec<String>,
    step: bool,
    resume_from: Option<String>,
) -> Result<RunResponse, String> {
    let home = app_home(&app)?;
    let flow_path = Path::new(&path);
    let flow = parse_and_validate(&home, flow_path)?;
    let inputs = parse_inputs(&inputs_json)?;
    execute_flow(
        &home,
        Some(flow_path),
        flow,
        inputs,
        false,
        DebugOpts {
            breakpoints: breakpoints.into_iter().collect(),
            step_mode: step,
            resume_from,
        },
        &state.cancels,
        Some(desktop_prompter(&app, &state)),
    )
    .await
}

/// P0-1：`cancel_run` 的返回。`ok=false` 表示该 run 不存在或已结束 —— 取消
/// 与运行结束天然存在竞态，前端只需提示"已无可取消的运行"，不必当错误处理。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelRunResult {
    ok: bool,
}

/// P0-1：取消一个进行中的 run。幂等：运行中重复调用仍返回 ok=true（token
/// 的 cancel 本身幂等）；run 不存在 / 已结束返回 ok=false 而非报错。
#[tauri::command]
fn cancel_run(state: State<'_, DesktopState>, run_id: String) -> CancelRunResult {
    CancelRunResult {
        ok: cancel_run_inner(&state.cancels, &run_id),
    }
}

/// `cancel_run` 的内核，测试脱离 tauri `State` 直接驱动。句柄留在表里由
/// run 自己清理（见 `execute_flow`），触发后立刻摘除会让"运行中重复取消"
/// 误报 ok=false。
fn cancel_run_inner(cancels: &CancelMap, run_id: &str) -> bool {
    match cancels
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(run_id)
    {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

#[tauri::command]
fn list_runs(app: AppHandle, limit: u32) -> Result<Vec<RunDto>, String> {
    let repo = open_repo(&app)?;
    Ok(repo
        .list_runs(limit)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(run_dto)
        .collect())
}

#[tauri::command]
fn show_run(app: AppHandle, run_id: String) -> Result<RunDetail, String> {
    let repo = open_repo(&app)?;
    let run = repo
        .get_run(&run_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("run `{run_id}` not found"))?;
    let steps = repo
        .list_steps(&run_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(step_dto)
        .collect();
    Ok(RunDetail {
        run: run_dto(run),
        steps,
    })
}

/// X-10: every LLM/vision call this run made, with token + USD breakdown.
/// Studio renders the rows under the timeline so the user can see exactly
/// where the budget went.
#[tauri::command]
fn run_cost(app: AppHandle, run_id: String) -> Result<Vec<lumo_storage::AiCallRow>, String> {
    let repo = open_repo(&app)?;
    repo.list_ai_calls(&run_id).map_err(|e| e.to_string())
}

/// X-07 Time-Travel: every blob artifact (screenshot, DOM, HAR) captured during
/// a run, ordered by capture time. Studio's replay scrubber walks these to
/// reconstruct what the screen looked like at each step.
#[tauri::command]
fn list_artifacts(
    app: AppHandle,
    run_id: String,
) -> Result<Vec<lumo_storage::ArtifactRow>, String> {
    let repo = open_repo(&app)?;
    repo.list_artifacts(&run_id).map_err(|e| e.to_string())
}

/// X-07: read a single artifact blob off disk and hand it back as a base64
/// data URL so the webview `<img>`/`<iframe>` can render it without a custom
/// asset protocol. Guarded against path escapes — the row's `blob_path` must
/// resolve under `$LUMO_HOME`.
#[tauri::command]
fn read_artifact_blob(app: AppHandle, artifact_id: String) -> Result<ArtifactBlobDto, String> {
    use base64::Engine as _;
    let repo = open_repo(&app)?;
    let row = repo
        .get_artifact(&artifact_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("artifact `{artifact_id}` not found"))?;
    let home = app_home(&app)?;
    let path = PathBuf::from(&row.blob_path);
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("artifact blob missing: {e}"))?;
    let home_canon = home.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(&home_canon) {
        return Err("artifact path escapes LUMO_HOME".into());
    }
    let bytes = std::fs::read(&canonical).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(ArtifactBlobDto {
        id: row.id,
        mime: row.mime.clone(),
        data_url: format!("data:{};base64,{}", row.mime, b64),
        size: row.size,
    })
}

#[tauri::command]
fn list_actions(app: AppHandle) -> Result<Vec<ActionDto>, String> {
    let home = app_home(&app)?;
    let registry = build_action_registry(&home, None);
    let mut ids: Vec<_> = registry.iter_ids().collect();
    ids.sort();
    Ok(ids
        .into_iter()
        .filter_map(|id| {
            registry.get(&id).map(|action| ActionDto {
                family: id.split('.').next().unwrap_or("misc").to_string(),
                summary: action.summary().to_string(),
                id,
            })
        })
        .collect())
}

#[tauri::command]
fn action_schema(app: AppHandle, id: String) -> Result<Value, String> {
    let home = app_home(&app)?;
    let registry = build_action_registry(&home, None);
    let action = registry
        .get(&id)
        .ok_or_else(|| format!("action `{id}` not found"))?;
    Ok(action.schema().clone())
}

#[tauri::command]
fn provider_status(app: AppHandle) -> Result<ProviderStatus, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn save_provider(app: AppHandle, profile: ProviderInput) -> Result<ProviderStatus, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let mut cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    let activate = profile.activate;
    let to_activate = profile.name.clone();
    let mut api_key = profile.api_key.filter(|s| !s.is_empty());
    let mut api_key_env = profile.api_key_env.filter(|s| !s.is_empty());
    if api_key.is_none() {
        if let Some(value) = api_key_env.as_deref() {
            if !looks_like_env_var_name(value) {
                api_key = api_key_env.take();
            }
        }
    }
    let p = ProviderProfile {
        name: profile.name,
        kind: profile.kind,
        wire_api: profile.wire_api,
        base_url: profile.base_url,
        api_key,
        api_key_env,
        default_model: profile.default_model.filter(|s| !s.is_empty()),
        vision_model: profile.vision_model.filter(|s| !s.is_empty()),
        ocr_model: profile.ocr_model.filter(|s| !s.is_empty()),
        models: profile
            .models
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect(),
        headers: profile.headers,
        reasoning_effort: profile.reasoning_effort.filter(|s| !s.is_empty()),
        notes: profile.notes.filter(|s| !s.is_empty()),
    };
    cfg.upsert(p);
    if activate || cfg.active.is_none() {
        let _ = cfg.use_(&to_activate);
    }
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn remove_provider(app: AppHandle, name: String) -> Result<ProviderStatus, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let mut cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    cfg.remove(&name).map_err(|e| e.to_string())?;
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn use_provider(app: AppHandle, name: String) -> Result<ProviderStatus, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let mut cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    cfg.use_(&name).map_err(|e| e.to_string())?;
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn init_providers(app: AppHandle, force: bool) -> Result<ProviderStatus, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    if path.exists() && !force {
        return Err(format!(
            "providers config already exists at {} (pass force=true to overwrite)",
            path.display()
        ));
    }
    let cfg = ProvidersConfig::seed_default();
    cfg.save(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn enable_llm_network_for_session(app: AppHandle) -> Result<ProviderStatus, String> {
    lumo_ai::enable_llm_network_for_current_process();
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    Ok(make_provider_status(&path, &cfg))
}

#[tauri::command]
fn list_ocr_models(app: AppHandle) -> Result<Vec<lumo_ai::OcrModelStatus>, String> {
    // P2-2：home 显式传参给 lumo-ai —— 此前这里 set_var("LUMO_HOME") 与运行中
    // flow 的 getenv（LUMO_STEP_TIMEOUT_MS / LUMO_ALLOW_PROCESS …）在多线程
    // tokio 下构成数据竞争。
    let home = app_home(&app)?;
    Ok(lumo_ai::ocr_model_catalog(&home))
}

#[tauri::command]
async fn download_ocr_model(
    app: AppHandle,
    model: String,
) -> Result<lumo_ai::OcrModelDownload, String> {
    // P2-2：同上 —— 显式 home，运行期不再写环境变量。
    let home = app_home(&app)?;
    lumo_ai::download_modelscope_model(&home, &model)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn test_provider(
    app: AppHandle,
    name: String,
    prompt: Option<String>,
) -> Result<ProviderTestResult, String> {
    let home = app_home(&app)?;
    let path = providers_path(&home);
    let cfg = ProvidersConfig::load(&path).map_err(|e| e.to_string())?;
    let Some(profile) = cfg.get(&name) else {
        return Ok(ProviderTestResult {
            ok: false,
            provider: None,
            model: None,
            content: None,
            input_tokens: 0,
            output_tokens: 0,
            error: Some(format!("profile `{name}` not found")),
        });
    };
    let Some(model) = profile.default_model.clone() else {
        return Ok(ProviderTestResult {
            ok: false,
            provider: Some(name.clone()),
            model: None,
            content: None,
            input_tokens: 0,
            output_tokens: 0,
            error: Some(format!("profile `{name}` has no default_model")),
        });
    };
    if !llm_network_enabled() {
        return Ok(ProviderTestResult {
            ok: false,
            provider: Some(name.clone()),
            model: Some(model),
            content: None,
            input_tokens: 0,
            output_tokens: 0,
            error: Some(
                "LLM network is disabled. Enable it for this session from the Models page/status bar, or set LUMO_ALLOW_LLM_NETWORK=1 before launching the app."
                    .into(),
            ),
        });
    }
    let one_off = ProvidersConfig {
        active: Some(name.clone()),
        profiles: vec![profile.clone()],
    };
    let router = AiRouter::from_config(&one_off);
    let req = ChatRequest {
        model: format!("{name}/{model}"),
        messages: vec![ChatMessage::text(
            Role::User,
            prompt.unwrap_or_else(|| "Reply with one word: pong".into()),
        )],
        temperature: Some(0.0),
        max_tokens: Some(64),
        system: None,
    };
    match router.chat(req).await {
        Ok(r) => Ok(ProviderTestResult {
            ok: true,
            provider: Some(r.provider),
            model: Some(r.model),
            content: Some(r.content),
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
            error: None,
        }),
        Err(e) => Ok(ProviderTestResult {
            ok: false,
            provider: Some(name),
            model: Some(model),
            content: None,
            input_tokens: 0,
            output_tokens: 0,
            error: Some(e.to_string()),
        }),
    }
}

/// F-18 Magic Prompt: generate a lumo/v1 flow YAML from a natural-language
/// prompt via the configured LLM (shared `lumo_ai::copilot` core — same as
/// `lumo copilot`). `model` is an optional `<provider>/<model>` override; when
/// absent the active provider's default model is used. Returns validated YAML.
#[tauri::command]
async fn generate_flow(
    app: AppHandle,
    prompt: String,
    model: Option<String>,
) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("prompt is empty".into());
    }
    let home = app_home(&app)?;
    let cfg = ProvidersConfig::load(providers_path(&home)).map_err(|e| e.to_string())?;
    let router = AiRouter::from_config(&cfg);
    if router.provider_names().is_empty() {
        return Err("no LLM provider configured — add one in Settings → Providers first".into());
    }
    if !llm_network_enabled() {
        return Err(
            "LLM network is disabled. Enable it for this session from the Models page/status bar, or set LUMO_ALLOW_LLM_NETWORK=1 before launching the app.".into(),
        );
    }
    let model = match model {
        Some(m) if !m.trim().is_empty() => m,
        _ => {
            let active = cfg.active.clone().ok_or_else(|| {
                "no active provider — set one in Settings → Providers".to_string()
            })?;
            let profile = cfg
                .get(&active)
                .ok_or_else(|| format!("active provider `{active}` not found"))?;
            let dm = profile
                .default_model
                .clone()
                .ok_or_else(|| format!("provider `{active}` has no default model"))?;
            format!("{active}/{dm}")
        }
    };
    lumo_ai::copilot::generate_flow_with_validator(&router, &model, &prompt, 2, |yaml| {
        validate_generated_flow_yaml(&home, yaml)
    })
    .await
}

#[tauri::command]
fn list_skills(app: AppHandle) -> Result<Vec<SkillDto>, String> {
    let home = app_home(&app)?;
    let skills = load_skill_registry(&home, None);
    let manager =
        lumo_agent::SkillManager::new(skills_root(&home)).map_err(|error| error.to_string())?;
    Ok(skills
        .all()
        .into_iter()
        .map(|skill| {
            let active = manager.active(skill.name()).ok().flatten();
            SkillDto {
                name: skill.name().to_string(),
                description: skill.description().map(str::to_string),
                version: skill.frontmatter.version.clone(),
                tags: skill.frontmatter.tags.clone(),
                source: skill.source.display().to_string(),
                hash: active.as_ref().map(|version| version.hash.clone()),
                enabled: active
                    .as_ref()
                    .map(|version| version.enabled)
                    .unwrap_or(true),
            }
        })
        .collect())
}

/// Apply the *panel* alpha (CSS-driven via the slider in Settings → Appearance).
/// This complements `set_window_alpha`, which controls the underlying window
/// background. Kept for backward compatibility with the previous build.
///
/// Orphan: no current JS caller — the live UI drives `set_window_alpha` /
/// `apply_window_alpha` instead. Retained as a thin back-compat alias (the
/// percent→u8 alpha conversion) so older saved configs / external callers
/// don't break; safe to drop once back-compat is no longer a concern.
#[tauri::command]
fn apply_window_appearance(app: AppHandle, options: AppearanceOptions) -> Result<(), String> {
    let alpha = ((options.opacity.min(100) as f32 / 100.0) * 255.0).round() as u8;
    set_window_background(&app, alpha, [255, 255, 255])
}

/// Drive the window background alpha directly (0..=255). This is what the new
/// "整窗透明" slider calls — full range so the window can go all the way to
/// fully transparent (alpha=0) or fully opaque (alpha=255). Optional `rgb`
/// lets a future theme tint the background tone.
#[tauri::command]
fn set_window_alpha(app: AppHandle, options: WindowAlphaOptions) -> Result<(), String> {
    let rgb = options.rgb.unwrap_or([255, 255, 255]);
    set_window_background(&app, options.alpha, rgb)
}

#[tauri::command]
fn recorder_status(state: State<'_, DesktopState>) -> RecorderStatus {
    let slot = state.recorder.lock().unwrap_or_else(|e| e.into_inner());
    match &slot.active {
        Some(session) => RecorderStatus {
            recording: true,
            target: Some(session.target.clone()),
            started_at: Some(session.started_at.to_rfc3339()),
            backend: session.backend.clone(),
            note: format!(
                "Recorder active. Backend: {} · target: {}",
                session.backend, session.target
            ),
        },
        None => RecorderStatus {
            recording: false,
            target: None,
            started_at: None,
            backend: "idle".into(),
            note: "Recorder idle. Choose a target (browser / desktop / mixed) and click 开始录制."
                .into(),
        },
    }
}

#[tauri::command]
async fn recorder_start(
    app: AppHandle,
    state: State<'_, DesktopState>,
    target: Option<String>,
) -> Result<RecorderStatus, String> {
    let target_str = target.unwrap_or_else(|| "browser".to_string());

    // Live event channel: events stream out of the recorder and into a Tauri
    // emit forwarder so the WebView gets them via `listen()` in real time.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<RawEvent>(256);

    let (recorder, backend): (Arc<dyn Recorder>, &'static str) = match target_str.as_str() {
        "browser" => (Arc::new(BrowserRecorder::new()), "BrowserRecorder (CDP)"),
        // desktop & mixed both drive the cross-app DesktopRecorder (R-02):
        // OS-accessibility polling that streams desktop.* events live to the
        // WebView. A combined browser+desktop "mixed" recorder doesn't exist
        // yet, so mixed currently captures the desktop lane only (the browser
        // lane stays available via the dedicated "browser" target).
        "desktop" => (Arc::new(DesktopRecorder::new()), "DesktopRecorder (AX)"),
        "mixed" => (
            Arc::new(DesktopRecorder::new()),
            "DesktopRecorder (AX, mixed→desktop)",
        ),
        _ => (Arc::new(NoopRecorder::new()), "NoopRecorder (heartbeat)"),
    };

    recorder.start(Some(tx)).await.map_err(|e| e.to_string())?;

    let app_for_forwarder = app.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(evt) = rx.recv().await {
            // Best-effort emit — if the WebView is gone the recorder keeps buffering.
            let _ = app_for_forwarder.emit("lumo://recorder-event", &evt);
        }
    });

    let mut slot = state.recorder.lock().unwrap_or_else(|e| e.into_inner());
    if slot.active.is_some() {
        // Race window guard. Tear down what we just started.
        forwarder.abort();
        // We don't have a handle to stop() the recorder cleanly here without
        // moving Arc — leave it as a leaked task; user will see the error and retry.
        return Err("recorder already running — stop it first".into());
    }
    slot.active = Some(RecorderSession {
        recorder,
        started_at: chrono::Utc::now(),
        target: target_str,
        backend: backend.into(),
        forwarder: Some(forwarder),
    });
    drop(slot);
    Ok(recorder_status(state))
}

#[tauri::command]
async fn recorder_stop(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<RecorderStopResult, String> {
    let session = {
        let mut slot = state.recorder.lock().unwrap_or_else(|e| e.into_inner());
        slot.active.take()
    };
    let Some(mut session) = session else {
        return Err("recorder is not running".into());
    };
    if let Some(h) = session.forwarder.take() {
        h.abort();
    }
    let backend_label = session.backend.clone();
    let events = session.recorder.stop().await.map_err(|e| e.to_string())?;
    let yaml_patch = events_to_yaml_patch(&events);
    let element_count = upsert_recorded_elements(&app, &events).unwrap_or(0);
    Ok(RecorderStopResult {
        events: events.len(),
        note: format!(
            "captured {} events from {} backend; {} element(s) saved",
            events.len(),
            backend_label,
            element_count
        ),
        yaml_hint: yaml_patch,
    })
}

#[tauri::command]
fn feature_map() -> Vec<FeatureSection> {
    feature_map_data()
}

// ─── tauri::generate_handler & app entry ────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let updater_pubkey = std::env::var("LUMO_UPDATER_PUBKEY").unwrap_or_default();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(updater_pubkey)
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    handle_global_shortcut(app, &shortcut.to_string(), event.state);
                })
                .build(),
        )
        .manage(DesktopState::default())
        .setup(|app| {
            let handle = app.handle().clone();
            // P2-2：这是 desktop 里唯一允许的非测试 set_var —— 只在 setup 阶段
            // （窗口尚未创建、任何命令/flow 都还没跑）执行一次，供引擎内部无法
            // 显式传 home 的 env 回退路径使用（如 flow 内 OCR 的 cache_root、
            // SkillRegistry::default_root）。运行期一律显式传 home，禁止再写
            // 环境变量（与多线程 getenv 构成数据竞争）。
            if let Ok(home) = app_home(&handle) {
                std::env::set_var("LUMO_HOME", home);
            }
            // P2-3：skills 加载失败的首个 toast 需要一个进程级 AppHandle（构建
            // 注册表的路径深处拿不到 tauri State）。
            let _ = APP_HANDLE.set(handle.clone());
            setup_tray(app)?;
            setup_agent_service(&handle).map_err(|error| anyhow::anyhow!(error))?;
            setup_job_worker(&handle).map_err(|error| anyhow::anyhow!(error))?;
            setup_voice_host(&handle).map_err(|error| anyhow::anyhow!(error))?;
            setup_voice_daemon(&handle).map_err(|error| anyhow::anyhow!(error))?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_examples,
            list_flow_library,
            save_flow_as,
            export_flow_as,
            delete_flow,
            duplicate_flow,
            save_recording_as_flow,
            load_element_library,
            save_element_library,
            inspect_flow,
            read_flow_source,
            reveal_flow_file,
            save_flow_source,
            get_flow_capabilities,
            add_capability_grant,
            validate_flow,
            lint_flow,
            run_flow,
            run_step,
            debug_flow,
            cancel_run,
            human_respond,
            list_runs,
            show_run,
            run_cost,
            list_artifacts,
            read_artifact_blob,
            list_actions,
            action_schema,
            provider_status,
            save_provider,
            remove_provider,
            use_provider,
            init_providers,
            enable_llm_network_for_session,
            list_ocr_models,
            download_ocr_model,
            test_provider,
            generate_flow,
            list_skills,
            skill_import_local,
            skill_import_git,
            skill_versions,
            skill_activate,
            skill_set_enabled,
            skill_validate,
            skill_rollback,
            list_agent_profiles,
            list_improvement_proposals,
            evaluate_improvement,
            approve_improvement,
            reject_improvement,
            rollback_improvement,
            apply_window_appearance,
            set_window_alpha,
            recorder_status,
            recorder_start,
            recorder_stop,
            feature_map,
            list_mcp_servers,
            preview_mcp_import,
            apply_mcp_import,
            test_mcp_server,
            discover_mcp_tools,
            call_mcp_tool,
            delete_mcp_server,
            set_mcp_server_enabled,
            set_mcp_tool_enabled,
            mcp_oauth_start,
            mcp_oauth_callback,
            mcp_oauth_refresh,
            approve_mcp_schema_change,
            security_list,
            security_revoke,
            security_export_audit,
            security_biometric_challenge,
            voice_status,
            voice_configure,
            voice_start_listening,
            voice_stop,
            capsule_expand,
            voice_devices,
            voice_daemon_status,
            voice_daemon_set_enabled,
            voice_daemon_set_muted,
            voice_daemon_suspend,
            voice_daemon_resume,
            voice_daemon_set_device,
            voice_command_history,
            voice_tts_preview,
            agent_start,
            agent_pause,
            agent_resume,
            agent_cancel,
            agent_approve,
            agent_events,
            agent_run_list,
            agent_restore,
            job_list,
            job_schedule,
            job_pause,
            job_resume,
            job_cancel,
            voice_model_install,
            voice_model_remove,
            voice_model_list,
            desktop_update_status,
            desktop_update_check,
            desktop_update_install,
            desktop_update_restart,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LumoRPA desktop");
}

fn setup_tray(app: &mut tauri::App<Wry>) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let show = MenuItem::with_id(&handle, "tray_show", "显示 LumoRPA", true, None::<&str>)?;
    let runs = MenuItem::with_id(&handle, "tray_runs", "运行图表", true, None::<&str>)?;
    let models = MenuItem::with_id(&handle, "tray_models", "模型源", true, None::<&str>)?;
    let settings = MenuItem::with_id(&handle, "tray_settings", "设置", true, None::<&str>)?;
    let hide = MenuItem::with_id(&handle, "tray_hide", "隐藏窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(&handle, "tray_quit", "退出", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(&handle)?;
    let menu = Menu::with_items(
        &handle,
        &[&show, &runs, &models, &settings, &sep, &hide, &quit],
    )?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?.to_owned();

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("LumoRPA")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray_show" => show_main_window(app),
            "tray_runs" => open_main_view(app, "runs"),
            "tray_models" => open_main_view(app, "models"),
            "tray_settings" => open_main_view(app, "settings"),
            "tray_hide" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            "tray_quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(&handle)?;

    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn open_main_view(app: &AppHandle, view: &str) {
    show_main_window(app);
    let _ = app.emit("lumo://open-view", view);
}

#[tauri::command]
fn capsule_expand(app: AppHandle, expanded: bool) -> Result<(), String> {
    if !expanded {
        return Ok(());
    }
    show_main_window(&app);
    app.emit("lumo://open-view", "mission-control")
        .map_err(|error| error.to_string())
}



fn scan_flows_in(dir: &Path, source: &str) -> Vec<FlowSummary> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !is_flow_file(&path) {
            continue;
        }
        let mut s = flow_summary(&path);
        s.source = source.to_string();
        out.push(s);
    }
    // Newest first.
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
    out
}

fn is_flow_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".lumoflow.yaml") || n.ends_with(".lumoflow.yml"))
        .unwrap_or(false)
}

fn flow_summary(path: &Path) -> FlowSummary {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    let updated_ms = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    match lumo_dsl::parse_file(path) {
        Ok(flow) => {
            let validation_error = lumo_dsl::validate(&flow).err().map(|e| e.to_string());
            FlowSummary {
                path: path.display().to_string(),
                file_name,
                id: Some(flow.metadata.id.clone()),
                version: Some(flow.metadata.version.clone()),
                name: flow.metadata.name.clone(),
                description: flow.metadata.description.clone(),
                tags: flow.metadata.tags.clone(),
                inputs: io_dtos(&flow.spec.inputs),
                outputs: io_dtos(&flow.spec.outputs),
                step_count: count_steps(&flow.spec.steps),
                valid: validation_error.is_none(),
                error: validation_error,
                source: "user".into(),
                updated_ms,
            }
        }
        Err(e) => FlowSummary {
            path: path.display().to_string(),
            file_name,
            id: None,
            version: None,
            name: None,
            description: None,
            tags: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            step_count: 0,
            valid: false,
            error: Some(e.to_string()),
            source: "user".into(),
            updated_ms,
        },
    }
}

fn parse_and_validate(home: &Path, flow_path: &Path) -> Result<Flow, String> {
    let flow = lumo_dsl::parse_file(flow_path).map_err(|e| e.to_string())?;
    lumo_dsl::validate(&flow).map_err(|e| e.to_string())?;
    // P2-3：注册表与 skills 走同一次缓存查询（此前各自 load_dir 扫两遍）。
    let (registry, skills, skill_load_errors) = cached_registry(home, Some(flow_path));
    // P1-7:步级校验上移到 lumo-core 的单一实现(原桌面侧副本已删),与 CLI 共用。
    lumo_core::validate_steps(
        &flow.spec.steps,
        &flow.spec.capabilities,
        &registry,
        &|name| skills.get(name).is_some(),
    )
    .map_err(|e| attach_skill_load_errors(e.to_string(), &skill_load_errors))?;
    Ok(flow)
}

fn validate_generated_flow_yaml(home: &Path, yaml: &str) -> Result<(), String> {
    if yaml.trim().is_empty() {
        return Err("empty YAML".into());
    }
    let flow = lumo_dsl::parse_str(yaml).map_err(|e| format!("parse: {e}"))?;
    lumo_dsl::validate(&flow).map_err(|e| format!("validate: {e}"))?;
    let (registry, skills, skill_load_errors) = cached_registry(home, None);
    lumo_core::validate_steps(
        &flow.spec.steps,
        &flow.spec.capabilities,
        &registry,
        &|name| skills.get(name).is_some(),
    )
    .map_err(|e| attach_skill_load_errors(e.to_string(), &skill_load_errors))?;
    Ok(())
}

fn extract_step<'a>(steps: &'a [Step], id: &str) -> Option<&'a Step> {
    for step in steps {
        if step.id == id {
            return Some(step);
        }
        for child in step.children() {
            if let Some(found) = extract_step(child, id) {
                return Some(found);
            }
        }
    }
    None
}




fn parse_inputs(raw: &str) -> Result<Value, String> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("inputs JSON: {e}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err("inputs JSON must be an object".into())
    }
}





fn make_provider_status(path: &Path, cfg: &ProvidersConfig) -> ProviderStatus {
    ProviderStatus {
        path: path.display().to_string(),
        active: cfg.active.clone(),
        profiles: cfg
            .profiles
            .iter()
            .map(|p| ProviderProfileDto {
                name: p.name.clone(),
                kind: p.kind.clone(),
                wire_api: p.wire_api.clone(),
                default_model: p.default_model.clone(),
                vision_model: p.vision_model.clone(),
                ocr_model: p.ocr_model.clone(),
                base_url: p.base_url.clone(),
                api_key_env: p.api_key_env.clone(),
                has_inline_key: p.api_key.is_some(),
                has_key: p.kind == "local" || p.resolve_api_key().is_some(),
                reasoning_effort: p.reasoning_effort.clone(),
                models: p.models.clone(),
                headers: p.headers.clone(),
                notes: p.notes.clone(),
            })
            .collect(),
        network_enabled: llm_network_enabled(),
    }
}

fn set_window_background(app: &AppHandle, alpha: u8, rgb: [u8; 3]) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_background_color(Some(tauri::webview::Color(rgb[0], rgb[1], rgb[2], alpha)))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn llm_network_enabled() -> bool {
    lumo_ai::llm_network_allowed()
}

fn looks_like_env_var_name(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}







