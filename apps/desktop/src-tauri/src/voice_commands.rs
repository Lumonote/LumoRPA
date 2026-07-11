use super::{
    voice_daemon::{VoiceDaemon, VoiceDaemonAction, VoiceDaemonState, VoiceSuspendReason},
    DesktopState,
};
use async_trait::async_trait;
use lumo_voice::provider::{AudioCapture, ProviderError, SttEvent, SttProvider};
use lumo_voice::stt_router::{SttPreference, SttRouter, SttRouterConfig};
use lumo_voice::{cpal_capture::CpalAudioCapture, VoiceController, VoiceEvent, VoiceState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio_util::sync::CancellationToken;

type AppHandle = tauri::AppHandle<Wry>;

const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+Space";
const DEFAULT_DEVICE: &str = "default";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum MicrophonePermission {
    NotDetermined,
    Granted,
    Denied,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceConfigDto {
    pub(super) enabled: bool,
    pub(super) shortcut: String,
    pub(super) device_id: String,
}

impl Default for VoiceConfigDto {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: DEFAULT_SHORTCUT.into(),
            device_id: DEFAULT_DEVICE.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceDeviceDto {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceStatusDto {
    state: String,
    listening: bool,
    permission: MicrophonePermission,
    config: VoiceConfigDto,
    shortcut_registered: bool,
    shortcut_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceDaemonStatusDto {
    state: VoiceDaemonState,
    muted: bool,
    blocked: bool,
    selected_device: String,
    active_device: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct VoiceStatePayload {
    state: String,
    listening: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptPayload {
    text: String,
    is_final: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentStartRequest {
    utterance: String,
    source: &'static str,
}

pub(super) struct VoiceRuntime {
    pub(super) controller: VoiceController,
    pub(super) config: VoiceConfigDto,
    pub(super) permission: MicrophonePermission,
    pub(super) cancel: Option<CancellationToken>,
    shortcut_registered: bool,
    shortcut_error: Option<String>,
    pipeline_factory: Arc<dyn VoicePipelineFactory>,
    pipeline_task: Option<tauri::async_runtime::JoinHandle<()>>,
}

impl Default for VoiceRuntime {
    fn default() -> Self {
        let config = VoiceConfigDto::default();
        Self {
            controller: VoiceController::new(config.enabled),
            config,
            permission: microphone_permission(),
            cancel: None,
            shortcut_registered: false,
            shortcut_error: None,
            pipeline_factory: Arc::new(DefaultVoicePipelineFactory),
            pipeline_task: None,
        }
    }
}

pub(super) struct VoicePipeline {
    capture: Arc<dyn AudioCapture>,
    stt: Arc<dyn SttProvider>,
}

pub(super) struct VoicePipelineConfig {
    device_id: String,
    models_root: PathBuf,
    cloud_allowed: bool,
}

pub(super) trait VoicePipelineFactory: Send + Sync {
    fn create(&self, config: &VoicePipelineConfig) -> Result<VoicePipeline, ProviderError>;
}

#[async_trait]
trait VoicePipelineSink: Send + Sync {
    async fn state(&self, reason: Option<String>);
    async fn transcript(&self, text: String, is_final: bool);
}

struct DefaultVoicePipelineFactory;

impl VoicePipelineFactory for DefaultVoicePipelineFactory {
    fn create(&self, config: &VoicePipelineConfig) -> Result<VoicePipeline, ProviderError> {
        let local: Option<Arc<dyn SttProvider>> = lumo_voice::sherpa::native_sherpa_backend()
            .ok()
            .map(|backend| Arc::new(lumo_voice::sherpa::SherpaSttProvider::new(backend)) as _);
        let stt = SttRouter::new(
            local,
            None,
            SttRouterConfig {
                preference: SttPreference::LocalFirst,
                cloud_allowed: config.cloud_allowed,
                timeout: Duration::from_secs(60),
            },
        );
        let _ = &config.models_root;
        Ok(VoicePipeline {
            capture: if config.device_id == DEFAULT_DEVICE {
                Arc::new(CpalAudioCapture::default_device())
            } else {
                Arc::new(CpalAudioCapture::selected(&config.device_id))
            },
            stt: Arc::new(stt),
        })
    }
}

struct AppVoiceSink {
    app: AppHandle,
}

#[async_trait]
impl VoicePipelineSink for AppVoiceSink {
    async fn state(&self, reason: Option<String>) {
        if let Ok(runtime) = self.app.state::<DesktopState>().voice.lock() {
            emit_voice_state(&self.app, &runtime, reason);
        }
    }

    async fn transcript(&self, text: String, is_final: bool) {
        publish_transcript(&self.app, text, is_final);
    }
}

async fn run_voice_pipeline(
    pipeline: VoicePipeline,
    sink: Arc<dyn VoicePipelineSink>,
    cancel: CancellationToken,
) {
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(32);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
    let capture_cancel = cancel.child_token();
    let capture = tauri::async_runtime::spawn(async move {
        pipeline.capture.capture(audio_tx, capture_cancel).await
    });
    let event_sink = sink.clone();
    let events = tauri::async_runtime::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                SttEvent::Partial(text) => event_sink.transcript(text, false).await,
                SttEvent::Final(text) => event_sink.transcript(text, true).await,
            }
        }
    });
    let result = pipeline
        .stt
        .transcribe(audio_rx, event_tx, cancel.clone())
        .await;
    cancel.cancel();
    let _ = capture.await;
    let _ = events.await;
    sink.state(result.err().map(|error| error.to_string()))
        .await;
}

impl VoiceRuntime {
    fn status(&self) -> VoiceStatusDto {
        VoiceStatusDto {
            state: state_name(self.controller.state()),
            listening: self.controller.state() == VoiceState::Listening,
            permission: self.permission,
            config: self.config.clone(),
            shortcut_registered: self.shortcut_registered,
            shortcut_error: self.shortcut_error.clone(),
        }
    }
}

trait ShortcutRegistry {
    fn replace(&mut self, previous: &str, next: &str) -> Result<(), String>;
}

struct TauriShortcuts<'a> {
    app: &'a AppHandle,
}

impl ShortcutRegistry for TauriShortcuts<'_> {
    fn replace(&mut self, previous: &str, next: &str) -> Result<(), String> {
        if previous == next && self.app.global_shortcut().is_registered(next) {
            return Ok(());
        }
        if self.app.global_shortcut().is_registered(previous) {
            self.app
                .global_shortcut()
                .unregister(previous)
                .map_err(|error| error.to_string())?;
        }
        if let Err(error) = self.app.global_shortcut().register(next) {
            let _ = self.app.global_shortcut().register(previous);
            return Err(format!(
                "global shortcut `{next}` is already in use: {error}"
            ));
        }
        Ok(())
    }
}

pub(super) fn setup_voice_host(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let mut runtime = lock_runtime(&state.voice)?;
    match app
        .global_shortcut()
        .register(runtime.config.shortcut.as_str())
    {
        Ok(()) => {
            runtime.shortcut_registered = true;
            runtime.shortcut_error = None;
        }
        Err(error) => {
            runtime.shortcut_registered = false;
            runtime.shortcut_error = Some(format!(
                "global shortcut `{}` is already in use: {error}",
                runtime.config.shortcut
            ));
        }
    }
    emit_voice_state(app, &runtime, runtime.shortcut_error.clone());
    Ok(())
}

pub(super) fn setup_voice_daemon(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let (action, root) = start_daemon(&state)?;
    spawn_device_monitor(app.clone(), root);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        if let Err(error) = apply_daemon_action(&app, &state, action).await {
            let _ = app.emit("lumo://voice-daemon-error", json!({ "error": error }));
        }
    });
    Ok(())
}

pub(super) fn handle_global_shortcut(app: &AppHandle, event_state: ShortcutState) {
    if event_state != ShortcutState::Pressed {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let listening = state
            .voice
            .lock()
            .map(|runtime| runtime.controller.state().is_active())
            .unwrap_or(false);
        if listening {
            let _ = stop_host(&app, &state);
        } else {
            let _ = start_host(&app, &state).await;
        }
    });
}

#[tauri::command]
pub(super) fn voice_status(state: State<'_, DesktopState>) -> Result<VoiceStatusDto, String> {
    let mut runtime = lock_runtime(&state.voice)?;
    runtime.permission = microphone_permission();
    Ok(runtime.status())
}

#[tauri::command]
pub(super) fn voice_devices() -> Result<Vec<VoiceDeviceDto>, String> {
    platform_voice_devices()
}

#[tauri::command]
pub(super) fn voice_daemon_status(
    state: State<'_, DesktopState>,
) -> Result<VoiceDaemonStatusDto, String> {
    daemon_status(&state)
}

#[tauri::command]
pub(super) async fn voice_daemon_set_enabled(
    app: AppHandle,
    state: State<'_, DesktopState>,
    enabled: bool,
) -> Result<VoiceDaemonStatusDto, String> {
    if enabled {
        let (action, root) = start_daemon(&state)?;
        spawn_device_monitor(app.clone(), root);
        apply_daemon_action(&app, &state, action).await?;
    } else {
        let action = lock_daemon(&state.voice_daemon)?.stop();
        if let Some(action) = action {
            apply_daemon_action(&app, &state, action).await?;
        }
    }
    daemon_status(&state)
}

#[tauri::command]
pub(super) async fn voice_daemon_set_muted(
    app: AppHandle,
    state: State<'_, DesktopState>,
    muted: bool,
) -> Result<VoiceDaemonStatusDto, String> {
    let action = lock_daemon(&state.voice_daemon)?.set_muted(muted);
    if let Some(action) = action {
        apply_daemon_action(&app, &state, action).await?;
    }
    daemon_status(&state)
}

#[tauri::command]
pub(super) async fn voice_daemon_suspend(
    app: AppHandle,
    state: State<'_, DesktopState>,
    reason: String,
) -> Result<VoiceDaemonStatusDto, String> {
    let reason = parse_suspend_reason(&reason)?;
    let action = lock_daemon(&state.voice_daemon)?.suspend(reason);
    if let Some(action) = action {
        apply_daemon_action(&app, &state, action).await?;
    }
    daemon_status(&state)
}

#[tauri::command]
pub(super) async fn voice_daemon_resume(
    app: AppHandle,
    state: State<'_, DesktopState>,
    reason: String,
) -> Result<VoiceDaemonStatusDto, String> {
    let reason = parse_suspend_reason(&reason)?;
    let action = lock_daemon(&state.voice_daemon)?.resume(reason);
    if let Some(action) = action {
        apply_daemon_action(&app, &state, action).await?;
    }
    daemon_status(&state)
}

#[tauri::command]
pub(super) fn voice_daemon_set_device(
    state: State<'_, DesktopState>,
    device_id: String,
) -> Result<VoiceDaemonStatusDto, String> {
    let devices = platform_voice_devices()?;
    if device_id != DEFAULT_DEVICE && !devices.iter().any(|device| device.id == device_id) {
        return Err(format!("voice input device `{device_id}` is not available"));
    }
    let active_device = resolve_active_device(&device_id, &devices)?;
    lock_runtime(&state.voice)?.config.device_id = device_id.clone();
    lock_daemon(&state.voice_daemon)?.select_device(device_id, active_device, Instant::now());
    daemon_status(&state)
}

#[tauri::command]
pub(super) fn voice_configure(
    app: AppHandle,
    state: State<'_, DesktopState>,
    config: VoiceConfigDto,
) -> Result<VoiceStatusDto, String> {
    let devices = platform_voice_devices()?;
    let mut runtime = lock_runtime(&state.voice)?;
    let mut shortcuts = TauriShortcuts { app: &app };
    configure_runtime(&mut runtime, config, &devices, &mut shortcuts)?;
    emit_voice_state(&app, &runtime, None);
    sync_capsule_window(&app, runtime.controller.state());
    Ok(runtime.status())
}

#[tauri::command]
pub(super) async fn voice_start_listening(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<VoiceStatusDto, String> {
    start_host(&app, &state).await
}

#[tauri::command]
pub(super) fn voice_stop(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<VoiceStatusDto, String> {
    stop_host(&app, &state)
}

async fn start_host(app: &AppHandle, state: &DesktopState) -> Result<VoiceStatusDto, String> {
    start_host_with_cancel(app, state, None, None).await
}

async fn start_host_with_cancel(
    app: &AppHandle,
    state: &DesktopState,
    cancel_override: Option<CancellationToken>,
    device_override: Option<String>,
) -> Result<VoiceStatusDto, String> {
    let permission = request_microphone_permission().await?;
    let (pipeline, cancel, status) = {
        let mut runtime = lock_runtime(&state.voice)?;
        runtime.permission = permission;
        start_runtime(&mut runtime, cancel_override)?;
        let pipeline = runtime
            .pipeline_factory
            .create(&VoicePipelineConfig {
                device_id: device_override.unwrap_or_else(|| runtime.config.device_id.clone()),
                models_root: std::env::temp_dir().join("lumo-voice-models"),
                cloud_allowed: false,
            })
            .map_err(|error| error.to_string())?;
        let cancel = runtime
            .cancel
            .clone()
            .ok_or_else(|| "voice cancellation token is unavailable".to_string())?;
        emit_voice_state(app, &runtime, None);
        sync_capsule_window(app, runtime.controller.state());
        (pipeline, cancel, runtime.status())
    };
    let sink = Arc::new(AppVoiceSink { app: app.clone() });
    let task = tauri::async_runtime::spawn(run_voice_pipeline(pipeline, sink, cancel));
    lock_runtime(&state.voice)?.pipeline_task = Some(task);
    Ok(status)
}

fn stop_host(app: &AppHandle, state: &DesktopState) -> Result<VoiceStatusDto, String> {
    let mut runtime = lock_runtime(&state.voice)?;
    stop_runtime(&mut runtime)?;
    emit_voice_state(app, &runtime, Some("cancelled".into()));
    sync_capsule_window(app, runtime.controller.state());
    Ok(runtime.status())
}

fn configure_runtime(
    runtime: &mut VoiceRuntime,
    requested: VoiceConfigDto,
    devices: &[VoiceDeviceDto],
    shortcuts: &mut dyn ShortcutRegistry,
) -> Result<(), String> {
    if requested.shortcut.trim().is_empty() {
        return Err("voice shortcut must not be empty".into());
    }
    if !devices
        .iter()
        .any(|device| device.id == requested.device_id)
    {
        return Err(format!(
            "voice input device `{}` is not available",
            requested.device_id
        ));
    }
    shortcuts.replace(&runtime.config.shortcut, &requested.shortcut)?;

    if !requested.enabled {
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime
            .controller
            .transition(VoiceEvent::Disable)
            .map_err(|error| error.to_string())?;
    } else if runtime.controller.state() == VoiceState::Disabled {
        runtime
            .controller
            .transition(VoiceEvent::Enable)
            .map_err(|error| error.to_string())?;
    }
    runtime.config = requested;
    runtime.shortcut_registered = true;
    runtime.shortcut_error = None;
    Ok(())
}

fn start_runtime(
    runtime: &mut VoiceRuntime,
    cancel_override: Option<CancellationToken>,
) -> Result<(), String> {
    if !runtime.config.enabled || runtime.controller.state() == VoiceState::Disabled {
        return Err("voice listening is disabled".into());
    }
    match runtime.permission {
        MicrophonePermission::Denied => return Err("microphone permission was denied".into()),
        MicrophonePermission::Restricted => {
            return Err("microphone access is restricted by system policy".into())
        }
        MicrophonePermission::NotDetermined => {
            return Err("microphone permission has not been granted".into())
        }
        MicrophonePermission::Granted => {}
    }
    if runtime.controller.state().is_active() {
        return Err("voice listening is already active".into());
    }
    runtime
        .controller
        .transition(VoiceEvent::Wake)
        .and_then(|_| runtime.controller.transition(VoiceEvent::StartListening))
        .map_err(|error| error.to_string())?;
    runtime.cancel = Some(cancel_override.unwrap_or_default());
    Ok(())
}

fn start_daemon(state: &DesktopState) -> Result<(VoiceDaemonAction, CancellationToken), String> {
    let devices = platform_voice_devices()?;
    let config = lock_runtime(&state.voice)?.config.clone();
    let active_device = resolve_active_device(&config.device_id, &devices)?;
    let mut daemon = lock_daemon(&state.voice_daemon)?;
    let action = daemon.start_on_login(config.enabled, &config.device_id, active_device)?;
    let root = daemon
        .root_cancel_token()
        .ok_or_else(|| "voice daemon cancellation root is unavailable".to_string())?;
    Ok((action, root))
}

fn resolve_active_device(
    selected_device: &str,
    devices: &[VoiceDeviceDto],
) -> Result<String, String> {
    if selected_device == DEFAULT_DEVICE {
        return devices
            .iter()
            .find(|device| device.is_default)
            .or_else(|| devices.first())
            .map(|device| device.id.clone())
            .ok_or_else(|| "no voice input device is available".to_string());
    }
    devices
        .iter()
        .find(|device| device.id == selected_device)
        .map(|device| device.id.clone())
        .ok_or_else(|| format!("voice input device `{selected_device}` is not available"))
}

fn spawn_device_monitor(app: AppHandle, root: CancellationToken) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = root.cancelled() => break,
                _ = interval.tick() => {
                    let Ok(devices) = platform_voice_devices() else { continue };
                    let Some(default_device) = devices
                        .iter()
                        .find(|device| device.is_default)
                        .or_else(|| devices.first())
                        .map(|device| device.id.clone())
                    else { continue };
                    let state = app.state::<DesktopState>();
                    let action = {
                        let Ok(mut daemon) = lock_daemon(&state.voice_daemon) else { continue };
                        let now = Instant::now();
                        let active = daemon.active_device().to_string();
                        let removed = !devices.iter().any(|device| device.id == active);
                        let immediate = if removed {
                            daemon.device_removed(&active, &default_device, now)
                        } else {
                            daemon.default_device_changed(&default_device, now)
                        };
                        immediate.or_else(|| daemon.tick(now))
                    };
                    if let Some(action) = action {
                        if let Err(error) = apply_daemon_action(&app, &state, action).await {
                            let _ = app.emit("lumo://voice-daemon-error", json!({ "error": error }));
                        }
                    }
                }
            }
        }
    });
}

async fn apply_daemon_action(
    app: &AppHandle,
    state: &DesktopState,
    action: VoiceDaemonAction,
) -> Result<(), String> {
    match action {
        VoiceDaemonAction::Stop => {
            stop_host(app, state)?;
        }
        VoiceDaemonAction::Start { device_id } => {
            let cancel = lock_daemon(&state.voice_daemon)?
                .capture_cancel_token()
                .ok_or_else(|| "voice daemon capture token is unavailable".to_string())?;
            start_host_with_cancel(app, state, Some(cancel), Some(device_id)).await?;
        }
        VoiceDaemonAction::Restart { device_id } => {
            stop_host(app, state)?;
            let cancel = lock_daemon(&state.voice_daemon)?
                .capture_cancel_token()
                .ok_or_else(|| "voice daemon capture token is unavailable".to_string())?;
            start_host_with_cancel(app, state, Some(cancel), Some(device_id)).await?;
        }
    }
    emit_daemon_state(app, state);
    Ok(())
}

fn daemon_status(state: &DesktopState) -> Result<VoiceDaemonStatusDto, String> {
    let daemon = lock_daemon(&state.voice_daemon)?;
    Ok(VoiceDaemonStatusDto {
        state: daemon.state(),
        muted: daemon.is_muted(),
        blocked: daemon.is_blocked(),
        selected_device: daemon.selected_device().to_string(),
        active_device: daemon.active_device().to_string(),
    })
}

fn emit_daemon_state(app: &AppHandle, state: &DesktopState) {
    if let Ok(status) = daemon_status(state) {
        let _ = app.emit("lumo://voice-daemon-state", status);
    }
}

fn lock_daemon(
    daemon: &Mutex<VoiceDaemon>,
) -> Result<std::sync::MutexGuard<'_, VoiceDaemon>, String> {
    daemon
        .lock()
        .map_err(|_| "voice daemon state is unavailable".to_string())
}

fn parse_suspend_reason(reason: &str) -> Result<VoiceSuspendReason, String> {
    match reason.trim().to_ascii_lowercase().as_str() {
        "sleep" => Ok(VoiceSuspendReason::Sleep),
        "lock" | "screenlock" | "screen_lock" => Ok(VoiceSuspendReason::ScreenLock),
        _ => Err(format!("unknown voice suspension reason `{reason}`")),
    }
}

fn stop_runtime(runtime: &mut VoiceRuntime) -> Result<(), String> {
    if let Some(cancel) = runtime.cancel.take() {
        cancel.cancel();
    }
    if let Some(task) = runtime.pipeline_task.take() {
        task.abort();
    }
    if runtime.controller.state().is_active() {
        runtime
            .controller
            .transition(VoiceEvent::Cancel)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn lock_runtime(
    voice: &Mutex<VoiceRuntime>,
) -> Result<std::sync::MutexGuard<'_, VoiceRuntime>, String> {
    voice
        .lock()
        .map_err(|_| "voice runtime state is unavailable".to_string())
}

fn emit_voice_state(app: &AppHandle, runtime: &VoiceRuntime, reason: Option<String>) {
    let payload = VoiceStatePayload {
        state: state_name(runtime.controller.state()),
        listening: runtime.controller.state() == VoiceState::Listening,
        reason,
    };
    let _ = app.emit("lumo://voice-state", payload);
}

fn sync_capsule_window(app: &AppHandle, state: VoiceState) {
    let Some(window) = app.get_webview_window("voice-capsule") else {
        return;
    };
    let size = if state == VoiceState::Confirming {
        tauri::LogicalSize::new(480.0, 220.0)
    } else {
        tauri::LogicalSize::new(360.0, 64.0)
    };
    let _ = window.set_size(size);
    if state.is_active() {
        let _ = window.show();
    } else {
        let _ = window.hide();
    }
}

/// Provider/service boundary: STT implementations publish partial/final text
/// here. Final text is handed to the single agent-start service event; this
/// host deliberately does not duplicate routing or planning logic.
#[allow(dead_code)]
pub(super) fn publish_transcript(app: &AppHandle, text: String, is_final: bool) {
    let _ = app.emit(
        "lumo://transcript",
        TranscriptPayload {
            text: text.clone(),
            is_final,
        },
    );
    if is_final && !text.trim().is_empty() {
        let _ = app.emit(
            "lumo://agent-start-request",
            AgentStartRequest {
                utterance: text,
                source: "voice",
            },
        );
    }
}

fn state_name(state: VoiceState) -> String {
    format!("{state:?}").to_ascii_lowercase()
}

#[cfg(not(target_os = "macos"))]
fn microphone_permission() -> MicrophonePermission {
    MicrophonePermission::Granted
}

#[cfg(target_os = "macos")]
fn microphone_permission() -> MicrophonePermission {
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return MicrophonePermission::Restricted;
    };
    match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
        AVAuthorizationStatus::Authorized => MicrophonePermission::Granted,
        AVAuthorizationStatus::Denied => MicrophonePermission::Denied,
        AVAuthorizationStatus::Restricted => MicrophonePermission::Restricted,
        _ => MicrophonePermission::NotDetermined,
    }
}

#[cfg(not(target_os = "macos"))]
async fn request_microphone_permission() -> Result<MicrophonePermission, String> {
    Ok(microphone_permission())
}

#[cfg(target_os = "macos")]
async fn request_microphone_permission() -> Result<MicrophonePermission, String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let current = microphone_permission();
    if current != MicrophonePermission::NotDetermined {
        return Ok(current);
    }
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Ok(MicrophonePermission::Restricted);
    };
    let (sender, receiver) = tokio::sync::oneshot::channel();
    {
        let sender = std::sync::Arc::new(Mutex::new(Some(sender)));
        let completion_sender = sender.clone();
        let completion = RcBlock::new(move |granted: Bool| {
            if let Ok(mut sender) = completion_sender.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(granted.as_bool());
                }
            }
        });
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &completion);
        }
    }
    receiver
        .await
        .map(|granted| {
            if granted {
                MicrophonePermission::Granted
            } else {
                MicrophonePermission::Denied
            }
        })
        .map_err(|_| "microphone permission request was cancelled".to_string())
}

#[cfg(not(target_os = "macos"))]
fn platform_voice_devices() -> Result<Vec<VoiceDeviceDto>, String> {
    Ok(vec![VoiceDeviceDto {
        id: DEFAULT_DEVICE.into(),
        name: "System Default".into(),
        is_default: true,
    }])
}

#[cfg(target_os = "macos")]
fn platform_voice_devices() -> Result<Vec<VoiceDeviceDto>, String> {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Err("AVFoundation audio media type is unavailable".into());
    };
    let default_id = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(audio) }
        .map(|device| unsafe { device.uniqueID() }.to_string());
    #[allow(deprecated)]
    let devices = unsafe { AVCaptureDevice::devicesWithMediaType(audio) };
    let mut result = devices
        .iter()
        .map(|device| {
            let id = unsafe { device.uniqueID() }.to_string();
            VoiceDeviceDto {
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                name: unsafe { device.localizedName() }.to_string(),
            }
        })
        .collect::<Vec<_>>();
    if result.is_empty() {
        result.push(VoiceDeviceDto {
            id: DEFAULT_DEVICE.into(),
            name: "System Default".into(),
            is_default: true,
        });
    }
    result.sort_by(|left, right| {
        right
            .is_default
            .cmp(&left.is_default)
            .then(left.name.cmp(&right.name))
    });
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct FakeShortcuts {
        registered: HashSet<String>,
        conflicts: HashSet<String>,
    }

    impl ShortcutRegistry for FakeShortcuts {
        fn replace(&mut self, previous: &str, next: &str) -> Result<(), String> {
            if self.conflicts.contains(next) {
                return Err(format!("global shortcut `{next}` is already in use"));
            }
            self.registered.remove(previous);
            self.registered.insert(next.to_string());
            Ok(())
        }
    }

    fn devices() -> Vec<VoiceDeviceDto> {
        vec![VoiceDeviceDto {
            id: "default".into(),
            name: "System Default".into(),
            is_default: true,
        }]
    }

    #[test]
    fn denied_microphone_permission_is_exposed_in_status() {
        let runtime = VoiceRuntime {
            permission: MicrophonePermission::Denied,
            ..VoiceRuntime::default()
        };

        let status = runtime.status();
        assert_eq!(status.permission, MicrophonePermission::Denied);
        assert!(!status.listening);
    }

    #[test]
    fn shortcut_conflict_preserves_previous_configuration() {
        let mut runtime = VoiceRuntime::default();
        let previous = runtime.config.shortcut.clone();
        let mut shortcuts = FakeShortcuts {
            conflicts: HashSet::from(["CommandOrControl+Shift+V".into()]),
            ..FakeShortcuts::default()
        };
        let requested = VoiceConfigDto {
            enabled: true,
            shortcut: "CommandOrControl+Shift+V".into(),
            device_id: "default".into(),
        };

        let error = configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
            .expect_err("conflicting shortcut must fail");
        assert!(error.contains("already in use"));
        assert_eq!(runtime.config.shortcut, previous);
    }

    #[test]
    fn configuring_an_unknown_device_is_rejected() {
        let mut runtime = VoiceRuntime::default();
        let mut shortcuts = FakeShortcuts::default();
        let requested = VoiceConfigDto {
            enabled: true,
            shortcut: runtime.config.shortcut.clone(),
            device_id: "missing-device".into(),
        };

        let error = configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
            .expect_err("unknown device must fail");
        assert!(error.contains("missing-device"));
    }

    #[test]
    fn authorized_start_and_stop_transition_and_cancel() {
        let mut runtime = VoiceRuntime {
            permission: MicrophonePermission::Granted,
            ..VoiceRuntime::default()
        };

        start_runtime(&mut runtime, None).expect("start listening");
        assert_eq!(
            runtime.controller.state(),
            lumo_voice::VoiceState::Listening
        );
        let cancel = runtime.cancel.clone().expect("active cancellation token");

        stop_runtime(&mut runtime).expect("stop listening");
        assert_eq!(runtime.controller.state(), lumo_voice::VoiceState::Idle);
        assert!(cancel.is_cancelled());
        assert!(runtime.cancel.is_none());
    }

    #[test]
    fn disabled_or_denied_runtime_cannot_start() {
        let mut disabled = VoiceRuntime {
            controller: lumo_voice::VoiceController::new(false),
            config: VoiceConfigDto {
                enabled: false,
                ..VoiceConfigDto::default()
            },
            ..VoiceRuntime::default()
        };
        assert!(start_runtime(&mut disabled, None)
            .unwrap_err()
            .contains("disabled"));

        let mut denied = VoiceRuntime {
            permission: MicrophonePermission::Denied,
            ..VoiceRuntime::default()
        };
        assert!(start_runtime(&mut denied, None)
            .unwrap_err()
            .contains("denied"));
    }
}
