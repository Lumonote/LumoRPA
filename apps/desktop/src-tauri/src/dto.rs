//! Serde DTOs shared by the desktop IPC commands, plus row/report -> DTO
//! conversion helpers. Pure move out of `lib.rs`; semantics unchanged.

use super::*;


#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppInfo {
    pub(crate) version: String,
    pub(crate) data_dir: String,
    pub(crate) resource_dir: Option<String>,
    pub(crate) examples_dir: Option<String>,
    pub(crate) providers_path: String,
    pub(crate) skills_path: String,
    pub(crate) platform: String,
    pub(crate) arch: String,
    pub(crate) network_enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IoDeclDto {
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) required: bool,
    pub(crate) default: Option<Value>,
    pub(crate) description: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FlowSummary {
    pub(crate) path: String,
    pub(crate) file_name: String,
    pub(crate) id: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) inputs: Vec<IoDeclDto>,
    pub(crate) outputs: Vec<IoDeclDto>,
    pub(crate) step_count: usize,
    pub(crate) valid: bool,
    pub(crate) error: Option<String>,
    /// `"user"` (saved by the operator) / `"recording"` (recorder output)
    /// / `"example"` (bundled). Defaults to `"user"` when scanned via the
    /// bare flow_summary helper; the library scanner overrides per source.
    #[serde(default)]
    pub(crate) source: String,
    /// File modification time as a unix-ms timestamp. Lets the library sort
    /// recently-touched flows to the top.
    #[serde(default)]
    pub(crate) updated_ms: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ValidationReport {
    pub(crate) path: String,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) inputs: Vec<IoDeclDto>,
    pub(crate) outputs: Vec<IoDeclDto>,
    pub(crate) capabilities: Value,
    pub(crate) step_count: usize,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionDto {
    pub(crate) id: String,
    pub(crate) family: String,
    pub(crate) summary: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunReportDto {
    pub(crate) run_id: String,
    pub(crate) success: bool,
    pub(crate) steps_total: usize,
    pub(crate) steps_ok: usize,
    pub(crate) steps_executed: usize,
    pub(crate) steps_failed: usize,
    pub(crate) steps_skipped: usize,
    pub(crate) steps_retried: usize,
    pub(crate) steps_caught: usize,
    pub(crate) duration_ms: u128,
    pub(crate) outputs: Option<Value>,
    /// F-20: when the run paused at a breakpoint / single-step, the path of the
    /// step it paused before. `None` for runs that completed/failed/cancelled.
    pub(crate) paused_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDto {
    pub(crate) id: String,
    pub(crate) flow_id: String,
    pub(crate) flow_version: String,
    pub(crate) trigger_kind: String,
    pub(crate) inputs: Value,
    pub(crate) outputs: Option<Value>,
    pub(crate) state: String,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) cost_token: i64,
    pub(crate) cost_usd_micro: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StepRunDto {
    pub(crate) seq: i64,
    pub(crate) path: String,
    pub(crate) parent_path: Option<String>,
    pub(crate) depth: i64,
    pub(crate) step_id: String,
    pub(crate) idx: i64,
    pub(crate) state: String,
    pub(crate) attempt: i64,
    pub(crate) output_json: Option<Value>,
    pub(crate) vars_json: Option<Value>,
    pub(crate) error: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResponse {
    pub(crate) report: RunReportDto,
    pub(crate) run: Option<RunDto>,
    pub(crate) steps: Vec<StepRunDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunDetail {
    pub(crate) run: RunDto,
    pub(crate) steps: Vec<StepRunDto>,
}

/// X-07 Time-Travel: a single artifact blob streamed back to the webview as a
/// base64 data URL so `<img>` / `<iframe>` can render it directly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArtifactBlobDto {
    pub(crate) id: String,
    pub(crate) mime: String,
    pub(crate) data_url: String,
    pub(crate) size: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderProfileDto {
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) wire_api: Option<String>,
    pub(crate) default_model: Option<String>,
    pub(crate) vision_model: Option<String>,
    pub(crate) ocr_model: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) api_key_env: Option<String>,
    pub(crate) has_inline_key: bool,
    pub(crate) has_key: bool,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) models: Vec<String>,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) notes: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderStatus {
    pub(crate) path: String,
    pub(crate) active: Option<String>,
    pub(crate) profiles: Vec<ProviderProfileDto>,
    pub(crate) network_enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderInput {
    pub(crate) name: String,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) wire_api: Option<String>,
    #[serde(default)]
    pub(crate) base_url: Option<String>,
    #[serde(default)]
    pub(crate) api_key: Option<String>,
    #[serde(default)]
    pub(crate) api_key_env: Option<String>,
    #[serde(default)]
    pub(crate) default_model: Option<String>,
    pub(crate) vision_model: Option<String>,
    pub(crate) ocr_model: Option<String>,
    #[serde(default)]
    pub(crate) models: Vec<String>,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) reasoning_effort: Option<String>,
    #[serde(default)]
    pub(crate) notes: Option<String>,
    /// When true, mark this profile as active after upsert.
    #[serde(default)]
    pub(crate) activate: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderTestResult {
    pub(crate) ok: bool,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) input_tokens: u32,
    pub(crate) output_tokens: u32,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillDto {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) tags: Vec<String>,
    pub(crate) source: String,
    pub(crate) hash: Option<String>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppearanceOptions {
    /// Panel alpha (0-100, percentage applied to white).
    pub(crate) opacity: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WindowAlphaOptions {
    /// 0..=255 alpha applied to the window background color. 0 = fully clear,
    /// 255 = fully opaque. Sliders use the full range.
    pub(crate) alpha: u8,
    /// Optional tinted background color (RGB). Defaults to white-ish so the
    /// platform vibrancy is preserved.
    #[serde(default)]
    pub(crate) rgb: Option<[u8; 3]>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecorderStatus {
    pub(crate) recording: bool,
    pub(crate) target: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) backend: String,
    pub(crate) note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecorderStopResult {
    pub(crate) events: usize,
    pub(crate) note: String,
    pub(crate) yaml_hint: String,
}

pub(crate) fn validation_report(path: &str, flow: &Flow) -> ValidationReport {
    let warnings = if flow_uses_action(&flow.spec.steps, "ai.chat")
        || flow_uses_action(&flow.spec.steps, "image.ocr")
    {
        vec!["This flow uses AI actions; configure providers.toml and the corresponding API key environment variables before running it.".into()]
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

pub(crate) fn io_dtos(items: &[IoDecl]) -> Vec<IoDeclDto> {
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

pub(crate) fn count_steps(steps: &[Step]) -> usize {
    steps
        .iter()
        .map(|step| 1 + step.children().into_iter().map(count_steps).sum::<usize>())
        .sum()
}

pub(crate) fn report_dto(report: lumo_core::RunReport) -> RunReportDto {
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
        paused_at: report.paused_at,
    }
}

pub(crate) fn run_dto(row: FlowRunRow) -> RunDto {
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

pub(crate) fn step_dto(row: StepRunRow) -> StepRunDto {
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
        vars_json: row.vars_json,
        error: row.error,
        started_at: row.started_at.map(|t| t.to_rfc3339()),
        finished_at: row.finished_at.map(|t| t.to_rfc3339()),
        duration_ms,
    }
}
