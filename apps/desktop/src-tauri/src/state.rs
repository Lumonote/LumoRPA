use super::*;

#[derive(Default)]
pub(super) struct DesktopState {
    pub(super) recorder: Mutex<RecorderSlot>,
    pub(super) cancels: CancelMap,
    pub(super) prompts: PromptMap,
    pub(super) pending_mcp_imports: Mutex<HashMap<String, lumo_agent::McpImportBatch>>,
    pub(super) mcp: Mutex<McpSupervisorRuntime>,
    pub(super) security: DesktopSecurityRuntime,
    pub(super) voice: Mutex<VoiceRuntime>,
    pub(super) voice_daemon: Mutex<VoiceDaemon>,
    pub(super) agent: Mutex<DesktopAgentRuntime>,
    pub(super) agent_service: Mutex<Option<Arc<DesktopAgentService>>>,
    pub(super) jobs: Mutex<JobRuntime>,
    pub(super) voice_models: Mutex<VoiceModelRuntime>,
    pub(super) voice_history:
        Mutex<std::collections::VecDeque<crate::voice_commands::VoiceHistoryEntry>>,
}

pub(super) type CancelMap = Arc<Mutex<HashMap<String, CancelToken>>>;
pub(super) type PromptMap = Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<Value>>>>;

#[derive(Default)]
pub(super) struct RecorderSlot {
    pub(super) active: Option<RecorderSession>,
}

pub(super) struct RecorderSession {
    pub(super) recorder: Arc<dyn Recorder>,
    pub(super) started_at: chrono::DateTime<chrono::Utc>,
    pub(super) target: String,
    pub(super) backend: String,
    pub(super) forwarder: Option<tokio::task::JoinHandle<()>>,
}
