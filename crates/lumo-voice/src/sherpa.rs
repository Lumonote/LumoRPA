//! Replaceable Sherpa-compatible wake-word and streaming STT boundary.
//!
//! This crate intentionally does not link a native `sherpa-onnx` library yet.
//! [`SherpaBackend`] is the native integration seam; the deterministic backend
//! keeps provider, routing, cancellation and UI contracts testable meanwhile.

use crate::audio::AudioFrame;
use crate::model_installer::VoiceModelKind;
use crate::provider::{ProviderError, SttEvent, SttProvider, WakeWordProvider};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SherpaModelDescriptor {
    pub id: String,
    pub kind: VoiceModelKind,
    pub required_files: Vec<PathBuf>,
}

pub fn sherpa_model_catalog() -> Vec<SherpaModelDescriptor> {
    vec![
        SherpaModelDescriptor {
            id: "sherpa-kws-en-v1".into(),
            kind: VoiceModelKind::WakeWord,
            required_files: vec![
                "encoder.onnx".into(),
                "decoder.onnx".into(),
                "joiner.onnx".into(),
                "tokens.txt".into(),
                "keywords.txt".into(),
            ],
        },
        SherpaModelDescriptor {
            id: "sherpa-stt-en-v1".into(),
            kind: VoiceModelKind::SpeechToText,
            required_files: vec![
                "encoder.onnx".into(),
                "decoder.onnx".into(),
                "joiner.onnx".into(),
                "tokens.txt".into(),
            ],
        },
    ]
}

pub fn validate_sherpa_models(
    root: &Path,
    models: &[SherpaModelDescriptor],
) -> Result<(), ProviderError> {
    let mut missing = Vec::new();
    for model in models {
        let directory = root.join(&model.id);
        let complete = model.required_files.iter().all(|relative| {
            std::fs::metadata(directory.join(relative))
                .map(|metadata| metadata.is_file() && metadata.len() > 0)
                .unwrap_or(false)
        });
        if !complete {
            missing.push(model.id.clone());
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ProviderError::ModelMissing { asset_ids: missing })
    }
}

#[async_trait]
pub trait SherpaBackend: Send + Sync {
    async fn wait_for_wake(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;

    async fn transcribe(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError>;
}

pub fn native_sherpa_backend() -> Result<Arc<dyn SherpaBackend>, ProviderError> {
    Err(ProviderError::NativeUnavailable {
        backend: "sherpa-onnx".into(),
    })
}

#[derive(Debug, Clone)]
pub struct DeterministicSherpaBackend {
    wake_threshold: i16,
    partials: Vec<String>,
    final_transcript: String,
}

impl DeterministicSherpaBackend {
    pub fn new(wake_threshold: i16, partials: Vec<String>, final_transcript: String) -> Self {
        Self {
            wake_threshold: wake_threshold.saturating_abs(),
            partials,
            final_transcript,
        }
    }
}

#[async_trait]
impl SherpaBackend for DeterministicSherpaBackend {
    async fn wait_for_wake(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                frame = audio.recv() => frame,
            };
            let Some(frame) = frame else {
                return Err(ProviderError::NoWakeDetected);
            };
            if frame.samples.iter().any(|sample| {
                i32::from(*sample).unsigned_abs() >= i32::from(self.wake_threshold).unsigned_abs()
            }) {
                return Ok(());
            }
        }
    }

    async fn transcribe(
        &self,
        mut audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        let mut partial_index = 0;
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                frame = audio.recv() => frame,
            };
            let Some(_frame) = frame else {
                break;
            };
            if let Some(partial) = self.partials.get(partial_index) {
                send_event(&events, SttEvent::Partial(partial.clone()), &cancel).await?;
                partial_index += 1;
            }
        }
        send_event(
            &events,
            SttEvent::Final(self.final_transcript.clone()),
            &cancel,
        )
        .await
    }
}

async fn send_event(
    events: &mpsc::Sender<SttEvent>,
    event: SttEvent,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::Cancelled),
        result = events.send(event) => result.map_err(|_| ProviderError::Other("STT event receiver closed".into())),
    }
}

pub struct SherpaWakeWordProvider {
    backend: Arc<dyn SherpaBackend>,
}

impl SherpaWakeWordProvider {
    pub fn new(backend: Arc<dyn SherpaBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl WakeWordProvider for SherpaWakeWordProvider {
    async fn wait_for_wake(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.backend.wait_for_wake(audio, cancel).await
    }
}

pub struct SherpaSttProvider {
    backend: Arc<dyn SherpaBackend>,
}

impl SherpaSttProvider {
    pub fn new(backend: Arc<dyn SherpaBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl SttProvider for SherpaSttProvider {
    async fn transcribe(
        &self,
        audio: mpsc::Receiver<AudioFrame>,
        events: mpsc::Sender<SttEvent>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        self.backend.transcribe(audio, events, cancel).await
    }
}
