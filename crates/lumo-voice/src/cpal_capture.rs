use async_trait::async_trait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::audio::{AudioFrame, TARGET_SAMPLE_RATE};
use crate::provider::{AudioCapture, ProviderError};

pub struct CpalAudioCapture {
    device_name: Option<String>,
}

impl CpalAudioCapture {
    pub fn default_device() -> Self {
        Self { device_name: None }
    }
    pub fn selected(name: impl Into<String>) -> Self {
        Self {
            device_name: Some(name.into()),
        }
    }

    fn device(&self) -> Result<Device, ProviderError> {
        let host = cpal::default_host();
        if let Some(name) = &self.device_name {
            return host
                .input_devices()
                .map_err(other)?
                .find(|device| device.name().ok().as_deref() == Some(name.as_str()))
                .ok_or_else(|| ProviderError::InvalidInput {
                    message: format!("audio input device `{name}` was not found"),
                });
        }
        host.default_input_device()
            .ok_or(ProviderError::Unavailable)
    }
}

pub fn convert_interleaved_f32(
    input: &[f32],
    channels: usize,
    sample_rate: u32,
) -> Result<AudioFrame, ProviderError> {
    if channels == 0 || sample_rate == 0 {
        return Err(ProviderError::InvalidInput {
            message: "audio channels and sample rate must be non-zero".into(),
        });
    }
    let mono = input
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect::<Vec<_>>();
    if sample_rate == TARGET_SAMPLE_RATE {
        return Ok(AudioFrame::from_f32_interleaved(&mono, 1));
    }
    let output_len =
        ((mono.len() as u64 * TARGET_SAMPLE_RATE as u64) / sample_rate as u64) as usize;
    let mut output = Vec::with_capacity(output_len);
    for index in 0..output_len {
        let source = index as f64 * sample_rate as f64 / TARGET_SAMPLE_RATE as f64;
        let left = source.floor() as usize;
        let right = (left + 1).min(mono.len().saturating_sub(1));
        let fraction = (source - left as f64) as f32;
        output.push(
            mono.get(left).copied().unwrap_or(0.0) * (1.0 - fraction)
                + mono.get(right).copied().unwrap_or(0.0) * fraction,
        );
    }
    Ok(AudioFrame::from_f32_interleaved(&output, 1))
}

fn other(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::Other(error.to_string())
}

#[async_trait]
impl AudioCapture for CpalAudioCapture {
    async fn capture(
        &self,
        frames: mpsc::Sender<AudioFrame>,
        cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let device_name = self.device_name.clone();
        tokio::task::spawn_blocking(move || {
            let capture = CpalAudioCapture { device_name };
            let device = capture.device()?;
            let supported = device.default_input_config().map_err(other)?;
            let sample_format = supported.sample_format();
            let config: StreamConfig = supported.into();
            let channels = config.channels as usize;
            let sample_rate = config.sample_rate.0;
            let error = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
            let stream = match sample_format {
                SampleFormat::F32 => {
                    let sender = frames.clone();
                    let error_out = error.clone();
                    device.build_input_stream(
                        &config,
                        move |data: &[f32], _| {
                            if let Ok(frame) = convert_interleaved_f32(data, channels, sample_rate)
                            {
                                let _ = sender.try_send(frame);
                            }
                        },
                        move |e| {
                            if let Ok(mut slot) = error_out.lock() {
                                *slot = Some(e.to_string());
                            }
                        },
                        None,
                    )
                }
                SampleFormat::I16 => {
                    let sender = frames.clone();
                    let error_out = error.clone();
                    device.build_input_stream(
                        &config,
                        move |data: &[i16], _| {
                            let data = data
                                .iter()
                                .map(|v| *v as f32 / i16::MAX as f32)
                                .collect::<Vec<_>>();
                            if let Ok(frame) = convert_interleaved_f32(&data, channels, sample_rate)
                            {
                                let _ = sender.try_send(frame);
                            }
                        },
                        move |e| {
                            if let Ok(mut slot) = error_out.lock() {
                                *slot = Some(e.to_string());
                            }
                        },
                        None,
                    )
                }
                SampleFormat::U16 => {
                    let error_out = error.clone();
                    device.build_input_stream(
                        &config,
                        move |data: &[u16], _| {
                            let data = data
                                .iter()
                                .map(|v| (*v as f32 / u16::MAX as f32) * 2.0 - 1.0)
                                .collect::<Vec<_>>();
                            if let Ok(frame) = convert_interleaved_f32(&data, channels, sample_rate)
                            {
                                let _ = frames.try_send(frame);
                            }
                        },
                        move |e| {
                            if let Ok(mut slot) = error_out.lock() {
                                *slot = Some(e.to_string());
                            }
                        },
                        None,
                    )
                }
                format => {
                    return Err(ProviderError::Other(format!(
                        "unsupported audio sample format {format:?}"
                    )))
                }
            }
            .map_err(other)?;
            stream.play().map_err(other)?;
            while !cancel.is_cancelled() {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(stream);
            let message = error
                .lock()
                .map_err(|_| ProviderError::Other("audio error lock poisoned".into()))?
                .take();
            if let Some(message) = message {
                return Err(ProviderError::Other(message));
            }
            Err(ProviderError::Cancelled)
        })
        .await
        .map_err(|error| ProviderError::Other(error.to_string()))?
    }
}
