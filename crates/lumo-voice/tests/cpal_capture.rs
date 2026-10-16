//! 麦克风采集集成测试。随 `capture` feature 编译：`cpal_capture` 模块本身在
//! 该 feature 关闭时不存在（见 crates/lumo-voice/src/lib.rs），默认构建不编译
//! 这个文件。CI 的 `cargo test -p lumo-voice --features capture` 步骤负责跑到它。
//!
//! 注意 `convert_interleaved_f32` 是纯函数（重采样/混音），本可不依赖 cpal；
//! 但它定义在 `cpal_capture` 模块内，故一并门控。
#![cfg(feature = "capture")]

use lumo_voice::cpal_capture::{convert_interleaved_f32, CpalAudioCapture};
use lumo_voice::provider::{AudioCapture, ProviderError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
fn converts_stereo_48khz_to_mono_16khz() {
    let mut input = Vec::new();
    for index in 0..480 {
        let sample = index as f32 / 480.0;
        input.extend([sample, sample]);
    }

    let frame = convert_interleaved_f32(&input, 2, 48_000).unwrap();
    assert_eq!(frame.samples.len(), 160);
    assert!(frame.samples.windows(2).all(|pair| pair[0] <= pair[1]));
}

#[test]
fn conversion_rejects_invalid_stream_metadata() {
    assert!(matches!(
        convert_interleaved_f32(&[0.0], 0, 48_000),
        Err(ProviderError::InvalidInput { .. })
    ));
    assert!(matches!(
        convert_interleaved_f32(&[0.0], 1, 0),
        Err(ProviderError::InvalidInput { .. })
    ));
}

#[tokio::test]
async fn pre_cancelled_capture_never_touches_the_audio_device() {
    let capture = CpalAudioCapture::selected("device-that-does-not-exist");
    let (frames, _receiver) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    cancel.cancel();

    assert!(matches!(
        capture.capture(frames, cancel).await,
        Err(ProviderError::Cancelled)
    ));
}
