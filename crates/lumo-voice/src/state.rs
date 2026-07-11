use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceState {
    Disabled,
    Idle,
    WakeDetected,
    Listening,
    Transcribing,
    Routing,
    Planning,
    Confirming,
    Executing,
    Reporting,
    Error,
}

impl VoiceState {
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Disabled | Self::Idle | Self::Error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceEvent {
    Enable,
    Disable,
    Wake,
    StartListening,
    AudioEnded,
    TranscriptReady,
    RouteReady,
    PlanReady,
    ConfirmationRequired,
    Confirmed,
    ExecutionFinished,
    ReportFinished,
    Fail,
    Recover,
    Cancel,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VoiceStateError {
    #[error("invalid voice transition from {state:?} on {event:?}")]
    InvalidTransition {
        state: VoiceState,
        event: VoiceEvent,
    },
}

#[derive(Debug, Clone)]
pub struct VoiceController {
    state: VoiceState,
}

impl VoiceController {
    pub fn new(enabled: bool) -> Self {
        Self {
            state: if enabled {
                VoiceState::Idle
            } else {
                VoiceState::Disabled
            },
        }
    }
    pub fn state(&self) -> VoiceState {
        self.state
    }
    pub fn transition(&mut self, event: VoiceEvent) -> Result<VoiceState, VoiceStateError> {
        use VoiceEvent as E;
        use VoiceState as S;
        let next = match (self.state, event) {
            (S::Disabled, E::Enable) => S::Idle,
            (_, E::Disable) => S::Disabled,
            (S::Idle, E::Wake) => S::WakeDetected,
            (S::WakeDetected, E::Wake) => S::WakeDetected,
            (S::WakeDetected, E::StartListening) => S::Listening,
            (S::Listening, E::AudioEnded) => S::Transcribing,
            (S::Transcribing, E::TranscriptReady) => S::Routing,
            (S::Routing, E::RouteReady) => S::Planning,
            (S::Planning, E::ConfirmationRequired) => S::Confirming,
            (S::Planning, E::PlanReady) => S::Executing,
            (S::Confirming, E::Confirmed) => S::Executing,
            (S::Executing, E::ExecutionFinished) => S::Reporting,
            (S::Reporting, E::ReportFinished | E::Recover) | (S::Error, E::Recover) => S::Idle,
            (s, E::Cancel) if s.is_active() => S::Idle,
            (s, E::Fail) if s.is_active() => S::Error,
            (state, event) => return Err(VoiceStateError::InvalidTransition { state, event }),
        };
        self.state = next;
        Ok(next)
    }
}
