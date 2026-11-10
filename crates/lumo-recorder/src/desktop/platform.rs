//! Platform back-ends for the desktop recorder. Each module implements
//! [`Backend::poll`] in terms of native accessibility APIs (NSAccessibility
//! on macOS, UI Automation on Windows). Unsupported platforms fall back to
//! the stub which surfaces "is_supported = false" so the host UI can show
//! a friendly "platform recorder not available" banner.

use async_trait::async_trait;
use std::sync::Arc;

use super::input::{CaptureGuard, InputEmitter};
use super::FocusSnapshot;

#[async_trait]
pub trait Backend: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str;
    fn is_supported(&self) -> bool;
    async fn poll(&self) -> anyhow::Result<FocusSnapshot>;
    /// Tests and metadata-only remote backends may omit a native listener.
    fn start_input(&self, _emitter: InputEmitter) -> anyhow::Result<Option<CaptureGuard>> {
        Ok(None)
    }
}

/// Pick the right back-end for the current OS. Behind `cfg`s so the
/// non-target platforms never bring in their FFI deps.
pub fn default_backend() -> Arc<dyn Backend> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacOsBackend::new())
    }
    #[cfg(target_os = "windows")]
    {
        return Arc::new(windows_be::WindowsBackend::new());
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::LinuxBackend)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Arc::new(stub::StubBackend)
    }
}

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
mod macos_input;

#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
pub mod windows_be;
#[cfg(target_os = "windows")]
mod windows_input;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
pub mod stub {
    use super::*;

    #[derive(Debug, Default)]
    pub struct StubBackend;

    #[async_trait]
    impl Backend for StubBackend {
        fn name(&self) -> &'static str {
            "stub"
        }
        fn is_supported(&self) -> bool {
            false
        }
        async fn poll(&self) -> anyhow::Result<FocusSnapshot> {
            Ok(FocusSnapshot::default())
        }
    }
}
