use super::{
    agent_commands::{active_agent_session_count, cancel_active_agent_sessions},
    app_home,
    voice_daemon::{VoiceDaemon, VoiceDaemonAction, VoiceDaemonState, VoiceSuspendReason},
    voice_intents::{
        coerce_input_value, intent_label, is_affirmative_reply, is_negative_reply,
        match_quick_commands, resolve_flow, validate_quick_commands, FlowResolution,
        MatchedCommand, QuickCommandDto, QuickIntent,
    },
    voice_persona::{is_known_persona, persona, PersonaMoment, DEFAULT_PERSONA_ID},
    DesktopState,
};
use async_trait::async_trait;
use lumo_voice::audio::{AudioFrame, TARGET_SAMPLE_RATE};
use lumo_voice::cloud_stt::{
    CloudAudioScope, CloudSttChunk, CloudSttConfig, CloudSttProvider, CloudSttRequest,
    CloudSttTransport, VaultCredentialRef, VoiceSecretResolver,
};
use lumo_voice::provider::{
    AudioCapture, ProviderError, SttEvent, SttProvider, WakeWordProvider,
};
use lumo_voice::stt_router::{SttPreference, SttRouter, SttRouterConfig, VoicePrivacyPolicy};
use lumo_voice::{cpal_capture::CpalAudioCapture, VoiceController, VoiceEvent, VoiceState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::{collections::VecDeque, path::PathBuf};
use tauri::{Emitter, Manager, State, Wry};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tokio::sync::mpsc;
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
    #[serde(default)]
    pub(super) wake_word_enabled: bool,
    #[serde(default = "default_stt_profile")]
    pub(super) stt_profile: String,
    #[serde(default)]
    pub(super) quiet_mode: bool,
    #[serde(default)]
    pub(super) retain_audio: bool,
    #[serde(default = "default_follow_up_enabled")]
    pub(super) follow_up_enabled: bool,
    #[serde(default = "default_follow_up_timeout_seconds")]
    pub(super) follow_up_timeout_seconds: u64,
    #[serde(default)]
    pub(super) shortcuts: Vec<ShortcutBindingDto>,
    #[serde(default = "default_voice_pack")]
    pub(super) voice_pack: String,
    #[serde(default)]
    pub(super) quick_commands: Vec<QuickCommandDto>,
    /// 内置「运行 X 流程」口令是否需要语音二次确认。
    #[serde(default)]
    pub(super) confirm_flow_run: bool,
    /// 覆盖语音包音色（AVSpeech identifier/语言代码等）；None = 语音包默认。
    #[serde(default)]
    pub(super) tts_voice: Option<String>,
    /// 语速百分比 10..=100（50 为默认），存整数以保持配置可比较。
    #[serde(default)]
    pub(super) tts_rate_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShortcutBindingDto {
    pub(super) id: String,
    pub(super) action: String,
    pub(super) accelerator: String,
    pub(super) enabled: bool,
}

fn default_stt_profile() -> String {
    "local".into()
}

fn default_follow_up_enabled() -> bool {
    true
}

fn default_follow_up_timeout_seconds() -> u64 {
    8
}

fn default_voice_pack() -> String {
    DEFAULT_PERSONA_ID.into()
}

impl Default for VoiceConfigDto {
    fn default() -> Self {
        Self {
            enabled: true,
            shortcut: DEFAULT_SHORTCUT.into(),
            device_id: DEFAULT_DEVICE.into(),
            wake_word_enabled: false,
            stt_profile: default_stt_profile(),
            quiet_mode: false,
            retain_audio: false,
            follow_up_enabled: true,
            follow_up_timeout_seconds: default_follow_up_timeout_seconds(),
            shortcuts: vec![ShortcutBindingDto {
                id: "voice-toggle".into(),
                action: "voice_toggle".into(),
                accelerator: DEFAULT_SHORTCUT.into(),
                enabled: true,
            }],
            voice_pack: default_voice_pack(),
            quick_commands: Vec::new(),
            confirm_flow_run: false,
            tts_voice: None,
            tts_rate_percent: None,
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
    conversation_id: String,
    conversation_context: Vec<String>,
}

pub(super) struct VoiceRuntime {
    pub(super) controller: VoiceController,
    pub(super) config: VoiceConfigDto,
    pub(super) permission: MicrophonePermission,
    pub(super) cancel: Option<CancellationToken>,
    tts_cancel: Option<CancellationToken>,
    conversation_id: Option<String>,
    conversation_turns: VecDeque<String>,
    follow_up_deadline: Option<Instant>,
    /// 进行中的语音对话（二次确认 / 流程参数收集）。
    dialog: Option<VoiceDialog>,
    push_to_talk_guard: Option<CancellationToken>,
    push_to_talk_generation: u64,
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
            tts_cancel: None,
            conversation_id: None,
            conversation_turns: VecDeque::new(),
            follow_up_deadline: None,
            dialog: None,
            push_to_talk_guard: None,
            push_to_talk_generation: 0,
            shortcut_registered: false,
            shortcut_error: None,
            pipeline_factory: Arc::new(DefaultVoicePipelineFactory),
            pipeline_task: None,
        }
    }
}

pub(super) struct VoicePipeline {
    capture: Arc<dyn AudioCapture>,
    wake: Option<Arc<dyn WakeWordProvider>>,
    stt: Arc<dyn SttProvider>,
}

pub(super) struct VoicePipelineConfig {
    device_id: String,
    models_root: PathBuf,
    cloud_allowed: bool,
    wake_word_enabled: bool,
    stt_profile: String,
}

pub(super) trait VoicePipelineFactory: Send + Sync {
    fn create(&self, config: &VoicePipelineConfig) -> Result<VoicePipeline, ProviderError>;
}

#[async_trait]
trait VoicePipelineSink: Send + Sync {
    async fn state(&self, reason: Option<String>);
    async fn transcript(&self, text: String, is_final: bool);
    async fn level(&self, level: f32);
}

struct DefaultVoicePipelineFactory;

struct DesktopVoiceSecretResolver {
    home: PathBuf,
}

#[async_trait]
impl VoiceSecretResolver for DesktopVoiceSecretResolver {
    async fn resolve(&self, reference: &VaultCredentialRef) -> Result<String, ProviderError> {
        let mut parts = reference.key().split('.');
        let name = parts.next().unwrap_or_default();
        let key = parts.collect::<Vec<_>>().join("_");
        let env = format!(
            "LUMO_VAULT_{}_{}",
            sanitize_voice_env(name),
            sanitize_voice_env(&key)
        );
        if let Ok(value) = std::env::var(&env) {
            return Ok(value);
        }
        let identity_path = std::env::var_os("LUMO_VAULT_IDENTITY")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join("age-identity.txt"));
        let identity = lumo_storage::VaultIdentity::load(&identity_path).map_err(|error| {
            ProviderError::Other(format!("voice Vault identity unavailable: {error}"))
        })?;
        let repo = lumo_storage::Repo::open(self.home.join("lumo.db"))
            .map_err(|error| ProviderError::Other(error.to_string()))?;
        lumo_storage::vault::get_field(&repo, &identity, name, &key)
            .map_err(|error| ProviderError::Other(error.to_string()))?
            .ok_or_else(|| ProviderError::InvalidInput {
                message: format!("voice Vault value `{}` is missing", reference.key()),
            })
    }
}

struct OpenAiCloudSttTransport {
    client: reqwest::Client,
}

#[async_trait]
impl CloudSttTransport for OpenAiCloudSttTransport {
    async fn stream(
        &self,
        request: CloudSttRequest,
        mut audio: mpsc::Receiver<AudioFrame>,
        chunks: mpsc::Sender<CloudSttChunk>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if request.audio_scope != CloudAudioScope::PostWakeOnly {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT only accepts post-wake audio".into(),
            });
        }
        let mut samples = Vec::new();
        let mut speech_seen = false;
        loop {
            let wait = if speech_seen {
                Duration::from_millis(1_200)
            } else {
                Duration::from_secs(15)
            };
            let frame = tokio::select! {
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                result = tokio::time::timeout(wait, audio.recv()) => match result { Ok(frame) => frame, Err(_) if speech_seen => None, Err(_) => return Err(ProviderError::Timeout { timeout_ms: 15_000 }) },
            };
            let Some(frame) = frame else { break };
            speech_seen |= frame
                .samples
                .iter()
                .any(|sample| sample.unsigned_abs() > 500);
            samples.extend(frame.samples);
            if samples.len() >= TARGET_SAMPLE_RATE as usize * 30 {
                break;
            }
        }
        if samples.is_empty() {
            return Err(ProviderError::InvalidInput {
                message: "cloud STT received no speech audio".into(),
            });
        }
        let wav = pcm16_wav(&samples);
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("speech.wav")
            .mime_str("audio/wav")
            .map_err(|error| ProviderError::Other(error.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .text("model", request.model)
            .text("response_format", "json")
            .part("file", part);
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            response = self.client.post(request.endpoint).header(reqwest::header::AUTHORIZATION, request.authorization).multipart(form).send() => response.map_err(|error| ProviderError::Other(format!("cloud STT request failed: {error}")))?,
        };
        if !response.status().is_success() {
            return Err(ProviderError::Other(format!(
                "cloud STT returned HTTP {}",
                response.status()
            )));
        }
        let body: Value = response.json().await.map_err(|error| {
            ProviderError::Other(format!("invalid cloud STT response: {error}"))
        })?;
        let text = body
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(ProviderError::Other(
                "cloud STT returned an empty transcript".into(),
            ));
        }
        chunks
            .send(CloudSttChunk::Final(text))
            .await
            .map_err(|_| ProviderError::Other("cloud STT event receiver closed".into()))
    }
}

fn pcm16_wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&TARGET_SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(TARGET_SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn sanitize_voice_env(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

impl VoicePipelineFactory for DefaultVoicePipelineFactory {
    fn create(&self, config: &VoicePipelineConfig) -> Result<VoicePipeline, ProviderError> {
        let backend = lumo_voice::sherpa::native_sherpa_backend().ok();
        let local: Option<Arc<dyn SttProvider>> = backend
            .clone()
            .map(|backend| Arc::new(lumo_voice::sherpa::SherpaSttProvider::new(backend)) as _);
        let wake: Option<Arc<dyn WakeWordProvider>> = config
            .wake_word_enabled
            .then(|| {
                backend.map(|backend| {
                    Arc::new(lumo_voice::sherpa::SherpaWakeWordProvider::new(backend))
                        as Arc<dyn WakeWordProvider>
                })
            })
            .flatten();
        let cloud: Option<Arc<dyn SttProvider>> = if config.cloud_allowed {
            let endpoint = std::env::var("LUMO_CLOUD_STT_ENDPOINT").map_err(|_| {
                ProviderError::InvalidInput {
                    message: "LUMO_CLOUD_STT_ENDPOINT is required for cloud STT".into(),
                }
            })?;
            let model =
                std::env::var("LUMO_CLOUD_STT_MODEL").unwrap_or_else(|_| "whisper-1".into());
            let credential = VaultCredentialRef::parse(
                &std::env::var("LUMO_CLOUD_STT_CREDENTIAL")
                    .unwrap_or_else(|_| "${{ vault.voice.api_key }}".into()),
            )?;
            Some(Arc::new(CloudSttProvider::new(
                CloudSttConfig {
                    endpoint,
                    model,
                    credential,
                    timeout: Duration::from_secs(60),
                    cost_per_audio_second_usd_micro: 0,
                },
                VoicePrivacyPolicy {
                    cloud_allowed: true,
                    retain_transcript: false,
                    retain_audio: false,
                    max_cloud_seconds: 30,
                    max_cost_usd_micro: 0,
                },
                Arc::new(DesktopVoiceSecretResolver {
                    home: config
                        .models_root
                        .parent()
                        .unwrap_or(&config.models_root)
                        .to_path_buf(),
                }),
                Arc::new(OpenAiCloudSttTransport {
                    client: reqwest::Client::new(),
                }),
            )))
        } else {
            None
        };
        let stt = SttRouter::new(
            local,
            cloud,
            SttRouterConfig {
                preference: if config.stt_profile == "cloud" {
                    SttPreference::Cloud
                } else {
                    SttPreference::LocalFirst
                },
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
            wake,
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

    async fn level(&self, level: f32) {
        let _ = self
            .app
            .emit("lumo://voice-level", json!({ "level": level }));
    }
}

async fn run_voice_pipeline(
    pipeline: VoicePipeline,
    sink: Arc<dyn VoicePipelineSink>,
    cancel: CancellationToken,
) {
    if let Some(wake) = pipeline.wake.as_ref() {
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(32);
        let wake_cancel = cancel.child_token();
        let capture_cancel = wake_cancel.clone();
        let capture = pipeline.capture.clone();
        let wake_capture =
            tauri::async_runtime::spawn(
                async move { capture.capture(wake_tx, capture_cancel).await },
            );
        let wake_result = wake.wait_for_wake(wake_rx, cancel.child_token()).await;
        wake_cancel.cancel();
        let _ = wake_capture.await;
        if let Err(error) = wake_result {
            sink.state(Some(error.to_string())).await;
            return;
        }
    }
    let (capture_tx, mut capture_rx) = tokio::sync::mpsc::channel(32);
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel(32);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
    let capture_cancel = cancel.child_token();
    let capture = tauri::async_runtime::spawn(async move {
        pipeline.capture.capture(capture_tx, capture_cancel).await
    });
    let meter_sink = sink.clone();
    let meter = tauri::async_runtime::spawn(async move {
        let mut sequence = 0_u8;
        while let Some(frame) = capture_rx.recv().await {
            if sequence == 0 {
                meter_sink.level(audio_level(&frame)).await;
            }
            sequence = (sequence + 1) % 3;
            if audio_tx.send(frame).await.is_err() {
                break;
            }
        }
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
    let _ = meter.await;
    let _ = events.await;
    sink.state(result.err().map(|error| error.to_string()))
        .await;
}

fn audio_level(frame: &AudioFrame) -> f32 {
    if frame.samples.is_empty() {
        return 0.0;
    }
    let mean_square = frame
        .samples
        .iter()
        .map(|sample| {
            let normalized = f64::from(*sample) / f64::from(i16::MAX);
            normalized * normalized
        })
        .sum::<f64>()
        / frame.samples.len() as f64;
    (mean_square.sqrt() as f32 * 3.2).clamp(0.0, 1.0)
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
    fn replace_all(&mut self, previous: &[String], next: &[String]) -> Result<(), String>;
}

struct TauriShortcuts<'a> {
    app: &'a AppHandle,
}

impl ShortcutRegistry for TauriShortcuts<'_> {
    fn replace_all(&mut self, previous: &[String], next: &[String]) -> Result<(), String> {
        for accelerator in previous {
            if self
                .app
                .global_shortcut()
                .is_registered(accelerator.as_str())
            {
                self.app
                    .global_shortcut()
                    .unregister(accelerator.as_str())
                    .map_err(|error| error.to_string())?;
            }
        }
        let mut registered: Vec<String> = Vec::new();
        for accelerator in next {
            if let Err(error) = self.app.global_shortcut().register(accelerator.as_str()) {
                for added in &registered {
                    let _ = self.app.global_shortcut().unregister(added.as_str());
                }
                for old in previous {
                    let _ = self.app.global_shortcut().register(old.as_str());
                }
                return Err(format!(
                    "global shortcut `{accelerator}` is already in use: {error}"
                ));
            }
            registered.push(accelerator.clone());
        }
        Ok(())
    }
}

pub(super) fn setup_voice_host(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let mut runtime = lock_runtime(&state.voice)?;
    let home = app_home(app)?;
    runtime.config = load_voice_config(&voice_config_path(&home))?;
    normalize_shortcut_config(&mut runtime.config);
    runtime.controller = VoiceController::new(runtime.config.enabled);
    std::env::set_var("LUMO_SHERPA_HOME", home.join("voice-models"));
    let accelerators = enabled_accelerators(&runtime.config);
    match (TauriShortcuts { app }).replace_all(&[], &accelerators) {
        Ok(()) => {
            runtime.shortcut_registered = true;
            runtime.shortcut_error = None;
        }
        Err(error) => {
            runtime.shortcut_registered = false;
            runtime.shortcut_error = Some(format!(
                "one or more global shortcuts could not be registered: {error}"
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

pub(super) fn handle_global_shortcut(
    app: &AppHandle,
    accelerator: &str,
    event_state: ShortcutState,
) {
    let app = app.clone();
    let accelerator = accelerator.to_string();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let action = state
            .voice
            .lock()
            .ok()
            .and_then(|runtime| shortcut_action(&runtime.config, &accelerator));
        match (action.as_deref(), event_state) {
            (Some("voice_toggle"), ShortcutState::Pressed) => {
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
            }
            (Some("push_to_talk"), ShortcutState::Pressed) => {
                let active = state
                    .voice
                    .lock()
                    .map(|runtime| runtime.controller.state().is_active())
                    .unwrap_or(false);
                if !active && start_host(&app, &state).await.is_ok() {
                    arm_push_to_talk_guard(app.clone());
                }
            }
            (Some("push_to_talk"), ShortcutState::Released) => {
                if let Ok(mut runtime) = state.voice.lock() {
                    if let Some(guard) = runtime.push_to_talk_guard.take() {
                        guard.cancel();
                    }
                }
                let _ = stop_host(&app, &state);
            }
            (Some("mission_control"), ShortcutState::Pressed) => {
                open_shortcut_view(&app, "mission-control")
            }
            (Some("capability_hub"), ShortcutState::Pressed) => {
                open_shortcut_view(&app, "capability-hub")
            }
            (Some("stop_interaction"), ShortcutState::Pressed) => {
                let _ = stop_host(&app, &state);
                let _ = cancel_active_agent_sessions(&app, &state);
            }
            _ => {}
        }
    });
}

fn open_shortcut_view(app: &AppHandle, view: &str) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit("lumo://open-view", view);
}

fn arm_push_to_talk_guard(app: AppHandle) {
    let guard = CancellationToken::new();
    let generation = if let Ok(mut runtime) = app.state::<DesktopState>().voice.lock() {
        if let Some(previous) = runtime.push_to_talk_guard.replace(guard.clone()) {
            previous.cancel();
        }
        runtime.push_to_talk_generation = runtime.push_to_talk_generation.wrapping_add(1);
        runtime.push_to_talk_generation
    } else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = guard.cancelled() => {}
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                let state = app.state::<DesktopState>();
                let should_stop = state.voice.lock().map(|mut runtime| {
                    if runtime.push_to_talk_generation == generation && runtime.push_to_talk_guard.is_some() {
                        runtime.push_to_talk_guard.take();
                        true
                    } else { false }
                }).unwrap_or(false);
                if should_stop { let _ = stop_host(&app, &state); }
            }
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
    save_voice_config(&voice_config_path(&app_home(&app)?), &runtime.config)?;
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
    let status = stop_host(&app, &state)?;
    cancel_active_agent_sessions(&app, &state)?;
    Ok(status)
}

async fn start_host(app: &AppHandle, state: &DesktopState) -> Result<VoiceStatusDto, String> {
    start_host_with_cancel(app, state, None, None, false).await
}

async fn start_host_with_cancel(
    app: &AppHandle,
    state: &DesktopState,
    cancel_override: Option<CancellationToken>,
    device_override: Option<String>,
    skip_wake_word: bool,
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
                models_root: app_home(app)?.join("voice-models"),
                cloud_allowed: runtime.config.stt_profile == "cloud",
                wake_word_enabled: runtime.config.wake_word_enabled && !skip_wake_word,
                stt_profile: runtime.config.stt_profile.clone(),
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
    let mut requested = requested;
    normalize_shortcut_config(&mut requested);
    validate_shortcuts(&requested.shortcuts)?;
    validate_quick_commands(&requested.quick_commands)?;
    if !is_known_persona(&requested.voice_pack) {
        return Err(format!(
            "unknown voice pack `{}`; available packs: default, lumo",
            requested.voice_pack
        ));
    }
    if let Some(voice) = requested.tts_voice.take() {
        let trimmed = voice.trim().to_string();
        if !trimmed.is_empty() {
            requested.tts_voice = Some(trimmed);
        }
    }
    if let Some(percent) = requested.tts_rate_percent {
        if !(10..=100).contains(&percent) {
            return Err("voice ttsRatePercent must be between 10 and 100".into());
        }
    }
    if !matches!(requested.stt_profile.as_str(), "local" | "cloud") {
        return Err("voice sttProfile must be `local` or `cloud`".into());
    }
    if requested.retain_audio {
        return Err("raw voice audio retention is not supported by the privacy policy".into());
    }
    if !(3..=30).contains(&requested.follow_up_timeout_seconds) {
        return Err("voice followUpTimeoutSeconds must be between 3 and 30".into());
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
    shortcuts.replace_all(
        &enabled_accelerators(&runtime.config),
        &enabled_accelerators(&requested),
    )?;

    if !requested.enabled {
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = runtime.tts_cancel.take() {
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

fn normalize_shortcut_config(config: &mut VoiceConfigDto) {
    if config.shortcuts.is_empty() && !config.shortcut.trim().is_empty() {
        config.shortcuts.push(ShortcutBindingDto {
            id: "voice-toggle".into(),
            action: "voice_toggle".into(),
            accelerator: config.shortcut.clone(),
            enabled: true,
        });
    }
    if let Some(binding) = config
        .shortcuts
        .iter()
        .find(|binding| binding.action == "voice_toggle" && binding.enabled)
    {
        config.shortcut.clone_from(&binding.accelerator);
    }
}

fn validate_shortcuts(bindings: &[ShortcutBindingDto]) -> Result<(), String> {
    let allowed = [
        "voice_toggle",
        "push_to_talk",
        "mission_control",
        "capability_hub",
        "stop_interaction",
    ];
    let mut accelerators = std::collections::HashSet::new();
    for binding in bindings.iter().filter(|binding| binding.enabled) {
        if binding.id.trim().is_empty() || binding.accelerator.trim().is_empty() {
            return Err("enabled shortcut id and accelerator must not be empty".into());
        }
        if !allowed.contains(&binding.action.as_str()) {
            return Err(format!("unsupported shortcut action `{}`", binding.action));
        }
        if !accelerators.insert(normalize_accelerator(&binding.accelerator)) {
            return Err(format!(
                "duplicate shortcut accelerator `{}`",
                binding.accelerator
            ));
        }
    }
    Ok(())
}

fn enabled_accelerators(config: &VoiceConfigDto) -> Vec<String> {
    config
        .shortcuts
        .iter()
        .filter(|binding| binding.enabled)
        .map(|binding| binding.accelerator.clone())
        .collect()
}

fn shortcut_action(config: &VoiceConfigDto, accelerator: &str) -> Option<String> {
    let needle = normalize_accelerator(accelerator);
    config
        .shortcuts
        .iter()
        .find(|binding| binding.enabled && normalize_accelerator(&binding.accelerator) == needle)
        .map(|binding| binding.action.clone())
}

fn normalize_accelerator(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn voice_config_path(home: &std::path::Path) -> PathBuf {
    home.join("voice-config.json")
}

fn load_voice_config(path: &std::path::Path) -> Result<VoiceConfigDto, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid voice configuration: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(VoiceConfigDto::default()),
        Err(error) => Err(error.to_string()),
    }
}

fn save_voice_config(path: &std::path::Path, config: &VoiceConfigDto) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(config).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    std::fs::rename(temporary, path).map_err(|error| error.to_string())
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
    if let Some(cancel) = runtime.tts_cancel.take() {
        cancel.cancel();
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
            start_host_with_cancel(app, state, Some(cancel), Some(device_id), false).await?;
        }
        VoiceDaemonAction::Restart { device_id } => {
            stop_host(app, state)?;
            let cancel = lock_daemon(&state.voice_daemon)?
                .capture_cancel_token()
                .ok_or_else(|| "voice daemon capture token is unavailable".to_string())?;
            start_host_with_cancel(app, state, Some(cancel), Some(device_id), false).await?;
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
    if let Some(guard) = runtime.push_to_talk_guard.take() {
        guard.cancel();
    }
    if let Some(cancel) = runtime.cancel.take() {
        cancel.cancel();
    }
    if let Some(task) = runtime.pipeline_task.take() {
        task.abort();
    }
    if let Some(cancel) = runtime.tts_cancel.take() {
        cancel.cancel();
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

// ─── 语音快捷指令执行（多指令快速通道，未命中才进 agent 规划） ─────────────

const VOICE_HISTORY_LIMIT: usize = 50;
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);
const DIALOG_TIMEOUT: Duration = Duration::from_secs(25);

/// 流程参数收集中的单个待问参数（IoDeclDto 的可 Clone 精简版）。
#[derive(Debug, Clone)]
pub(super) struct PendingInput {
    name: String,
    kind: String,
    description: Option<String>,
}

/// 进行中的语音对话：下一句 final transcript 优先由它消费。
#[derive(Debug, Clone)]
pub(super) enum VoiceDialog {
    Confirm {
        intents: Vec<QuickIntent>,
        utterance: String,
        deadline: Instant,
    },
    CollectInputs {
        path: String,
        name: String,
        pending: VecDeque<PendingInput>,
        collected: serde_json::Map<String, Value>,
        utterance: String,
        deadline: Instant,
    },
}

fn dialog_deadline(dialog: &VoiceDialog) -> Instant {
    match dialog {
        VoiceDialog::Confirm { deadline, .. } | VoiceDialog::CollectInputs { deadline, .. } => {
            *deadline
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct VoiceHistoryEntry {
    at_ms: i64,
    utterance: String,
    intent: String,
    ok: bool,
    message: String,
}

pub(super) fn push_voice_history(
    app: &AppHandle,
    utterance: &str,
    intent: &str,
    ok: bool,
    message: &str,
) {
    let entry = VoiceHistoryEntry {
        at_ms: chrono::Utc::now().timestamp_millis(),
        utterance: utterance.to_string(),
        intent: intent.to_string(),
        ok,
        message: message.to_string(),
    };
    let state = app.state::<DesktopState>();
    if let Ok(mut history) = state.voice_history.lock() {
        history.push_front(entry.clone());
        history.truncate(VOICE_HISTORY_LIMIT);
    }
    let _ = app.emit("lumo://voice-history", entry);
}

#[tauri::command]
pub(super) fn voice_command_history(
    state: State<'_, DesktopState>,
) -> Result<Vec<VoiceHistoryEntry>, String> {
    Ok(state
        .voice_history
        .lock()
        .map_err(|_| "voice history is unavailable".to_string())?
        .iter()
        .cloned()
        .collect())
}

/// 试听当前音色/语速（显式点击触发，不受安静模式限制）。
#[tauri::command]
pub(super) async fn voice_tts_preview(state: State<'_, DesktopState>) -> Result<(), String> {
    let (options, cancel) = {
        let mut runtime = lock_runtime(&state.voice)?;
        if let Some(cancel) = runtime.tts_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancellationToken::new();
        runtime.tts_cancel = Some(cancel.clone());
        (tts_options(&runtime.config), cancel)
    };
    lumo_voice::system_tts::speak("你好，这是当前语音包的音色与语速。", &options, cancel)
        .await
        .map_err(|error| error.to_string())
}

/// 音色/语速解析：用户覆盖 > 语音包默认；语速百分比映射 0..1。
fn tts_options(config: &VoiceConfigDto) -> lumo_voice::system_tts::SystemTtsOptions {
    let pack = persona(&config.voice_pack);
    lumo_voice::system_tts::SystemTtsOptions {
        voice: config
            .tts_voice
            .clone()
            .filter(|voice| !voice.trim().is_empty())
            .or_else(|| pack.tts_voice.map(str::to_string)),
        rate: config
            .tts_rate_percent
            .map(|percent| f32::from(percent) / 100.0),
    }
}

pub(super) fn handle_quick_commands(
    app: &AppHandle,
    commands: Vec<MatchedCommand>,
    utterance: String,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let confirm_flow_run = {
            let state = app.state::<DesktopState>();
            state
                .voice
                .lock()
                .map(|runtime| runtime.config.confirm_flow_run)
                .unwrap_or(false)
        };
        let needs_confirm = commands.iter().any(|command| {
            command.confirm
                || (confirm_flow_run && matches!(command.intent, QuickIntent::RunFlow { .. }))
        });
        let intents: Vec<QuickIntent> = commands.into_iter().map(|command| command.intent).collect();
        if needs_confirm {
            let labels = intents.iter().map(intent_label).collect::<Vec<_>>().join("、");
            {
                let state = app.state::<DesktopState>();
                let guard = state.voice.lock();
                if let Ok(mut runtime) = guard {
                    runtime.dialog = Some(VoiceDialog::Confirm {
                        intents,
                        utterance,
                        deadline: Instant::now() + CONFIRM_TIMEOUT,
                    });
                }
            }
            announce_dialog_question(
                &app,
                &format!("即将执行：{labels}。确认请说「确认」，放弃请说「取消」"),
            );
            return;
        }
        execute_intents(&app, intents, &utterance).await;
    });
}

async fn execute_intents(app: &AppHandle, intents: Vec<QuickIntent>, utterance: &str) {
    let multi = intents.len() > 1;
    let total = intents.len();
    for intent in intents {
        // 多指令时逐项静默播报，最后统一汇总，避免 TTS 重叠
        dispatch_quick_intent(app, intent, utterance, !multi, !multi).await;
    }
    if multi {
        let detail = format!("已依次执行 {total} 项指令");
        record_agent_outcome(app, &detail);
        announce_feedback(app, PersonaMoment::Success, &detail, true, true);
    }
}

fn quick_feedback(
    app: &AppHandle,
    utterance: &str,
    label: &str,
    moment: PersonaMoment,
    detail: &str,
    speak: bool,
    allow_follow_up: bool,
) {
    push_voice_history(
        app,
        utterance,
        label,
        moment != PersonaMoment::Failure,
        detail,
    );
    record_agent_outcome(app, detail);
    announce_feedback(app, moment, detail, speak, allow_follow_up);
}

async fn dispatch_quick_intent(
    app: &AppHandle,
    intent: QuickIntent,
    utterance: &str,
    speak: bool,
    allow_dialogs: bool,
) {
    let label = intent_label(&intent);
    match intent {
        QuickIntent::OpenView { view, label: view_label } => {
            open_shortcut_view(app, view);
            let detail = format!("已打开{view_label}");
            quick_feedback(app, utterance, &label, PersonaMoment::Success, &detail, speak, true);
        }
        QuickIntent::StopAll => {
            let state = app.state::<DesktopState>();
            let _ = stop_host(app, &state);
            let cancelled = cancel_active_agent_sessions(app, &state)
                .map(|run_ids| run_ids.len())
                .unwrap_or(0);
            let detail = if cancelled > 0 {
                format!("已停止语音并取消 {cancelled} 个进行中的任务")
            } else {
                "已停止当前语音交互".to_string()
            };
            // 用户明确喊停：不再开启续听窗口
            quick_feedback(app, utterance, &label, PersonaMoment::Success, &detail, speak, false);
        }
        QuickIntent::SetMuted(muted) => match set_daemon_muted_internal(app, muted).await {
            Ok(()) => {
                let detail = if muted {
                    "已静音，需要时再唤醒我"
                } else {
                    "已恢复拾音，随时吩咐"
                };
                quick_feedback(app, utterance, &label, PersonaMoment::Success, detail, speak, !muted);
            }
            Err(error) => {
                quick_feedback(
                    app,
                    utterance,
                    &label,
                    PersonaMoment::Failure,
                    &format!("静音设置未能生效：{error}"),
                    speak,
                    false,
                );
            }
        },
        QuickIntent::StartListening => {
            let state = app.state::<DesktopState>();
            // 成功时 start_host 自己会广播 listening 状态；播报反而会打断收音
            match start_host(app, &state).await {
                Ok(_) => push_voice_history(app, utterance, &label, true, "开始听写"),
                Err(error) => {
                    quick_feedback(
                        app,
                        utterance,
                        &label,
                        PersonaMoment::Failure,
                        &format!("未能开始听写：{error}"),
                        false,
                        false,
                    );
                }
            }
        }
        QuickIntent::Status => {
            let detail = compose_status_report(app);
            quick_feedback(app, utterance, &label, PersonaMoment::Status, &detail, speak, true);
        }
        QuickIntent::RunFlow { query } => {
            run_flow_by_voice(app, &query, utterance, speak, allow_dialogs).await;
        }
    }
}

/// 对话答复处理：确认 → 执行/取消；参数收集 → 逐项入库直至运行。
pub(super) fn handle_dialog_reply(app: &AppHandle, dialog: VoiceDialog, reply: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match dialog {
            VoiceDialog::Confirm { intents, utterance, .. } => {
                if is_affirmative_reply(&reply) {
                    execute_intents(&app, intents, &utterance).await;
                    return;
                }
                let message = if is_negative_reply(&reply) {
                    "已取消，本次不执行"
                } else {
                    "没有听到确认口令，已取消。需要时请重新说指令"
                };
                push_voice_history(&app, &utterance, "二次确认", false, message);
                record_agent_outcome(&app, message);
                announce_feedback(&app, PersonaMoment::Success, message, true, false);
            }
            VoiceDialog::CollectInputs {
                path,
                name,
                mut pending,
                mut collected,
                utterance,
                ..
            } => {
                if is_negative_reply(&reply) {
                    let message = format!("已取消流程「{name}」");
                    push_voice_history(&app, &utterance, "参数收集", false, &message);
                    record_agent_outcome(&app, &message);
                    announce_feedback(&app, PersonaMoment::Success, &message, true, false);
                    return;
                }
                let Some(current) = pending.pop_front() else {
                    return;
                };
                match coerce_input_value(&current.kind, &reply) {
                    Ok(value) => {
                        collected.insert(current.name.clone(), value);
                        if let Some(next) = pending.front().cloned() {
                            let question = input_question(&name, &next);
                            rearm_collect_dialog(&app, path, name, pending, collected, utterance);
                            announce_dialog_question(&app, &question);
                        } else {
                            let inputs_json = Value::Object(collected).to_string();
                            execute_voice_flow(&app, &utterance, path, name, inputs_json, true)
                                .await;
                        }
                    }
                    Err(hint) => {
                        let question = format!("{hint}。{}", input_question(&name, &current));
                        pending.push_front(current);
                        rearm_collect_dialog(&app, path, name, pending, collected, utterance);
                        announce_dialog_question(&app, &question);
                    }
                }
            }
        }
    });
}

fn rearm_collect_dialog(
    app: &AppHandle,
    path: String,
    name: String,
    pending: VecDeque<PendingInput>,
    collected: serde_json::Map<String, Value>,
    utterance: String,
) {
    let state = app.state::<DesktopState>();
    let guard = state.voice.lock();
    if let Ok(mut runtime) = guard {
        runtime.dialog = Some(VoiceDialog::CollectInputs {
            path,
            name,
            pending,
            collected,
            utterance,
            deadline: Instant::now() + DIALOG_TIMEOUT,
        });
    }
}

fn input_question(flow: &str, input: &PendingInput) -> String {
    let type_hint = match input.kind.to_ascii_lowercase().as_str() {
        "number" | "integer" | "float" => "（数字）",
        "boolean" | "bool" => "（是/否）",
        _ => "",
    };
    match input.description.as_deref().map(str::trim) {
        Some(description) if !description.is_empty() => format!(
            "流程「{flow}」参数「{}」{type_hint}：{description}。请说出它的值",
            input.name
        ),
        _ => format!("请说出流程「{flow}」参数「{}」的值{type_hint}", input.name),
    }
}

/// 提问 + 免唤醒开启一轮收音等待回答；胶囊进入 confirming 态。
fn announce_dialog_question(app: &AppHandle, message: &str) {
    let (options, quiet, tts_cancel) = {
        let state = app.state::<DesktopState>();
        let Ok(mut runtime) = state.voice.lock() else {
            return;
        };
        if let Some(cancel) = runtime.tts_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancellationToken::new();
        if !runtime.config.quiet_mode {
            runtime.tts_cancel = Some(cancel.clone());
        }
        (tts_options(&runtime.config), runtime.config.quiet_mode, cancel)
    };
    let _ = app.emit(
        "lumo://voice-state",
        VoiceStatePayload {
            state: "confirming".into(),
            listening: false,
            reason: Some(message.to_string()),
        },
    );
    let message = message.to_string();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if !quiet {
            let _ = lumo_voice::system_tts::speak(&message, &options, tts_cancel.clone()).await;
        }
        if tts_cancel.is_cancelled() {
            return;
        }
        let state = app.state::<DesktopState>();
        let _ = start_host_with_cancel(&app, &state, None, None, true).await;
    });
}

async fn set_daemon_muted_internal(app: &AppHandle, muted: bool) -> Result<(), String> {
    let state = app.state::<DesktopState>();
    let action = lock_daemon(&state.voice_daemon)?.set_muted(muted);
    if let Some(action) = action {
        apply_daemon_action(app, &state, action).await?;
    } else {
        emit_daemon_state(app, &state);
    }
    let _ = app.emit("lumo://voice-muted", json!({ "muted": muted }));
    Ok(())
}

fn compose_status_report(app: &AppHandle) -> String {
    let state = app.state::<DesktopState>();
    let daemon_part = match daemon_status(&state) {
        Ok(status) if status.muted => "语音守护已静音".to_string(),
        Ok(status) => match status.state {
            VoiceDaemonState::Running => "语音守护运行中".to_string(),
            VoiceDaemonState::Suspended => "语音守护已挂起".to_string(),
            VoiceDaemonState::Muted => "语音守护已静音".to_string(),
            VoiceDaemonState::Stopped => "语音守护已停止".to_string(),
        },
        Err(_) => "语音守护状态未知".to_string(),
    };
    let active = active_agent_session_count(&state);
    let agent_part = if active > 0 {
        format!("{active} 个任务执行中")
    } else {
        "暂无进行中的任务".to_string()
    };
    format!("{daemon_part}，{agent_part}")
}

async fn run_flow_by_voice(
    app: &AppHandle,
    query: &str,
    utterance: &str,
    speak: bool,
    allow_dialogs: bool,
) {
    let label = format!("运行流程「{query}」");
    let flows = match crate::list_flow_library(app.clone()) {
        Ok(flows) => flows,
        Err(error) => {
            quick_feedback(
                app,
                utterance,
                &label,
                PersonaMoment::Failure,
                &format!("无法读取流程库：{error}"),
                speak,
                false,
            );
            return;
        }
    };
    match resolve_flow(query, &flows) {
        FlowResolution::Unique { path, name } => {
            let missing: Vec<PendingInput> = flows
                .iter()
                .find(|flow| flow.path == path)
                .map(|flow| {
                    flow.inputs
                        .iter()
                        .filter(|input| input.required && input.default.is_none())
                        .map(|input| PendingInput {
                            name: input.name.clone(),
                            kind: input.kind.clone(),
                            description: input.description.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if missing.is_empty() {
                execute_voice_flow(app, utterance, path, name, "{}".to_string(), speak).await;
                return;
            }
            if !allow_dialogs {
                quick_feedback(
                    app,
                    utterance,
                    &label,
                    PersonaMoment::Failure,
                    &format!(
                        "流程「{name}」需要 {} 个参数，请单独说「运行{name}」再补充",
                        missing.len()
                    ),
                    speak,
                    false,
                );
                return;
            }
            let count = missing.len();
            let first_question = input_question(&name, &missing[0]);
            rearm_collect_dialog(
                app,
                path,
                name.clone(),
                missing.into(),
                serde_json::Map::new(),
                utterance.to_string(),
            );
            announce_dialog_question(
                app,
                &format!("流程「{name}」需要 {count} 个参数。{first_question}"),
            );
        }
        FlowResolution::Ambiguous(names) => {
            quick_feedback(
                app,
                utterance,
                &label,
                PersonaMoment::Failure,
                &format!("找到多个匹配的流程：{}。请说得更具体", names.join("、")),
                speak,
                true,
            );
        }
        FlowResolution::NotFound => {
            quick_feedback(
                app,
                utterance,
                &label,
                PersonaMoment::Failure,
                &format!("没有找到名为「{query}」的流程"),
                speak,
                true,
            );
        }
    }
}

async fn execute_voice_flow(
    app: &AppHandle,
    utterance: &str,
    path: String,
    name: String,
    inputs_json: String,
    speak: bool,
) {
    let label = format!("运行流程「{name}」");
    let ack = format!("运行流程「{name}」");
    record_agent_outcome(app, &ack);
    announce_feedback(app, PersonaMoment::Ack, &ack, speak, false);
    let result = crate::run_flow(
        app.clone(),
        app.state::<DesktopState>(),
        path,
        inputs_json,
        false,
    )
    .await;
    let ok = matches!(&result, Ok(response) if response.report.success);
    let detail = match result {
        Ok(response) if response.report.success => format!(
            "流程「{name}」执行完成，{}/{} 步成功",
            response.report.steps_ok, response.report.steps_total
        ),
        Ok(response) => format!(
            "流程「{name}」执行失败，{} 步出错",
            response.report.steps_failed
        ),
        Err(error) => format!("流程「{name}」未能运行：{error}"),
    };
    quick_feedback(
        app,
        utterance,
        &label,
        if ok {
            PersonaMoment::Success
        } else {
            PersonaMoment::Failure
        },
        &detail,
        speak,
        true,
    );
}

/// Provider/service boundary: STT implementations publish partial/final text
/// here. Final text first passes the local quick-command layer (毫秒级执行，
/// 不经 LLM)；未命中的话语走 single agent-start service event; this
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
        let state = app.state::<DesktopState>();
        // 1) 进行中的语音对话（二次确认 / 参数收集）优先消费本句；过期则丢弃
        let dialog = state.voice.lock().ok().and_then(|mut runtime| {
            match runtime.dialog.take() {
                Some(dialog) if dialog_deadline(&dialog) < Instant::now() => None,
                other => other,
            }
        });
        if let Some(dialog) = dialog {
            handle_dialog_reply(app, dialog, text);
            return;
        }
        // 2) 快捷指令（含「然后」连接的多指令）
        let quick = state
            .voice
            .lock()
            .ok()
            .and_then(|runtime| match_quick_commands(&text, &runtime.config.quick_commands));
        if let Some(commands) = quick {
            handle_quick_commands(app, commands, text);
            return;
        }
        // 3) 回落 agent 规划
        let request = state
            .voice
            .lock()
            .ok()
            .map(|mut runtime| conversation_request(&mut runtime, text.clone()))
            .unwrap_or_else(|| AgentStartRequest {
                utterance: text,
                source: "voice",
                conversation_id: ulid::Ulid::new().to_string(),
                conversation_context: Vec::new(),
            });
        let _ = app.emit("lumo://agent-start-request", request);
    }
}

fn conversation_request(runtime: &mut VoiceRuntime, utterance: String) -> AgentStartRequest {
    if runtime
        .follow_up_deadline
        .is_some_and(|deadline| Instant::now() > deadline)
    {
        clear_conversation(runtime);
    }
    let conversation_id = runtime
        .conversation_id
        .get_or_insert_with(|| ulid::Ulid::new().to_string())
        .clone();
    let conversation_context = runtime.conversation_turns.iter().cloned().collect();
    runtime
        .conversation_turns
        .push_back(format!("用户：{utterance}"));
    while runtime.conversation_turns.len() > 6 {
        runtime.conversation_turns.pop_front();
    }
    runtime.follow_up_deadline = None;
    AgentStartRequest {
        utterance,
        source: "voice",
        conversation_id,
        conversation_context,
    }
}

fn clear_conversation(runtime: &mut VoiceRuntime) {
    runtime.conversation_id = None;
    runtime.conversation_turns.clear();
    runtime.follow_up_deadline = None;
}

pub(super) fn record_agent_outcome(app: &AppHandle, message: &str) {
    if let Ok(mut runtime) = app.state::<DesktopState>().voice.lock() {
        if runtime.conversation_id.is_some() {
            runtime
                .conversation_turns
                .push_back(format!("助手：{message}"));
            while runtime.conversation_turns.len() > 6 {
                runtime.conversation_turns.pop_front();
            }
        }
    }
}

pub(super) fn report_agent_start_failure(app: &AppHandle, error: &str) {
    let _ = app.emit(
        "lumo://voice-state",
        VoiceStatePayload {
            state: "error".into(),
            listening: false,
            reason: Some(error.into()),
        },
    );
    record_agent_outcome(app, "任务未能启动");
    announce_agent_status(app, "任务未能启动，请调整指令后重试");
}

pub(super) fn announce_agent_status(app: &AppHandle, message: impl Into<String>) {
    let message = message.into();
    let ok = !(message.contains("失败") || message.contains("未能"));
    let moment = if ok {
        PersonaMoment::Success
    } else {
        PersonaMoment::Failure
    };
    announce_feedback(app, moment, &message, true, true);
}

/// 统一结果反馈通道：胶囊状态（reporting/error）+ `lumo://voice-feedback` +
/// 主窗口 toast + 语音包（人格）化 TTS 播报（尊重安静模式）+ 可选续听。
pub(super) fn announce_feedback(
    app: &AppHandle,
    moment: PersonaMoment,
    detail: &str,
    speak: bool,
    allow_follow_up: bool,
) {
    let ok = moment != PersonaMoment::Failure;
    let (message, persona_name, options, quiet, follow_up, timeout_seconds, tts_cancel) = {
        let state = app.state::<DesktopState>();
        let Ok(mut runtime) = state.voice.lock() else {
            return;
        };
        let voice = persona(&runtime.config.voice_pack);
        let message = voice.render(moment, detail);
        if let Some(cancel) = runtime.tts_cancel.take() {
            cancel.cancel();
        }
        let cancel = CancellationToken::new();
        if !runtime.config.quiet_mode && speak {
            runtime.tts_cancel = Some(cancel.clone());
        }
        (
            message,
            voice.display_name,
            tts_options(&runtime.config),
            runtime.config.quiet_mode,
            allow_follow_up && runtime.config.follow_up_enabled,
            runtime.config.follow_up_timeout_seconds,
            cancel,
        )
    };
    let visual_state = if ok { "reporting" } else { "error" };
    let _ = app.emit(
        "lumo://voice-state",
        VoiceStatePayload {
            state: visual_state.into(),
            listening: false,
            reason: Some(message.clone()),
        },
    );
    let _ = app.emit(
        "lumo://voice-feedback",
        json!({ "ok": ok, "message": message }),
    );
    let _ = app.emit(
        "lumo://toast",
        json!({
            "kind": if ok { "ok" } else { "error" },
            "title": persona_name,
            "message": message,
        }),
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if !quiet && speak {
            let _ = lumo_voice::system_tts::speak(&message, &options, tts_cancel.clone()).await;
        }
        if follow_up && !tts_cancel.is_cancelled() {
            schedule_follow_up(app, timeout_seconds).await;
        } else if !tts_cancel.is_cancelled() {
            let _ = app.emit(
                "lumo://voice-state",
                VoiceStatePayload {
                    state: "idle".into(),
                    listening: false,
                    reason: None,
                },
            );
        }
    });
}

async fn schedule_follow_up(app: AppHandle, timeout_seconds: u64) {
    let state = app.state::<DesktopState>();
    let daemon_ready = state
        .voice_daemon
        .lock()
        .map(|daemon| daemon.state() == VoiceDaemonState::Running)
        .unwrap_or(false);
    if !daemon_ready {
        return;
    }
    let conversation_id = state.voice.lock().ok().and_then(|mut runtime| {
        runtime.follow_up_deadline = Some(Instant::now() + Duration::from_secs(timeout_seconds));
        runtime.conversation_id.clone()
    });
    let cancel = CancellationToken::new();
    let timeout_cancel = cancel.clone();
    let timeout_app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(timeout_seconds)).await;
        timeout_cancel.cancel();
        if let Ok(mut runtime) = timeout_app.state::<DesktopState>().voice.lock() {
            let same_session = runtime.conversation_id == conversation_id;
            let expired = runtime
                .follow_up_deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
            if same_session && expired {
                clear_conversation(&mut runtime);
            }
        }
    });
    let follow_up_message = state
        .voice
        .lock()
        .ok()
        .map(|runtime| persona(&runtime.config.voice_pack).render(PersonaMoment::FollowUp, ""))
        .unwrap_or_default();
    let _ = app.emit(
        "lumo://voice-follow-up",
        json!({ "timeoutSeconds": timeout_seconds, "message": follow_up_message }),
    );
    let _ = start_host_with_cancel(&app, &state, Some(cancel), None, true).await;
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
        fn replace_all(&mut self, previous: &[String], next: &[String]) -> Result<(), String> {
            if let Some(conflict) = next
                .iter()
                .find(|accelerator| self.conflicts.contains(*accelerator))
            {
                return Err(format!("global shortcut `{conflict}` is already in use"));
            }
            for accelerator in previous {
                self.registered.remove(accelerator);
            }
            self.registered.extend(next.iter().cloned());
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
            shortcuts: vec![ShortcutBindingDto {
                id: "voice-toggle".into(),
                action: "voice_toggle".into(),
                accelerator: "CommandOrControl+Shift+V".into(),
                enabled: true,
            }],
            ..VoiceConfigDto::default()
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
            ..VoiceConfigDto::default()
        };

        let error = configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
            .expect_err("unknown device must fail");
        assert!(error.contains("missing-device"));
    }

    #[test]
    fn multiple_shortcuts_route_actions_and_reject_duplicates() {
        let config = VoiceConfigDto {
            shortcuts: vec![
                ShortcutBindingDto {
                    id: "voice".into(),
                    action: "voice_toggle".into(),
                    accelerator: "CommandOrControl+Shift+Space".into(),
                    enabled: true,
                },
                ShortcutBindingDto {
                    id: "mission".into(),
                    action: "mission_control".into(),
                    accelerator: "CommandOrControl+Shift+M".into(),
                    enabled: true,
                },
            ],
            ..VoiceConfigDto::default()
        };
        validate_shortcuts(&config.shortcuts).unwrap();
        assert_eq!(
            shortcut_action(&config, "commandorcontrol+shift+m").as_deref(),
            Some("mission_control")
        );
        let duplicates = vec![
            config.shortcuts[0].clone(),
            ShortcutBindingDto {
                id: "duplicate".into(),
                action: "push_to_talk".into(),
                accelerator: " commandorcontrol+shift+space ".into(),
                enabled: true,
            },
        ];
        assert!(validate_shortcuts(&duplicates)
            .unwrap_err()
            .contains("duplicate"));
    }

    #[test]
    fn authorized_start_and_stop_transition_and_cancel() {
        let mut runtime = VoiceRuntime {
            permission: MicrophonePermission::Granted,
            ..VoiceRuntime::default()
        };
        let tts_cancel = CancellationToken::new();
        runtime.tts_cancel = Some(tts_cancel.clone());

        start_runtime(&mut runtime, None).expect("start listening");
        assert!(
            tts_cancel.is_cancelled(),
            "listening must interrupt active TTS"
        );
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
    fn voice_configuration_round_trips_all_privacy_and_routing_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("voice.json");
        let config = VoiceConfigDto {
            enabled: true,
            shortcut: "CommandOrControl+Shift+L".into(),
            device_id: "default".into(),
            wake_word_enabled: true,
            stt_profile: "cloud".into(),
            quiet_mode: true,
            retain_audio: false,
            follow_up_enabled: true,
            follow_up_timeout_seconds: 12,
            shortcuts: vec![ShortcutBindingDto {
                id: "voice-toggle".into(),
                action: "voice_toggle".into(),
                accelerator: "CommandOrControl+Shift+L".into(),
                enabled: true,
            }],
            voice_pack: "lumo".into(),
            quick_commands: vec![QuickCommandDto {
                id: "qc-1".into(),
                phrase: "开工".into(),
                action: "run_flow".into(),
                argument: "晨间日报".into(),
                enabled: true,
                confirm: true,
            }],
            confirm_flow_run: true,
            tts_voice: Some("zh-CN".into()),
            tts_rate_percent: Some(65),
        };
        save_voice_config(&path, &config).unwrap();
        assert_eq!(load_voice_config(&path).unwrap(), config);
    }

    #[test]
    fn legacy_voice_config_defaults_new_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("voice.json");
        std::fs::write(
            &path,
            br#"{"enabled":true,"shortcut":"CommandOrControl+Shift+Space","deviceId":"default"}"#,
        )
        .unwrap();
        let config = load_voice_config(&path).unwrap();
        assert_eq!(config.voice_pack, "default");
        assert!(config.quick_commands.is_empty());
    }

    #[test]
    fn configure_validates_voice_pack_and_quick_commands() {
        let mut runtime = VoiceRuntime::default();
        let mut shortcuts = FakeShortcuts::default();
        let requested = VoiceConfigDto {
            voice_pack: "alien".into(),
            ..VoiceConfigDto::default()
        };
        assert!(
            configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
                .unwrap_err()
                .contains("voice pack")
        );
        let requested = VoiceConfigDto {
            quick_commands: vec![QuickCommandDto {
                id: "qc-1".into(),
                phrase: "开工".into(),
                action: "reboot".into(),
                argument: String::new(),
                enabled: true,
                confirm: false,
            }],
            ..VoiceConfigDto::default()
        };
        assert!(
            configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
                .unwrap_err()
                .contains("unsupported")
        );
        let requested = VoiceConfigDto {
            voice_pack: "lumo".into(),
            quick_commands: vec![QuickCommandDto {
                id: "qc-1".into(),
                phrase: "开工".into(),
                action: "open_view".into(),
                argument: "mission-control".into(),
                enabled: true,
                confirm: false,
            }],
            ..VoiceConfigDto::default()
        };
        configure_runtime(&mut runtime, requested, &devices(), &mut shortcuts)
            .expect("valid lumo pack with quick command");
        assert_eq!(runtime.config.voice_pack, "lumo");
        assert_eq!(runtime.config.quick_commands.len(), 1);
    }

    #[test]
    fn cloud_transport_encodes_valid_pcm16_wav_header() {
        let wav = pcm16_wav(&[1, -1, 2]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
    }

    #[test]
    fn audio_meter_is_normalized_and_handles_silence() {
        assert_eq!(audio_level(&AudioFrame { samples: vec![] }), 0.0);
        assert_eq!(
            audio_level(&AudioFrame {
                samples: vec![0; 64]
            }),
            0.0
        );
        let loud = audio_level(&AudioFrame {
            samples: vec![i16::MAX; 64],
        });
        assert!((0.99..=1.0).contains(&loud));
    }

    #[test]
    fn conversation_context_is_bounded_reused_and_expires() {
        let mut runtime = VoiceRuntime::default();
        let first = conversation_request(&mut runtime, "打开日报".into());
        assert!(first.conversation_context.is_empty());
        runtime.follow_up_deadline = Some(Instant::now() + Duration::from_secs(2));
        let second = conversation_request(&mut runtime, "再归档它".into());
        assert_eq!(second.conversation_id, first.conversation_id);
        assert_eq!(second.conversation_context, vec!["用户：打开日报"]);

        runtime.follow_up_deadline = Some(Instant::now() - Duration::from_millis(1));
        let expired = conversation_request(&mut runtime, "新任务".into());
        assert_ne!(expired.conversation_id, first.conversation_id);
        assert!(expired.conversation_context.is_empty());
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
