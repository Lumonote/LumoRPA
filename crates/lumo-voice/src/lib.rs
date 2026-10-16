pub mod audio;
pub mod cloud_stt;
/// 麦克风采集，随 `capture` feature 编译（见 Cargo.toml：cpal 在 Linux 上
/// 需要 ALSA 开发包，在交叉编译时需要目标 sysroot）。默认关闭。
#[cfg(feature = "capture")]
pub mod cpal_capture;
pub mod macos_tts;
pub mod model_installer;
pub mod provider;
pub mod sherpa;
pub mod sherpa_native;
pub mod state;
pub mod stt_router;
pub mod system_tts;

pub use audio::{AudioFrame, PreRollBuffer, TARGET_SAMPLE_RATE};
pub use state::{VoiceController, VoiceEvent, VoiceState, VoiceStateError};
