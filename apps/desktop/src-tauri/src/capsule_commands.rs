//! The companion owns one task. Its projection never follows the global latest run.
use super::{agent_commands, agent_service::AgentStartInput, open_repo, DesktopState};
use lumo_agent::AgentEvent;
use lumo_core::CancelToken;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use tauri::{Emitter, Listener, Manager, State, Wry};

type AppHandle = tauri::AppHandle<Wry>;
const MAX_TEXT_CHARS: usize = 8192;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CapsuleSnapshot {
    state: String,
    listening: bool,
    muted: bool,
    blocked: bool,
    message: String,
    partial: String,
    transcript: String,
    source: String,
    progress: Option<f64>,
    active_run_id: Option<String>,
    run_state: Option<String>,
    task_label: String,
    result: String,
    error: Option<String>,
    seq: i64,
    revision: u64,
    expanded: bool,
    config: super::voice_commands::VoiceConfigDto,
}

impl Default for CapsuleSnapshot {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            listening: false,
            muted: false,
            blocked: false,
            message: String::new(),
            partial: String::new(),
            transcript: String::new(),
            source: String::new(),
            progress: None,
            active_run_id: None,
            run_state: None,
            task_label: String::new(),
            result: String::new(),
            error: None,
            seq: 0,
            revision: 0,
            expanded: false,
            config: Default::default(),
        }
    }
}

#[derive(Default)]
pub(super) struct CapsuleRuntime {
    snapshot: CapsuleSnapshot,
    generation: u64,
    pending: Option<CancelToken>,
    completed_nodes: HashSet<String>,
    result_summaries: Vec<String>,
    node_count: usize,
}

impl CapsuleRuntime {
    fn changed(&mut self) -> CapsuleSnapshot {
        self.snapshot.revision = self.snapshot.revision.saturating_add(1);
        self.snapshot.clone()
    }

    fn busy(&self) -> bool {
        self.pending.is_some()
            || matches!(
                self.snapshot.run_state.as_deref(),
                Some("running" | "planning" | "waitingApproval" | "paused")
            )
    }

    fn reserve(&mut self, text: &str, source: &str) -> Result<(u64, CancelToken), String> {
        let text = validate_text(text)?;
        if self.busy() {
            return Err("桌宠任务正在处理，请先停止或等待当前任务完成".into());
        }
        self.generation = self.generation.saturating_add(1);
        let cancel = CancelToken::new();
        self.pending = Some(cancel.clone());
        self.completed_nodes.clear();
        self.result_summaries.clear();
        self.node_count = 0;
        self.snapshot.active_run_id = None;
        self.snapshot.seq = 0;
        self.snapshot.run_state = Some("planning".into());
        self.snapshot.state = "planning".into();
        self.snapshot.listening = false;
        self.snapshot.source = source.into();
        self.snapshot.task_label = bounded(text, 160);
        self.snapshot.transcript = text.into();
        self.snapshot.partial.clear();
        self.snapshot.result.clear();
        self.snapshot.error = None;
        self.snapshot.progress = None;
        self.snapshot.message = "正在规划任务".into();
        Ok((self.generation, cancel))
    }

    fn bind(&mut self, generation: u64, run_id: &str) -> bool {
        if generation != self.generation || self.pending.is_none() {
            return false;
        }
        self.pending = None;
        self.snapshot.active_run_id = Some(run_id.into());
        self.snapshot.run_state = Some("running".into());
        self.snapshot.state = "executing".into();
        self.snapshot.message = "正在执行任务".into();
        true
    }

    fn require_run(&self, run_id: &str) -> Result<(), String> {
        if run_id.is_empty() || self.snapshot.active_run_id.as_deref() != Some(run_id) {
            return Err("桌宠任务已变化，请刷新后重试".into());
        }
        Ok(())
    }

    fn cancel_pending(&mut self) -> bool {
        let Some(cancel) = self.pending.take() else {
            return false;
        };
        cancel.cancel();
        self.generation = self.generation.saturating_add(1);
        self.snapshot.run_state = Some("cancelled".into());
        self.snapshot.state = "idle".into();
        self.snapshot.message = "任务已停止".into();
        self.snapshot.result = self.snapshot.message.clone();
        true
    }

    fn apply_event(
        &mut self,
        run_id: &str,
        seq: i64,
        kind: &str,
        node_id: Option<&str>,
        payload: &Value,
    ) -> bool {
        if self.snapshot.active_run_id.as_deref() != Some(run_id) || seq <= self.snapshot.seq {
            return false;
        }
        // Terminal state is absorbing even when a late progress/control event is persisted.
        if matches!(
            self.snapshot.run_state.as_deref(),
            Some("completed" | "failed" | "cancelled")
        ) {
            return false;
        }
        self.snapshot.seq = seq;
        match kind {
            "plan.created" | "plan.revised" => {
                self.node_count = payload
                    .get("plan")
                    .and_then(|p| p.get("nodes"))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
            }
            "permission.requested" => {
                self.snapshot.run_state = Some("waitingApproval".into());
                self.snapshot.state = "confirming".into();
                self.snapshot.message = "等待审批，请打开任务详情确认".into();
            }
            "run.paused" => {
                self.snapshot.run_state = Some("paused".into());
                self.snapshot.message = "任务已暂停".into();
            }
            "run.started" | "run.resumed" => {
                let waiting =
                    payload.get("state").and_then(Value::as_str) == Some("waitingApproval");
                self.snapshot.run_state = Some(
                    if waiting {
                        "waitingApproval"
                    } else {
                        "running"
                    }
                    .into(),
                );
                self.snapshot.state = if waiting { "confirming" } else { "executing" }.into();
                self.snapshot.message = if waiting {
                    "等待审批，请打开任务详情确认"
                } else {
                    "正在执行任务"
                }
                .into();
            }
            "node.started" => {
                if self.snapshot.run_state.as_deref() == Some("waitingApproval") {
                    self.snapshot.run_state = Some("running".into());
                    self.snapshot.state = "executing".into();
                }
                self.snapshot.message = "正在执行任务步骤".into();
            }
            "node.completed" => {
                if let Some(id) = node_id {
                    if self.completed_nodes.insert(id.to_string()) {
                        if let Some(summary) = payload.get("result").and_then(summarize_result) {
                            if self.result_summaries.len() == 3 {
                                self.result_summaries.remove(0);
                            }
                            self.result_summaries.push(summary);
                            self.snapshot.result = bounded(&self.result_summaries.join("\n"), 400);
                        }
                    }
                }
                if self.node_count > 0 {
                    self.snapshot.progress =
                        Some((self.completed_nodes.len() as f64 / self.node_count as f64).min(1.0));
                }
                self.snapshot.message = format!("已完成 {} 个步骤", self.completed_nodes.len());
            }
            "run.completed" | "run.failed" | "run.cancelled" => {
                let (run_state, state, message) = match kind {
                    "run.completed" => ("completed", "reporting", "任务执行完成"),
                    "run.failed" => (
                        "failed",
                        "error",
                        classify_failure(
                            payload
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        ),
                    ),
                    _ => ("cancelled", "idle", "任务已停止"),
                };
                self.snapshot.run_state = Some(run_state.into());
                self.snapshot.state = state.into();
                self.snapshot.message = message.into();
                self.snapshot.result =
                    if kind == "run.completed" && !self.completed_nodes.is_empty() {
                        format!(
                            "任务执行完成，共完成 {} 个步骤。",
                            self.completed_nodes.len()
                        )
                    } else {
                        message.into()
                    };
                if !self.result_summaries.is_empty() {
                    self.snapshot.result.push('\n');
                    self.snapshot
                        .result
                        .push_str(&self.result_summaries.join("\n"));
                    self.snapshot.result = bounded(&self.snapshot.result, 400);
                }
                self.snapshot.error = (kind == "run.failed").then(|| message.into());
                if kind == "run.completed" {
                    self.snapshot.progress = Some(1.0);
                }
            }
            _ => {}
        }
        true
    }

    fn apply_voice_event(&mut self, name: &str, payload: &Value) {
        match name {
            "lumo://voice-state" => {
                self.snapshot.listening = payload
                    .get("listening")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if self.snapshot.listening || !self.busy() {
                    if let Some(state) = payload.get("state").and_then(Value::as_str) {
                        self.snapshot.state = bounded(state, 40);
                    }
                    if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
                        self.snapshot.message = bounded(reason, 400);
                    }
                }
            }
            "lumo://transcript" => {
                let text = bounded(
                    payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    MAX_TEXT_CHARS,
                );
                let final_text = payload
                    .get("isFinal")
                    .or_else(|| payload.get("final"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if final_text {
                    if !text.trim().is_empty() && !self.busy() {
                        self.snapshot.active_run_id = None;
                        self.snapshot.run_state = None;
                        self.snapshot.seq = 0;
                        self.snapshot.progress = None;
                        self.snapshot.result.clear();
                        self.snapshot.error = None;
                        self.snapshot.task_label = bounded(&text, 160);
                        self.snapshot.source = "voice".into();
                        self.snapshot.message = "正在处理语音指令".into();
                    }
                    self.snapshot.transcript = text;
                    self.snapshot.partial.clear();
                } else {
                    self.snapshot.partial = text;
                }
            }
            "lumo://voice-daemon-state" | "lumo://voice-muted" => {
                if let Some(muted) = payload.get("muted").and_then(Value::as_bool) {
                    self.snapshot.muted = muted;
                }
                if let Some(blocked) = payload.get("blocked").and_then(Value::as_bool) {
                    self.snapshot.blocked = blocked;
                }
            }
            "lumo://voice-feedback" if !self.busy() && self.snapshot.active_run_id.is_none() => {
                if let Some(message) = payload.get("message").and_then(Value::as_str) {
                    self.snapshot.message = bounded(message, 400);
                }
            }
            "lumo://voice-follow-up" if !self.busy() => {
                self.snapshot.partial.clear();
                self.snapshot.transcript.clear();
                self.snapshot.state = "listening".into();
                if let Some(message) = payload.get("message").and_then(Value::as_str) {
                    self.snapshot.message = bounded(message, 400);
                }
            }
            _ => {}
        }
    }
}

fn bounded(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn summarize_result(value: &Value) -> Option<String> {
    fn summary(value: &Value, depth: usize) -> Option<String> {
        if depth > 3 {
            return None;
        }
        if let Some(text) = value.as_str() {
            let text = redact_summary_text(text);
            return (!text.trim().is_empty()).then_some(text);
        }
        if let Some(object) = value.as_object() {
            // These fields are intended for human feedback. Never stringify
            // arbitrary objects containing credentials or task inputs.
            for key in ["summary", "message", "text"] {
                if let Some(text) = object.get(key).and_then(Value::as_str) {
                    let text = redact_summary_text(text);
                    if !text.trim().is_empty() {
                        return Some(text);
                    }
                }
            }
            for key in ["structuredContent", "result", "outputs", "content", "data"] {
                if let Some(value) = object.get(key) {
                    if let Some(text) = summary(value, depth + 1) {
                        return Some(text);
                    }
                }
            }
            let mut facts = Vec::new();
            for (key, label) in [
                ("count", "数量"),
                ("total", "总数"),
                ("processed", "已处理"),
                ("matched", "已匹配"),
                ("created", "已创建"),
                ("updated", "已更新"),
                ("deleted", "已删除"),
                ("skipped", "已跳过"),
            ] {
                if let Some(count) = object.get(key).and_then(Value::as_u64) {
                    facts.push(format!("{label} {count}"));
                }
            }
            for key in ["success", "ok"] {
                if let Some(success) = object.get(key).and_then(Value::as_bool) {
                    facts.push(
                        if success {
                            "结果成功"
                        } else {
                            "结果未成功"
                        }
                        .into(),
                    );
                    break;
                }
            }
            for key in ["rows", "items", "records"] {
                if let Some(rows) = object.get(key).and_then(Value::as_array) {
                    facts.push(format!("返回 {} 条记录", rows.len()));
                    break;
                }
            }
            if facts.is_empty() && !object.is_empty() {
                facts.push(format!(
                    "返回 {} 项结果字段，完整输出请打开任务详情",
                    object.len()
                ));
            }
            return (!facts.is_empty())
                .then(|| facts.into_iter().take(4).collect::<Vec<_>>().join("，"));
        }
        if let Some(rows) = value.as_array() {
            // MCP text content can contain a useful message; ordinary record
            // arrays expose their count without expanding private row values.
            if let Some(text) = rows.iter().take(2).find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| {
                        item.get("text")
                            .and_then(Value::as_str)
                            .map(redact_summary_text)
                    })
                    .flatten()
            }) {
                return Some(text);
            }
            return Some(format!("返回 {} 条记录", rows.len()));
        }
        if let Some(success) = value.as_bool() {
            return Some(
                if success {
                    "结果为是"
                } else {
                    "结果为否"
                }
                .into(),
            );
        }
        value.as_number().map(|number| format!("结果：{number}"))
    }
    summary(&lumo_agent::redact_arguments(value), 0).map(|text| bounded(&text, 360))
}

fn redact_summary_text(text: &str) -> String {
    if text.contains("-----BEGIN") && text.contains("PRIVATE KEY-----") {
        return "包含凭证的结果已隐藏，请打开任务详情查看".into();
    }
    let mut redact_next = false;
    let mut words = Vec::new();
    // Limit work before splitting externally supplied tool responses.
    let text = bounded(text, 2400);
    for word in text.split_whitespace() {
        let trimmed = word
            .trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '{' | '}' | '[' | ']'));
        let lower = trimmed.to_ascii_lowercase();
        if redact_next {
            if matches!(
                lower.as_str(),
                "bearer" | "basic" | "is" | ":" | "=" | "是" | "为"
            ) {
                continue;
            }
            words.push("[已隐藏]".to_string());
            redact_next = false;
            continue;
        }
        if matches!(lower.as_str(), "bearer" | "basic") {
            words.push("[已隐藏]".into());
            redact_next = true;
            continue;
        }
        let key = trimmed
            .split(['=', ':', '：'])
            .next()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .flat_map(char::to_lowercase)
            .collect::<String>();
        let sensitive_key = [
            "authorization",
            "apikey",
            "accesstoken",
            "refreshtoken",
            "token",
            "password",
            "passwd",
            "secret",
            "cookie",
            "privatekey",
        ]
        .iter()
        .any(|secret| key == *secret || key.ends_with(secret));
        let token = lower.starts_with("sk-")
            || lower.starts_with("sk_")
            || lower.starts_with("ghp_")
            || lower.starts_with("gho_")
            || lower.starts_with("github_pat_")
            || lower.starts_with("xoxb-")
            || lower.starts_with("xoxp-")
            || lower.starts_with("akia") && lower.len() >= 16
            || lower.starts_with("eyj") && lower.split('.').count() == 3
            || [
                "token=",
                "api_key=",
                "apikey=",
                "password=",
                "secret=",
                "cookie=",
            ]
            .iter()
            .any(|marker| lower.contains(marker));
        if sensitive_key {
            words.push("[已隐藏]".into());
            redact_next = trimmed
                .split_once(['=', ':', '：'])
                .is_none_or(|(_, value)| {
                    let value = value.trim_matches(['"', '\'', ':']).to_ascii_lowercase();
                    value.is_empty() || matches!(value.as_str(), "bearer" | "basic")
                });
        } else if token {
            words.push("[已隐藏]".into());
        } else {
            words.push(word.into());
        }
    }
    bounded(&words.join(" "), 360)
}

fn classify_failure(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if ["timeout", "timed out", "deadline"]
        .iter()
        .any(|word| normalized.contains(word))
    {
        "任务执行超时，请检查服务连接或调整步骤超时"
    } else if ["budget", "quota", "rate limit", "429"]
        .iter()
        .any(|word| normalized.contains(word))
    {
        "任务受到预算或服务额度限制，请检查模型和用量配置"
    } else if ["permission", "denied", "approval", "unauthorized"]
        .iter()
        .any(|word| normalized.contains(word))
    {
        "任务未获授权，请在任务详情检查审批与权限"
    } else if [
        "unavailable",
        "not found",
        "connection",
        "source changed",
        "capability",
    ]
    .iter()
    .any(|word| normalized.contains(word))
    {
        "任务依赖不可用或已变更，请检查能力和服务配置"
    } else {
        "任务执行失败，请打开任务详情查看原因"
    }
}

fn validate_text(text: &str) -> Result<&str, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("请输入任务指令".into());
    }
    if trimmed.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("任务指令不能超过 {MAX_TEXT_CHARS} 个字符"));
    }
    Ok(trimmed)
}

fn update(
    app: &AppHandle,
    change: impl FnOnce(&mut CapsuleRuntime),
) -> Result<CapsuleSnapshot, String> {
    let snapshot = {
        let state = app.state::<DesktopState>();
        let mut runtime = state
            .capsule
            .lock()
            .map_err(|_| "capsule state is unavailable")?;
        change(&mut runtime);
        runtime.changed()
    };
    let _ = app.emit("lumo://capsule-state", &snapshot);
    Ok(snapshot)
}

pub(super) fn setup_capsule_host(app: &AppHandle) -> Result<(), String> {
    for name in [
        "lumo://voice-state",
        "lumo://transcript",
        "lumo://voice-daemon-state",
        "lumo://voice-muted",
        "lumo://voice-feedback",
        "lumo://voice-follow-up",
    ] {
        let app = app.clone();
        let listener_app = app.clone();
        app.listen(name, move |event| {
            if let Ok(payload) = serde_json::from_str::<Value>(event.payload()) {
                let _ = update(&listener_app, |runtime| {
                    runtime.apply_voice_event(name, &payload)
                });
            }
        });
    }
    refresh_snapshot(app).map(|_| ())
}

fn refresh_snapshot(app: &AppHandle) -> Result<CapsuleSnapshot, String> {
    let state = app.state::<DesktopState>();
    // Never hold the voice/daemon mutex while taking the projection mutex: voice
    // event callbacks can run synchronously from inside their emitting lock.
    let (config, listening, voice_state) = {
        let voice = state
            .voice
            .lock()
            .map_err(|_| "voice state is unavailable")?;
        (
            voice.config.clone(),
            voice.controller.state() == lumo_voice::VoiceState::Listening,
            super::voice_commands::state_name(voice.controller.state()),
        )
    };
    let (muted, blocked) = {
        let daemon = state
            .voice_daemon
            .lock()
            .map_err(|_| "voice daemon is unavailable")?;
        (daemon.is_muted(), daemon.is_blocked())
    };
    update(app, |runtime| {
        runtime.snapshot.config = config;
        runtime.snapshot.listening = listening;
        if runtime.snapshot.revision == 0 {
            runtime.snapshot.state = voice_state;
        }
        runtime.snapshot.muted = muted;
        runtime.snapshot.blocked = blocked;
    })
}

#[tauri::command]
pub(super) fn capsule_status(app: AppHandle) -> Result<CapsuleSnapshot, String> {
    refresh_snapshot(&app)
}

#[tauri::command]
pub(super) async fn capsule_submit_text(
    app: AppHandle,
    text: String,
) -> Result<CapsuleSnapshot, String> {
    let text = validate_text(&text)?.to_string();
    start_companion_request(
        &app,
        AgentStartInput {
            utterance: text,
            profile_id: None,
            supplied_plan: None,
            conversation_id: None,
            conversation_context: Vec::new(),
        },
        "text",
    )
    .await
}

pub(super) async fn start_companion_request(
    app: &AppHandle,
    input: AgentStartInput,
    source: &str,
) -> Result<CapsuleSnapshot, String> {
    let state = app.state::<DesktopState>();
    let (generation, cancel) = {
        let mut runtime = state
            .capsule
            .lock()
            .map_err(|_| "capsule state is unavailable")?;
        runtime.reserve(&input.utterance, source)?
    };
    let _ = update(app, |_| {});
    super::voice_commands::show_capsule_window(app);
    // The same token is checked during planning and by the executor. Even a
    // cancellation racing with service startup cannot execute a late task.
    let started = agent_commands::start_agent_request_cancellable(&state, input, cancel).await;
    match started {
        Ok(session) => {
            let repo = open_repo(app)?;
            // Bind and recover the durable prefix under the projection lock.
            // A concurrent live event cannot jump ahead of the replay and make
            // us skip earlier plan/progress events as duplicate sequences.
            let recovered = {
                let mut runtime = state
                    .capsule
                    .lock()
                    .map_err(|_| "capsule state is unavailable")?;
                if runtime.bind(generation, &session.run_id) {
                    let events = repo
                        .list_agent_events(&session.run_id, 0)
                        .map_err(|error| error.to_string())?;
                    for event in events {
                        runtime.apply_event(
                            &event.run_id,
                            event.seq,
                            &event.kind,
                            event.node_id.as_deref(),
                            &event.payload,
                        );
                    }
                    let announce = runtime.snapshot.source == "voice"
                        && matches!(
                            runtime.snapshot.run_state.as_deref(),
                            Some("completed" | "failed")
                        );
                    Some((runtime.changed(), announce))
                } else {
                    None
                }
            };
            let Some((snapshot, announce)) = recovered else {
                // A late response must never attach itself to a newer submission.
                let _ = agent_commands::cancel_agent_session(&state, &session.run_id);
                return Err("任务已停止".into());
            };
            let _ = app.emit("lumo://capsule-state", &snapshot);
            if announce {
                super::voice_commands::record_agent_outcome(app, &snapshot.message);
                super::voice_commands::announce_agent_outcome(
                    app,
                    &snapshot.message,
                    snapshot.run_state.as_deref() == Some("completed"),
                );
            }
            refresh_snapshot(app)
        }
        Err(error) => {
            let mut report = false;
            update(app, |runtime| {
                if generation == runtime.generation && runtime.pending.is_some() {
                    report = runtime.snapshot.source == "voice";
                    runtime.pending = None;
                    runtime.snapshot.state = "error".into();
                    runtime.snapshot.run_state = Some("failed".into());
                    runtime.snapshot.message = "任务未能启动，请在主窗口检查模型和能力配置".into();
                    runtime.snapshot.error = Some(runtime.snapshot.message.clone());
                    runtime.snapshot.result = runtime.snapshot.message.clone();
                }
            })?;
            if report {
                super::voice_commands::report_agent_start_failure(
                    app,
                    "任务未能启动，请检查模型和能力配置",
                );
            }
            Err(error)
        }
    }
}

pub(super) fn observe_agent_event(app: &AppHandle, event: &AgentEvent) {
    apply_agent_event(
        app,
        &event.run_id,
        event.seq,
        event.kind.as_str(),
        event.node_id.as_deref(),
        &event.payload,
    );
}

fn apply_agent_event(
    app: &AppHandle,
    run_id: &str,
    seq: i64,
    kind: &str,
    node_id: Option<&str>,
    payload: &Value,
) {
    let state = app.state::<DesktopState>();
    let applied = state.capsule.lock().ok().and_then(|mut runtime| {
        runtime
            .apply_event(run_id, seq, kind, node_id, payload)
            .then(|| {
                let announce = runtime.snapshot.source == "voice"
                    && matches!(kind, "run.completed" | "run.failed");
                (runtime.changed(), announce)
            })
    });
    if let Some((snapshot, announce)) = applied {
        let _ = app.emit("lumo://capsule-state", &snapshot);
        if announce {
            super::voice_commands::record_agent_outcome(app, &snapshot.message);
            super::voice_commands::announce_agent_outcome(
                app,
                &snapshot.message,
                snapshot.run_state.as_deref() == Some("completed"),
            );
        }
    }
}

/// Closing the companion never cancels Mission Control or background jobs.
pub(super) fn stop_associated_task(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let (run_id, busy) = {
        let mut runtime = state
            .capsule
            .lock()
            .map_err(|_| "capsule state is unavailable")?;
        runtime.cancel_pending();
        (runtime.snapshot.active_run_id.clone(), runtime.busy())
    };
    if busy {
        if let Some(run_id) = run_id {
            agent_commands::cancel_agent_session(&state, &run_id)?;
        }
    }
    let _ = update(app, |_| {});
    Ok(())
}

#[tauri::command]
pub(super) fn capsule_control(
    app: AppHandle,
    state: State<'_, DesktopState>,
    action: String,
    run_id: String,
) -> Result<CapsuleSnapshot, String> {
    {
        let runtime = state
            .capsule
            .lock()
            .map_err(|_| "capsule state is unavailable")?;
        runtime.require_run(&run_id)?;
        if action == "cancel" && !runtime.busy() {
            return Err("当前桌宠任务已经结束".into());
        }
    }
    match action.as_str() {
        "cancel" => {
            agent_commands::cancel_agent_session(&state, &run_id)?;
        }
        "open" => {
            super::show_main_window(&app);
            app.emit("lumo://open-view", "mission-control")
                .map_err(|error| error.to_string())?;
            app.emit(
                "lumo://capsule-open-run",
                serde_json::json!({"runId":run_id}),
            )
            .map_err(|error| error.to_string())?;
        }
        _ => return Err("unsupported capsule control".into()),
    }
    refresh_snapshot(&app)
}

#[tauri::command]
pub(super) fn capsule_resize(app: AppHandle, expanded: bool) -> Result<CapsuleSnapshot, String> {
    let snapshot = update(&app, |runtime| runtime.snapshot.expanded = expanded)?;
    super::voice_commands::show_capsule_window(&app);
    Ok(snapshot)
}

pub(super) fn window_state(app: &AppHandle) -> (bool, bool) {
    let state = app.state::<DesktopState>();
    let result = state
        .capsule
        .lock()
        .map(|runtime| (runtime.snapshot.expanded, runtime.busy()))
        .unwrap_or_default();
    result
}

pub(super) fn reset_expansion(app: &AppHandle) {
    let _ = update(app, |runtime| runtime.snapshot.expanded = false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn started() -> CapsuleRuntime {
        let mut runtime = CapsuleRuntime::default();
        let (generation, _) = runtime.reserve("任务", "text").unwrap();
        assert!(runtime.bind(generation, "companion-run"));
        runtime
    }

    #[test]
    fn companion_ignores_other_runs_and_duplicate_or_late_events() {
        let mut runtime = started();
        assert!(!runtime.apply_event("other-run", 100, "run.failed", None, &json!({})));
        assert!(runtime.apply_event("companion-run", 2, "run.completed", None, &json!({})));
        assert!(!runtime.apply_event("companion-run", 2, "run.completed", None, &json!({})));
        assert!(!runtime.apply_event("companion-run", 3, "run.started", None, &json!({})));
        assert_eq!(runtime.snapshot.run_state.as_deref(), Some("completed"));
    }

    #[test]
    fn cancelled_planning_cannot_bind_to_a_newer_request() {
        let mut runtime = CapsuleRuntime::default();
        let (first, cancel) = runtime.reserve("first", "voice").unwrap();
        assert!(runtime.reserve("duplicate", "text").is_err());
        assert!(runtime.cancel_pending());
        assert!(cancel.is_cancelled());
        let (second, _) = runtime.reserve("second", "text").unwrap();
        assert!(!runtime.bind(first, "old-run"));
        assert!(runtime.bind(second, "new-run"));
        assert!(runtime.require_run("old-run").is_err());
        assert!(runtime.require_run("latest").is_err());
        assert!(runtime.require_run("new-run").is_ok());
    }

    #[test]
    fn durable_replay_recovers_completion_and_counts_unique_nodes() {
        let mut runtime = started();
        runtime.apply_event(
            "companion-run",
            1,
            "plan.created",
            None,
            &json!({"plan":{"nodes":[{},{}]}}),
        );
        runtime.apply_event(
            "companion-run",
            2,
            "node.completed",
            Some("first"),
            &json!({"result":{"secret":"not for capsule"}}),
        );
        runtime.apply_event(
            "companion-run",
            3,
            "node.completed",
            Some("first"),
            &json!({}),
        );
        assert_eq!(runtime.snapshot.progress, Some(0.5));
        runtime.apply_event("companion-run", 4, "run.completed", None, &json!({}));
        assert_eq!(runtime.snapshot.progress, Some(1.0));
        assert!(!serde_json::to_string(&runtime.snapshot)
            .unwrap()
            .contains("secret"));
        assert!(runtime.reserve("next", "text").is_ok());
    }

    #[test]
    fn voice_transcript_and_daemon_bootstrap_contract() {
        let mut runtime = CapsuleRuntime::default();
        runtime.snapshot.expanded = true;
        for state in ["listening", "confirming", "idle"] {
            runtime.apply_voice_event(
                "lumo://voice-state",
                &json!({"state":state,"listening":state == "listening"}),
            );
            assert!(runtime.snapshot.expanded);
        }
        runtime.apply_voice_event(
            "lumo://transcript",
            &json!({"text":"partial", "isFinal":false}),
        );
        runtime.apply_voice_event(
            "lumo://transcript",
            &json!({"text":"final", "isFinal":true}),
        );
        assert_eq!(runtime.snapshot.transcript, "final");
        assert!(runtime.snapshot.partial.is_empty());
        runtime.apply_voice_event(
            "lumo://voice-daemon-state",
            &json!({"muted":true,"blocked":true}),
        );
        assert!(runtime.snapshot.muted && runtime.snapshot.blocked);
        assert!(runtime.changed().revision < runtime.changed().revision);
    }

    #[test]
    fn new_voice_capture_can_end_after_an_old_task_completed() {
        let mut runtime = started();
        runtime.apply_event("companion-run", 1, "run.completed", None, &json!({}));
        runtime.apply_voice_event(
            "lumo://voice-state",
            &json!({"state":"listening","listening":true}),
        );
        assert!(runtime.snapshot.listening);
        runtime.apply_voice_event(
            "lumo://voice-state",
            &json!({"state":"error","listening":false,"reason":"capture unavailable"}),
        );
        assert!(!runtime.snapshot.listening);
        assert_eq!(runtime.snapshot.state, "error");
        assert_eq!(runtime.snapshot.run_state.as_deref(), Some("completed"));
        assert_eq!(
            runtime.snapshot.active_run_id.as_deref(),
            Some("companion-run")
        );
        assert!(runtime.reserve("next text command", "text").is_ok());
    }

    #[test]
    fn approved_node_dispatch_replaces_waiting_approval_but_preserves_pause() {
        let mut runtime = started();
        runtime.apply_event("companion-run", 1, "permission.requested", None, &json!({}));
        assert_eq!(
            runtime.snapshot.run_state.as_deref(),
            Some("waitingApproval")
        );
        runtime.apply_event(
            "companion-run",
            2,
            "node.started",
            Some("approved"),
            &json!({}),
        );
        assert_eq!(runtime.snapshot.run_state.as_deref(), Some("running"));
        assert_eq!(runtime.snapshot.state, "executing");
        runtime.apply_event("companion-run", 3, "run.paused", None, &json!({}));
        runtime.apply_event(
            "companion-run",
            4,
            "node.started",
            Some("already-in-flight"),
            &json!({}),
        );
        assert_eq!(runtime.snapshot.run_state.as_deref(), Some("paused"));
    }

    #[test]
    fn text_limits_are_unicode_safe_and_do_not_depend_on_microphone() {
        assert!(validate_text(" \n").is_err());
        assert!(validate_text(&"中".repeat(MAX_TEXT_CHARS + 1)).is_err());
        let mut runtime = CapsuleRuntime::default();
        runtime.snapshot.muted = true;
        runtime.snapshot.blocked = true;
        runtime.snapshot.config.enabled = false;
        assert!(runtime.reserve(" 运行流程 ", "text").is_ok());
        assert_eq!(runtime.snapshot.transcript, "运行流程");
    }

    #[test]
    fn useful_output_is_projected_and_common_credentials_are_hidden() {
        let mut runtime = started();
        runtime.apply_event(
            "companion-run",
            1,
            "node.completed",
            Some("report"),
            &json!({
                "result":{"summary":"日报已生成，共 12 条记录", "password":"private-password"}
            }),
        );
        assert!(runtime.snapshot.result.contains("日报已生成"));
        runtime.apply_event("companion-run", 2, "run.completed", None, &json!({}));
        assert!(runtime.snapshot.result.contains("日报已生成"));
        let output = summarize_result(&json!({"message":"ready Authorization:Bearer private-value sk-test ghp_test password=private-password https://example.test/?api_key=query-secret"})).unwrap();
        for secret in [
            "private-value",
            "sk-test",
            "ghp_test",
            "private-password",
            "query-secret",
        ] {
            assert!(!output.contains(secret), "leaked {secret}: {output}");
        }
        assert!(output.contains("ready") && output.contains("已隐藏"));
        assert_eq!(
            summarize_result(&json!({"structuredContent":{"count":3,"success":true}})).as_deref(),
            Some("数量 3，结果成功")
        );
        assert_eq!(
            summarize_result(&json!({"data":[{"password":"private-password"}]})).as_deref(),
            Some("返回 1 条记录")
        );
        assert!(
            !summarize_result(&json!({"password":{"text":"private-password"}}))
                .unwrap()
                .contains("private-password")
        );
        assert!(redact_summary_text(&"中".repeat(1000)).chars().count() <= 360);
        assert_eq!(
            classify_failure("timeout: token=secret"),
            "任务执行超时，请检查服务连接或调整步骤超时"
        );
    }

    #[test]
    fn classified_run_failures_use_failure_persona_even_without_failure_words() {
        for error in [
            "timeout exceeded",
            "budget quota exhausted",
            "permission denied",
        ] {
            let mut runtime = started();
            runtime.apply_event(
                "companion-run",
                1,
                "run.failed",
                None,
                &json!({"error":error}),
            );
            let ok = runtime.snapshot.run_state.as_deref() == Some("completed");
            assert_eq!(
                super::super::voice_commands::agent_outcome_moment(ok),
                super::super::voice_persona::PersonaMoment::Failure
            );
            assert_ne!(
                runtime.snapshot.message,
                "任务执行失败，请打开任务详情查看原因"
            );
        }
    }
}
