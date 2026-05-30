use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter, ChatAction,
};
use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_dsl::{Flow, IoDecl, Step};
use lumo_recorder::{events_to_yaml_patch, BrowserRecorder, NoopRecorder, RawEvent, Recorder};
use lumo_skills::{register_skill_actions, SkillRegistry};
use lumo_storage::{FlowRunRow, Repo, StepRunRow};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State, Wry};

type AppHandle = tauri::AppHandle<Wry>;

// ─── Shared mutable state ───────────────────────────────────────────────────

#[derive(Default)]
struct DesktopState {
    recorder: Mutex<RecorderSlot>,
}

#[derive(Default)]
struct RecorderSlot {
    active: Option<RecorderSession>,
}

struct RecorderSession {
    recorder: Arc<dyn Recorder>,
    started_at: chrono::DateTime<chrono::Utc>,
    target: String,
    backend: String,
    forwarder: Option<tokio::task::JoinHandle<()>>,
}

// ─── DTOs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: String,
    data_dir: String,
    resource_dir: Option<String>,
    examples_dir: Option<String>,
    providers_path: String,
    skills_path: String,
    platform: String,
    arch: String,
    network_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IoDeclDto {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    required: bool,
    default: Option<Value>,
    description: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlowSummary {
    path: String,
    file_name: String,
    id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    inputs: Vec<IoDeclDto>,
    outputs: Vec<IoDeclDto>,
    step_count: usize,
    valid: bool,
    error: Option<String>,
    /// `"user"` (saved by the operator) / `"recording"` (recorder output)
    /// / `"example"` (bundled). Defaults to `"user"` when scanned via the
    /// bare flow_summary helper; the library scanner overrides per source.
    #[serde(default)]
    source: String,
    /// File modification time as a unix-ms timestamp. Lets the library sort
    /// recently-touched flows to the top.
    #[serde(default)]
    updated_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ValidationReport {
    path: String,
    id: String,
    version: String,
    name: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    inputs: Vec<IoDeclDto>,
    outputs: Vec<IoDeclDto>,
    capabilities: Value,
    step_count: usize,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActionDto {
    id: String,
    family: String,
    summary: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunReportDto {
    run_id: String,
    success: bool,
    steps_total: usize,
    steps_ok: usize,
    steps_executed: usize,
    steps_failed: usize,
    steps_skipped: usize,
    steps_retried: usize,
    steps_caught: usize,
    duration_ms: u128,
    outputs: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunDto {
    id: String,
    flow_id: String,
    flow_version: String,
    trigger_kind: String,
    inputs: Value,
    outputs: Option<Value>,
    state: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration_ms: Option<i64>,
    cost_token: i64,
    cost_usd_micro: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StepRunDto {
    seq: i64,
    path: String,
    parent_path: Option<String>,
    depth: i64,
    step_id: String,
    idx: i64,
    state: String,
    attempt: i64,
    output_json: Option<Value>,
    error: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
    duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunResponse {
    report: RunReportDto,
    run: Option<RunDto>,
    steps: Vec<StepRunDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RunDetail {
    run: RunDto,
    steps: Vec<StepRunDto>,
}

/// X-07 Time-Travel: a single artifact blob streamed back to the webview as a
/// base64 data URL so `<img>` / `<iframe>` can render it directly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactBlobDto {
    id: String,
    mime: String,
    data_url: String,
    size: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderProfileDto {
    name: String,
    kind: String,
    wire_api: Option<String>,
    default_model: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    has_inline_key: bool,
    has_key: bool,
    reasoning_effort: Option<String>,
    models: Vec<String>,
    headers: BTreeMap<String, String>,
    notes: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderStatus {
    path: String,
    active: Option<String>,
    profiles: Vec<ProviderProfileDto>,
    network_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInput {
    name: String,
    kind: String,
    #[serde(default)]
    wire_api: Option<String>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    /// When true, mark this profile as active after upsert.
    #[serde(default)]
    activate: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderTestResult {
    ok: bool,
    provider: Option<String>,
    model: Option<String>,
    content: Option<String>,
    input_tokens: u32,
    output_tokens: u32,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillDto {
    name: String,
    description: Option<String>,
    version: Option<String>,
    tags: Vec<String>,
    source: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppearanceOptions {
    /// Panel alpha (0-100, percentage applied to white).
    opacity: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowAlphaOptions {
    /// 0..=255 alpha applied to the window background color. 0 = fully clear,
    /// 255 = fully opaque. Sliders use the full range.
    alpha: u8,
    /// Optional tinted background color (RGB). Defaults to white-ish so the
    /// platform vibrancy is preserved.
    #[serde(default)]
    rgb: Option<[u8; 3]>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecorderStatus {
    recording: bool,
    target: Option<String>,
    started_at: Option<String>,
    backend: String,
    note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecorderStopResult {
    events: usize,
    note: String,
    yaml_hint: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureStatus {
    id: String,
    title: String,
    stage: String,
    status: String, // "ready" | "partial" | "planned"
    note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeatureSection {
    id: String,
    title: String,
    items: Vec<FeatureStatus>,
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

/// Wrap the recorder's `events_to_yaml_patch` fragment into a complete
/// LumoFlow doc so the user can hit ▶ on the result without hand-editing.
/// The fragment lives under `spec.steps`; everything else is reasonable
/// defaults that the user can tighten later.
fn wrap_recording_fragment(name: &str, fragment: &str) -> String {
    let id = sanitize_flow_name(name);
    let id = if id.is_empty() {
        "recording".into()
    } else {
        id
    };
    // Re-indent the fragment so it sits two spaces in under `spec.steps:`.
    let body: String = fragment
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') || l.contains("Recorder"))
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "apiVersion: lumorpa.io/v1\nkind: Flow\nmetadata:\n  id: {id}\n  version: 0.1.0\n  name: 录制 · {id}\n  tags: [recording]\nspec:\n  capabilities:\n    network: [\"*\"]\n  steps:\n{body}\n"
    )
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
    let body = wrap_recording_fragment(&stem, &yaml_hint);
    std::fs::write(&candidate, body).map_err(|e| format!("write {}: {e}", candidate.display()))?;
    Ok(candidate.display().to_string())
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
    path: String,
    inputs_json: String,
    no_store: bool,
) -> Result<RunResponse, String> {
    let home = app_home(&app)?;
    let flow_path = Path::new(&path);
    let flow = parse_and_validate(&home, flow_path)?;
    let inputs = parse_inputs(&inputs_json)?;
    execute_flow(&home, Some(flow_path), flow, inputs, no_store).await
}

/// Run a single step from the flow by `step_id`. The step is extracted from
/// the flow (including its nested children for control-flow steps) and wrapped
/// in an ad-hoc flow that preserves the original metadata + capability set so
/// validation still passes. Used by the Studio "▶ run this step" affordance —
/// one of the differentiators against 影刀 (which forces top-to-bottom runs).
#[tauri::command]
async fn run_step(
    app: AppHandle,
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
    execute_flow(&home, Some(flow_path), flow, inputs, no_store).await
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
    let p = ProviderProfile {
        name: profile.name,
        kind: profile.kind,
        wire_api: profile.wire_api,
        base_url: profile.base_url,
        api_key: profile.api_key.filter(|s| !s.is_empty()),
        api_key_env: profile.api_key_env.filter(|s| !s.is_empty()),
        default_model: profile.default_model.filter(|s| !s.is_empty()),
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
                "LLM network is disabled. Set LUMO_ALLOW_LLM_NETWORK=1 before launching the app."
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

#[tauri::command]
fn list_skills(app: AppHandle) -> Result<Vec<SkillDto>, String> {
    let home = app_home(&app)?;
    let skills = load_skill_registry(&home, None);
    Ok(skills
        .all()
        .into_iter()
        .map(|skill| SkillDto {
            name: skill.name().to_string(),
            description: skill.description().map(str::to_string),
            version: skill.frontmatter.version.clone(),
            tags: skill.frontmatter.tags.clone(),
            source: skill.source.display().to_string(),
        })
        .collect())
}

/// Apply the *panel* alpha (CSS-driven via the slider in Settings → Appearance).
/// This complements `set_window_alpha`, which controls the underlying window
/// background. Kept for backward compatibility with the previous build.
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
        // desktop & mixed land in a follow-up — fall back to noop heartbeat
        // so the UI still gets life signs from the recorder.
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
async fn recorder_stop(state: State<'_, DesktopState>) -> Result<RecorderStopResult, String> {
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
    Ok(RecorderStopResult {
        events: events.len(),
        note: format!(
            "captured {} events from {} backend",
            events.len(),
            backend_label
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
    tauri::Builder::default()
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_examples,
            list_flow_library,
            save_flow_as,
            delete_flow,
            duplicate_flow,
            save_recording_as_flow,
            inspect_flow,
            read_flow_source,
            save_flow_source,
            get_flow_capabilities,
            add_capability_grant,
            validate_flow,
            lint_flow,
            run_flow,
            run_step,
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
            test_provider,
            list_skills,
            apply_window_appearance,
            set_window_alpha,
            recorder_status,
            recorder_start,
            recorder_stop,
            feature_map,
        ])
        .run(tauri::generate_context!())
        .expect("error while running LumoRPA desktop");
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn app_home(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Roots the webview is allowed to *read* flow files from: the user's
/// LUMO_HOME (user flows + recordings + artifacts) and the read-only bundled
/// examples directory. Each is canonicalized; unreadable roots are skipped (P0-3).
fn flow_read_roots(app: &AppHandle) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = app_home(app) {
        if let Ok(canon) = home.canonicalize() {
            roots.push(canon);
        }
    }
    if let Some(ex) = examples_dir(app) {
        if let Ok(canon) = ex.canonicalize() {
            roots.push(canon);
        }
    }
    roots
}

/// Canonicalize `requested` and confirm it resolves inside one of `roots`
/// (each already canonicalized). The path must exist. Confines webview-driven
/// file *reads* to the flow library + examples so a crafted `..`/symlink path
/// can't exfiltrate arbitrary files (P0-3).
fn resolve_within(requested: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let canonical = Path::new(requested)
        .canonicalize()
        .map_err(|e| format!("resolve {requested}: {e}"))?;
    if roots.iter().any(|root| canonical.starts_with(root)) {
        Ok(canonical)
    } else {
        Err(format!(
            "refused: {} is outside the allowed flow directories",
            canonical.display()
        ))
    }
}

/// Resolve a *write* target for `requested`, confining it to `home`
/// (LUMO_HOME). The file need not exist yet, so its parent directory is
/// canonicalized (it must exist and resolve under `home`) and the file name is
/// re-appended. Bundled examples live outside `home` and are thus read-only (P0-3).
fn resolve_write_within(requested: &str, home: &Path) -> Result<PathBuf, String> {
    let requested_path = Path::new(requested);
    let file_name = requested_path
        .file_name()
        .ok_or_else(|| format!("invalid write path: {requested}"))?;
    let parent = requested_path.parent().unwrap_or_else(|| Path::new(""));
    let home_canon = home
        .canonicalize()
        .map_err(|e| format!("resolve LUMO_HOME: {e}"))?;
    let parent_canon = if parent.as_os_str().is_empty() {
        home_canon.clone()
    } else {
        parent
            .canonicalize()
            .map_err(|e| format!("resolve {}: {e}", parent.display()))?
    };
    if !parent_canon.starts_with(&home_canon) {
        return Err(format!(
            "refused: {} is outside LUMO_HOME",
            parent_canon.display()
        ));
    }
    Ok(parent_canon.join(file_name))
}

/// A `FlowSummary` for a path the webview isn't allowed to read — surfaced as
/// an invalid entry instead of leaking file metadata for arbitrary paths (P0-3).
fn refused_summary(path: &str, reason: String) -> FlowSummary {
    FlowSummary {
        path: path.to_string(),
        file_name: Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_default(),
        valid: false,
        error: Some(reason),
        ..Default::default()
    }
}

fn open_repo(app: &AppHandle) -> Result<Repo, String> {
    Repo::open(app_home(app)?.join("lumo.db")).map_err(|e| e.to_string())
}

fn examples_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("examples");
        if bundled.exists() {
            return Some(bundled);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../examples");
    if dev.exists() {
        return Some(dev);
    }
    None
}

/// User-owned flows. Lives under `$LUMO_HOME/flows`, created on first save.
fn user_flows_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_home(app)?.join("flows");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Recorder output drop zone. Each `recorder_stop_and_save` call writes one
/// `.lumoflow.yaml` here so the user can pick it up from the library.
fn recordings_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_home(app)?.join("recordings");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
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
    let registry = build_action_registry(home, Some(flow_path));
    let skills = load_skill_registry(home, Some(flow_path));
    validate_steps(
        &flow.spec.steps,
        &flow.spec.capabilities,
        &registry,
        &skills,
    )
    .map_err(|e| e.to_string())?;
    Ok(flow)
}

async fn execute_flow(
    home: &Path,
    flow_path: Option<&Path>,
    flow: Flow,
    inputs: Value,
    no_store: bool,
) -> Result<RunResponse, String> {
    let registry = build_action_registry(home, flow_path);
    let repo = if no_store {
        None
    } else {
        Some(Repo::open(home.join("lumo.db")).map_err(|e| e.to_string())?)
    };
    let vm = FlowVm::new(registry, repo.clone());
    // P0-1: attach AI hooks (heal / extract_visual / decide / diagnose) when the
    // flow enables AI and providers are configured; otherwise the VM stays
    // deterministic. Mirrors the CLI's `attach_ai_hooks`.
    let ai = flow.metadata.ai.clone().unwrap_or_default();
    let ai_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let vm = match lumo_ai::build_hook_provider(&ai_cfg, ai.enabled, ai.budget.max_calls_per_run) {
        Some(provider) => vm.with_ai_provider(provider),
        None => vm,
    };
    let report = vm
        .run(
            &flow,
            RunOptions {
                inputs,
                trigger_kind: "desktop".into(),
            },
        )
        .await
        .map_err(|e| e.to_string())?;

    let run = repo
        .as_ref()
        .and_then(|r| r.get_run(&report.run_id).ok().flatten())
        .map(run_dto);
    let steps = repo
        .as_ref()
        .and_then(|r| r.list_steps(&report.run_id).ok())
        .unwrap_or_default()
        .into_iter()
        .map(step_dto)
        .collect();

    Ok(RunResponse {
        report: report_dto(report),
        run,
        steps,
    })
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

fn validation_report(path: &str, flow: &Flow) -> ValidationReport {
    let warnings = if flow_uses_action(&flow.spec.steps, "ai.chat") {
        vec!["This flow uses ai.chat; configure providers.toml and the corresponding API key environment variables before running it.".into()]
    } else {
        Vec::new()
    };

    ValidationReport {
        path: path.to_string(),
        id: flow.metadata.id.clone(),
        version: flow.metadata.version.clone(),
        name: flow.metadata.name.clone(),
        description: flow.metadata.description.clone(),
        tags: flow.metadata.tags.clone(),
        inputs: io_dtos(&flow.spec.inputs),
        outputs: io_dtos(&flow.spec.outputs),
        capabilities: serde_json::to_value(&flow.spec.capabilities).unwrap_or(Value::Null),
        step_count: count_steps(&flow.spec.steps),
        warnings,
    }
}

fn io_dtos(items: &[IoDecl]) -> Vec<IoDeclDto> {
    items
        .iter()
        .map(|item| IoDeclDto {
            name: item.name.clone(),
            kind: item.ty.clone(),
            required: item.required,
            default: item
                .default
                .as_ref()
                .and_then(|v| serde_json::to_value(v).ok()),
            description: item.description.clone(),
        })
        .collect()
}

fn count_steps(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|step| 1 + step.children().into_iter().map(count_steps).sum::<usize>())
        .sum()
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

fn report_dto(report: lumo_core::RunReport) -> RunReportDto {
    RunReportDto {
        run_id: report.run_id,
        success: report.success,
        steps_total: report.steps_total,
        steps_ok: report.steps_ok,
        steps_executed: report.steps_executed,
        steps_failed: report.steps_failed,
        steps_skipped: report.steps_skipped,
        steps_retried: report.steps_retried,
        steps_caught: report.steps_caught,
        duration_ms: report.duration_ms,
        outputs: report.outputs,
    }
}

fn run_dto(row: FlowRunRow) -> RunDto {
    let duration_ms = match (&row.started_at, &row.finished_at) {
        (Some(started), Some(finished)) => {
            Some(finished.timestamp_millis() - started.timestamp_millis())
        }
        _ => None,
    };
    RunDto {
        id: row.id,
        flow_id: row.flow_id,
        flow_version: row.flow_version,
        trigger_kind: row.trigger_kind,
        inputs: row.inputs,
        outputs: row.outputs,
        state: row.state,
        started_at: row.started_at.map(|t| t.to_rfc3339()),
        finished_at: row.finished_at.map(|t| t.to_rfc3339()),
        duration_ms,
        cost_token: row.cost_token,
        cost_usd_micro: row.cost_usd_micro,
    }
}

fn step_dto(row: StepRunRow) -> StepRunDto {
    let duration_ms = match (&row.started_at, &row.finished_at) {
        (Some(started), Some(finished)) => {
            Some(finished.timestamp_millis() - started.timestamp_millis())
        }
        _ => None,
    };
    StepRunDto {
        seq: row.seq,
        path: row.path,
        parent_path: row.parent_path,
        depth: row.depth,
        step_id: row.step_id,
        idx: row.idx,
        state: row.state,
        attempt: row.attempt,
        output_json: row.output_json,
        error: row.error,
        started_at: row.started_at.map(|t| t.to_rfc3339()),
        finished_at: row.finished_at.map(|t| t.to_rfc3339()),
        duration_ms,
    }
}

fn providers_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_PROVIDERS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("providers.toml"))
}

fn skills_root(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_SKILLS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("skills"))
}

fn build_action_registry(home: &Path, flow_path: Option<&Path>) -> ActionRegistry {
    let providers_cfg = ProvidersConfig::load(providers_path(home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));

    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(router));

    let skill_reg = load_skill_registry(home, flow_path);
    register_skill_actions(&mut registry, skill_reg);
    registry
}

fn load_skill_registry(home: &Path, flow_path: Option<&Path>) -> Arc<SkillRegistry> {
    let skill_reg = Arc::new(SkillRegistry::new());
    let _ = skill_reg.load_dir(skills_root(home));
    if let Some(flow_path) = flow_path {
        if let Some(flow_dir) = flow_path.parent() {
            let _ = skill_reg.load_dir(flow_dir.join("skills"));
        }
    }
    skill_reg
}

fn validate_steps(
    steps: &[Step],
    capabilities: &lumo_dsl::Capabilities,
    registry: &ActionRegistry,
    skills: &Arc<SkillRegistry>,
) -> anyhow::Result<()> {
    for step in steps {
        let action = registry.get(&step.action).ok_or_else(|| {
            anyhow::anyhow!("unknown action `{}` in step `{}`", step.action, step.id)
        })?;
        validate_capability_declaration(step, capabilities)?;
        let input = serde_json::to_value(&step.with).unwrap_or(Value::Null);
        validate_schema(&step.id, &step.action, &input, action.schema())?;
        validate_skill_reference(step, &input, skills)?;
        for children in step.children() {
            validate_steps(children, capabilities, registry, skills)?;
        }
    }
    Ok(())
}

fn validate_capability_declaration(
    step: &Step,
    capabilities: &lumo_dsl::Capabilities,
) -> anyhow::Result<()> {
    let missing = match step.action.as_str() {
        "file.read" | "file.exists" | "excel.read_rows" if capabilities.fs_read.is_empty() => {
            Some("fs.read")
        }
        "file.write" | "excel.write_row" if capabilities.fs_write.is_empty() => Some("fs.write"),
        "http.request" | "browser.open" if capabilities.network.is_empty() => Some("network"),
        "ai.chat" if capabilities.llm.is_empty() => Some("llm"),
        _ => None,
    };
    if let Some(kind) = missing {
        anyhow::bail!(
            "step `{}` action `{}` requires spec.capabilities.{kind}",
            step.id,
            step.action
        );
    }
    Ok(())
}

fn validate_skill_reference(
    step: &Step,
    input: &Value,
    skills: &Arc<SkillRegistry>,
) -> anyhow::Result<()> {
    if step.action != "skill.invoke" {
        return Ok(());
    }
    let Some(name) = input.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    if is_template_string(name) {
        return Ok(());
    }
    if skills.get(name).is_none() {
        anyhow::bail!("step `{}` invokes unknown skill `{name}`", step.id);
    }
    Ok(())
}

fn validate_schema(
    step_id: &str,
    action_id: &str,
    input: &Value,
    schema: &Value,
) -> anyhow::Result<()> {
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        let Some(input_obj) = input.as_object() else {
            anyhow::bail!("step `{step_id}` action `{action_id}` with: must be an object");
        };
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !input_obj.contains_key(key) {
                    anyhow::bail!(
                        "step `{step_id}` action `{action_id}` missing required with.{key}"
                    );
                }
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
            for key in input_obj.keys() {
                if !properties
                    .map(|props| props.contains_key(key))
                    .unwrap_or(false)
                {
                    anyhow::bail!("step `{step_id}` action `{action_id}` has unknown with.{key}");
                }
            }
        }
        if let Some(properties) = properties {
            for (key, value) in input_obj {
                if let Some(prop_schema) = properties.get(key) {
                    validate_value_type(step_id, action_id, key, value, prop_schema)?;
                }
            }
        }
    }
    Ok(())
}

fn validate_value_type(
    step_id: &str,
    action_id: &str,
    key: &str,
    value: &Value,
    schema: &Value,
) -> anyhow::Result<()> {
    if value.as_str().map(is_template_string).unwrap_or(false) {
        return Ok(());
    }
    let Some(expected) = schema.get("type") else {
        return Ok(());
    };
    let ok = match expected {
        Value::String(s) => json_type_matches(s, value),
        Value::Array(types) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|s| json_type_matches(s, value)),
        _ => true,
    };
    if !ok {
        anyhow::bail!(
            "step `{step_id}` action `{action_id}` with.{key} expected {}, got {}",
            expected,
            json_kind(value)
        );
    }
    Ok(())
}

fn json_type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn is_template_string(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

fn flow_uses_action(steps: &[Step], action_id: &str) -> bool {
    steps.iter().any(|step| {
        step.action == action_id
            || step
                .children()
                .into_iter()
                .any(|children| flow_uses_action(children, action_id))
    })
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
                base_url: p.base_url.clone(),
                api_key_env: p.api_key_env.clone(),
                has_inline_key: p.api_key.is_some(),
                has_key: p.resolve_api_key().is_some(),
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
    matches!(
        std::env::var("LUMO_ALLOW_LLM_NETWORK").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes")
    )
}

/// Hard-coded snapshot of the implementation status of the design-doc feature
/// matrix. This is what the Studio "feature map" panel renders so the user can
/// see exactly which docs/01-Product-Design items are wired up vs. planned.
fn feature_map_data() -> Vec<FeatureSection> {
    fn item(id: &str, title: &str, stage: &str, status: &str, note: &str) -> FeatureStatus {
        FeatureStatus {
            id: id.into(),
            title: title.into(),
            stage: stage.into(),
            status: status.into(),
            note: note.into(),
        }
    }

    vec![
        FeatureSection {
            id: "design".into(),
            title: "流程设计 (D)".into(),
            items: vec![
                item(
                    "D-01",
                    "节点视图 + 表单参数",
                    "M1",
                    "ready",
                    "动作库可拖入画布，schema 自动生成属性表单。",
                ),
                item(
                    "D-02",
                    "流程图视图 (DAG)",
                    "M1",
                    "ready",
                    "Graph 视图基于 SVG 节点 + 折线连接。",
                ),
                item(
                    "D-03",
                    "代码视图 (YAML)",
                    "M1",
                    "ready",
                    "Code 视图行号 + 简易高亮。",
                ),
                item(
                    "D-04",
                    "三向同构实时同步",
                    "M1",
                    "ready",
                    "Graph / Tree / Code 共享同一 AST。",
                ),
                item(
                    "D-05",
                    "变量面板",
                    "M1",
                    "partial",
                    "inputs JSON 编辑 + outputs 展示。",
                ),
                item(
                    "D-06",
                    "子流程 / 参数化",
                    "M1",
                    "planned",
                    "lumo-dsl 当前不展开子流程导入。",
                ),
                item(
                    "D-07",
                    "Try / Catch / Finally",
                    "M1",
                    "ready",
                    "DSL + VM 已支持 try.catch.finally.",
                ),
                item(
                    "D-08",
                    "重试策略",
                    "M1",
                    "ready",
                    "retry.times/backoff/on 已在 VM 实现。",
                ),
                item(
                    "D-09",
                    "条件分支 / 循环",
                    "M1",
                    "ready",
                    "control.if / control.for / control.for_each / control.break.",
                ),
                item(
                    "D-10",
                    "并行块",
                    "M2",
                    "ready",
                    "★ control.parallel branches: [[steps], ...] 真并发执行（futures::join_all），StepCtx Arc<Mutex> 共享变量绑定；back-compat：do: [step,...] 每项作为单步分支。examples/parallel-demo.lumoflow.yaml.",
                ),
                item(
                    "D-11",
                    "注释 / 折叠 / 标签",
                    "M1",
                    "partial",
                    "Tree 视图可折叠；标签来自 metadata.tags.",
                ),
                item(
                    "D-12",
                    "任意节点级单步运行",
                    "M1",
                    "ready",
                    "★ 影刀短板：run_step 命令落地。",
                ),
                item(
                    "D-13",
                    "断点 / 条件断点",
                    "M1",
                    "planned",
                    "M2 计划在 VM 加入断点 hook.",
                ),
                item(
                    "D-15",
                    "自然语言生成节点 / 整段流程",
                    "M2",
                    "planned",
                    "AI Copilot 入口预留。",
                ),
            ],
        },
        FeatureSection {
            id: "recorder".into(),
            title: "录制器 (R)".into(),
            items: vec![
                item(
                    "R-01",
                    "Web 录制 (CDP)",
                    "M2",
                    "ready",
                    "★ BrowserRecorder + CDP Runtime.addBinding 注入 JS 钩子，捕获 click/input/change/keydown，附 CSS+XPath+a11y 标签。导航/心跳并存。",
                ),
                item(
                    "R-02",
                    "桌面录制 (Windows UIA)",
                    "M1",
                    "planned",
                    "AccessKit 桥接待补.",
                ),
                item(
                    "R-05",
                    "智能录制 (自动判别)",
                    "M1",
                    "planned",
                    "归并/抖动算法尚未实现.",
                ),
                item(
                    "R-08",
                    "事件去抖 / 合并",
                    "M2",
                    "ready",
                    "★ ActionBuffer 200ms 同 selector 输入合并 + 三档跨事件抑制:click→input 焦点丢弃(<250ms)、input→change blur 回声丢弃(<500ms)、近距 dblclick 折叠(<60ms);7 个新测试覆盖正负路径.",
                ),
                item(
                    "R-09",
                    "相似元素一键抓取",
                    "M2",
                    "ready",
                    "★ Alt+点击触发同款泛化：注入 JS 比对父节点同 tag + 80% 共有 class，生成 `parent > tag.class` 选择器，YAML patch 直出 browser.extract { all: true } + 兄弟数注释。",
                ),
                item(
                    "R-10",
                    "录制→YAML patch",
                    "M2",
                    "ready",
                    "★ events_to_yaml_patch 把录制流转成可粘贴的 browser.open/click/type 步骤；recorder_stop 直接返回。",
                ),
            ],
        },
        FeatureSection {
            id: "selectors".into(),
            title: "选择器 / Self-Healing (S)".into(),
            items: vec![
                item(
                    "S-01",
                    "CSS 选择器",
                    "M1",
                    "ready",
                    "browser.click / type 接受 selector (CSS) 或 selectors 多策略对象，二者择一。",
                ),
                item(
                    "S-02",
                    "XPath",
                    "M2",
                    "ready",
                    "★ selectors.xpath 走 document.evaluate；与 CSS / aria-label / text 共用 Self-Healing 回退。",
                ),
                item(
                    "S-06",
                    "智能多策略选择器",
                    "M2",
                    "ready",
                    "★ Self-Healing Router 完整落地：6 策略 (id/data-testid/css/aria-label/text/xpath)，按 base_cost × history_penalty 动态排序，每次解析记录 resolved_by 与 tried 列表，下一轮自动收益。Vision-LLM 后续 plug-in。",
                ),
                item(
                    "S-11",
                    "Vision-LLM 自愈",
                    "M2",
                    "partial",
                    "★ AI 层传输完成:`ChatMessage.attachments: Vec<ImageAttachment>` + base64/URL 双源 + Anthropic/OpenAI 双 wire 编码(`image_url` / `image` block)+ 7 个 vision 测试.OmniParser/UI-TARS 端到端注入选择器路由仍排期 M3.",
                ),
                item(
                    "S-12",
                    "Set-of-Mark 兜底",
                    "M2",
                    "partial",
                    "传输层就绪(可向 vision 模型发送截图);Set-of-Mark 标注 / 视觉坐标 → DOM 元素的反查机制排期 M3.",
                ),
            ],
        },
        FeatureSection {
            id: "browser".into(),
            title: "浏览器 (B)".into(),
            items: vec![
                item(
                    "B-01",
                    "Chromium CDP",
                    "M1",
                    "ready",
                    "lumo-actions::browser 已经接 chromiumoxide.",
                ),
                item(
                    "B-04",
                    "多 Tab / Context",
                    "M1",
                    "partial",
                    "browser.open / close 已就绪.",
                ),
                item(
                    "B-05",
                    "click / type / hover / scroll / upload / download",
                    "M1",
                    "ready",
                    "首发动作集已覆盖核心交互.",
                ),
                item(
                    "B-07",
                    "表格抓取",
                    "M1",
                    "partial",
                    "browser.extract 支持 map 字段.",
                ),
                item(
                    "B-11",
                    "Headless / Headed 切换",
                    "M1",
                    "ready",
                    "browser.launch 支持 headless 标志.",
                ),
                item(
                    "B-12",
                    "Stealth 反指纹",
                    "M2",
                    "planned",
                    "Patchright 思路排期 M2.",
                ),
            ],
        },
        FeatureSection {
            id: "office".into(),
            title: "Office / 文档 (O)".into(),
            items: vec![
                item(
                    "O-01",
                    "Excel 读写",
                    "M1",
                    "ready",
                    "excel.read_rows / write_row 已实现.",
                ),
                item(
                    "O-03",
                    "Polars DataFrame Action",
                    "M1",
                    "partial",
                    "data.* 系列动作初版.",
                ),
                item(
                    "O-08",
                    "Excel 行驱动循环",
                    "M1",
                    "ready",
                    "★ 影刀招牌场景；examples/excel-loop.lumoflow.yaml.",
                ),
                item(
                    "O-13",
                    "OCR (PaddleOCR 3.0)",
                    "M2",
                    "planned",
                    "本地视觉模型排期 M2.",
                ),
            ],
        },
        FeatureSection {
            id: "ai".into(),
            title: "AI 节点 (A)".into(),
            items: vec![
                item(
                    "A-01",
                    "LLM 节点 (多 provider)",
                    "M1",
                    "ready",
                    "ai.chat + ProvidersConfig + Anthropic/OpenAI 适配.",
                ),
                item(
                    "A-02",
                    "Embedding / 向量检索",
                    "M2",
                    "planned",
                    "libSQL F32_BLOB 待启用.",
                ),
                item(
                    "A-05",
                    "屏幕理解 (OmniParser v2)",
                    "M2",
                    "planned",
                    "本地视觉路线.",
                ),
                item(
                    "A-07",
                    "Computer Use 节点",
                    "M2",
                    "planned",
                    "Claude / Gemini CU 适配.",
                ),
                item(
                    "A-13",
                    "自然语言生成流程",
                    "M2",
                    "ready",
                    "★ `lumo copilot \"...\"` 子命令通过 AiRouter 生成 lumo/v1 YAML 草稿,内置 system prompt 含 schema/合法 action id 列表;parse+validate 失败自动重试一次并把错误带回提示;支持 --out / --dry-run / --model 覆盖.",
                ),
                item(
                    "A-14",
                    "Self-Healing Router",
                    "M2",
                    "ready",
                    "★ 双层学习:per-strategy 成功率(`history_penalty` 1-3 倍成本)+ per-(prev→next) 转移概率(`transition_score` 0-1);贪心选择 `cost(s)/(1+5×score(prev→s))` 把验证过的恢复策略提到第二位即使基础成本更高;`resolve_element` 自动记录 last_failed→winner 转移;选择器统计已 JSON 持久化.Vision-LLM 端点排期 M3.",
                ),
            ],
        },
        FeatureSection {
            id: "triggers".into(),
            title: "触发 / 调度 (T)".into(),
            items: vec![
                item(
                    "T-01",
                    "Cron",
                    "M2",
                    "ready",
                    "★ `lumo serve` 启动时扫 --flows 目录，spec.triggers.[kind: cron, with: { schedule: \"0 */5 * * * *\" }] 每个触发器起独立 tokio 任务，按 schedule 睡到下一次 fire，run 走 lumo.db 持久化（trigger_kind=cron）。每次 fire 重新 parse flow，编辑后无需重启。",
                ),
                item("T-02", "文件触发", "M2", "ready", "★ `lumo serve` 同进程内 spawn `notify` watcher;`triggers.[kind:file, with:{path, events:[create,modify,remove], pattern:\"*.csv\"}]` 触发 → 输入 `{trigger:{path,kind}}` 自动注入,run 走 lumo.db 持久化(trigger_kind=file)."),
                item(
                    "T-04",
                    "Webhook",
                    "M2",
                    "ready",
                    "★ `lumo serve` 启 axum HTTP server (默认 127.0.0.1:8787)，POST /webhook/<flow-name> 触发流；流必须声明 spec.triggers.[kind: webhook] 才能被外网驱动；X-Lumo-Token 共享密钥可选；run 走 lumo.db 持久化。",
                ),
                item("T-05", "热键", "M1", "planned", "rdev 跨平台 hook."),
                item(
                    "T-07",
                    "MCP 工具调用触发",
                    "M2",
                    "planned",
                    "MCP server 排期 M2.",
                ),
            ],
        },
        FeatureSection {
            id: "observe".into(),
            title: "调试 / 可观测 (X)".into(),
            items: vec![
                item(
                    "X-01",
                    "单步 / 变量面板",
                    "M1",
                    "ready",
                    "右栏属性 + 单步运行入口.",
                ),
                item(
                    "X-04",
                    "错误堆栈 + 重试链路",
                    "M1",
                    "ready",
                    "step_runs.error_json 已写库.",
                ),
                item(
                    "X-05",
                    "OTel GenAI semconv",
                    "M2",
                    "planned",
                    "opentelemetry crate 待集成.",
                ),
                item(
                    "X-07",
                    "Time-Travel Debugger",
                    "M1",
                    "partial",
                    "时间线滑块基于已有 step_runs.",
                ),
                item(
                    "X-09",
                    "实时 stdout/stderr",
                    "M1",
                    "partial",
                    "Studio 底栏聚合日志.",
                ),
            ],
        },
        FeatureSection {
            id: "mcp".into(),
            title: "MCP 双向 (MCP)".into(),
            items: vec![
                item(
                    "MCP-01",
                    "LumoRPA as MCP Server",
                    "M2",
                    "ready",
                    "`lumo mcp --flows ./flows` 通过 JSON-RPC 2.0 / stdio 暴露 5 个工具 (list_flows, validate_flow, run_flow, list_runs, get_run) 以及 resources/list + resources/read(把流文件以 `file://` URI 暴露,Claude/Cursor 可直接读取 YAML;路径越界拒绝).",
                ),
                item(
                    "MCP-02",
                    "LumoRPA as MCP Client",
                    "M2",
                    "ready",
                    "`mcp.call` action 已注册;通过 stdio + JSON-RPC 2.0 调用任意 MCP server,执行 initialize → tools/call 握手,受 `capabilities.mcp` 白名单门禁保护.",
                ),
                item(
                    "MCP-03",
                    "Tool Discovery + 审批",
                    "M3",
                    "ready",
                    "`mcp.discover` action 通过 `tools/list` 返回工具描述符 + `proposed_grant` + `already_allowed`;`capabilities.mcp` 支持 `server`、`server:tool`、`server:tool_*` 三档粒度,`mcp.call` 强制按 `(server,tool)` 对放行.",
                ),
            ],
        },
        FeatureSection {
            id: "security".into(),
            title: "安全 / 沙箱 (Se)".into(),
            items: vec![
                item(
                    "Se-01",
                    "Capability 声明",
                    "M1",
                    "ready",
                    "spec.capabilities 在执行前强校验;Studio 右侧 `权限` Tab 渲染当前 network/fs.read/fs.write/llm/mcp 五档 chip 列表;每档自带 `+加白名单` 表单,通过 `add_capability_grant` Tauri 命令把 grant 追加回 YAML 自动去重并热刷新编辑器(配合 MCP-03 的 `proposed_grant`).",
                ),
                item(
                    "Se-02",
                    "默认 deny 网络出站",
                    "M1",
                    "ready",
                    "ai.chat 需要 LUMO_ALLOW_LLM_NETWORK=1;`add_capability_grant` 把 network/fs.read/fs.write/llm/mcp 五档 grant 写回 YAML,自动去重.",
                ),
                item(
                    "Se-05",
                    "凭据 LLM 不可见",
                    "M3",
                    "planned",
                    "Vault JIT 注入排期 M3.",
                ),
            ],
        },
    ]
}

#[cfg(test)]
mod path_sandbox_tests {
    //! P0-3: the webview file IPC must confine reads to the flow library +
    //! bundled examples and confine writes to LUMO_HOME, so a crafted path
    //! (`../`, absolute, symlink) can't exfiltrate or tamper with arbitrary
    //! files on disk.
    use super::{resolve_within, resolve_write_within};
    use std::fs;

    #[test]
    fn resolve_within_allows_file_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let f = root.join("a.lumoflow.yaml");
        fs::write(&f, "x").unwrap();
        let got = resolve_within(f.to_str().unwrap(), std::slice::from_ref(&root)).unwrap();
        assert!(got.starts_with(&root));
    }

    #[test]
    fn resolve_within_rejects_dotdot_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("flows");
        fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("secret.txt");
        fs::write(&outside, "secret").unwrap();
        let root_canon = root.canonicalize().unwrap();
        let escape = root.join("../secret.txt");
        let err = resolve_within(escape.to_str().unwrap(), &[root_canon]).unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }

    #[test]
    fn resolve_within_rejects_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let err = resolve_within(root.join("nope.yaml").to_str().unwrap(), &[root]).unwrap_err();
        assert!(err.contains("resolve"), "got: {err}");
    }

    #[test]
    fn resolve_write_within_allows_new_file_under_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let flows = home.join("flows");
        fs::create_dir_all(&flows).unwrap();
        // target does not exist yet — must still resolve via its parent
        let target = flows.join("new.lumoflow.yaml");
        let got = resolve_write_within(target.to_str().unwrap(), &home).unwrap();
        assert_eq!(got, flows.join("new.lumoflow.yaml"));
    }

    #[test]
    fn resolve_write_within_rejects_escape_above_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let flows = home.join("flows");
        fs::create_dir_all(&flows).unwrap();
        let home_canon = home.canonicalize().unwrap();
        // home/flows/../../evil.yaml resolves to tmp/evil.yaml — outside home
        let escape = flows.join("../../evil.lumoflow.yaml");
        let err = resolve_write_within(escape.to_str().unwrap(), &home_canon).unwrap_err();
        assert!(err.contains("outside"), "got: {err}");
    }
}
