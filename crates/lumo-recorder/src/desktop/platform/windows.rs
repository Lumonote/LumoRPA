//! Windows desktop recorder back-end.
//!
//! Uses the foreground-window Win32 API to fish out the app, PID, and
//! window title. Full UI Automation focus walking is deferred — the focused
//! AXElement story on Windows requires a COM apartment and the
//! `IUIAutomation::GetFocusedElement` round-trip, which is non-trivial to
//! pump from a tokio task. What we ship today is enough for the recorder
//! to emit `desktop.focus_changed` and `desktop.app_changed` events; a
//! follow-up patch can layer in IUIAutomation against the same trait
//! without touching `desktop.rs`.

use async_trait::async_trait;

use super::Backend;
use crate::desktop::FocusSnapshot;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
};

#[derive(Debug, Default)]
pub struct WindowsBackend;

impl WindowsBackend {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Backend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }
    fn is_supported(&self) -> bool {
        true
    }
    async fn poll(&self) -> anyhow::Result<FocusSnapshot> {
        // The Win32 surface is sync. Hop to a blocking task so we don't
        // stall the tokio reactor — the call itself is microseconds, but
        // doing this consistently lets us add UIA (which blocks on COM)
        // later without changing the call site.
        let snap = tokio::task::spawn_blocking(read_foreground)
            .await
            .map_err(|e| anyhow::anyhow!("desktop poll join: {e}"))??;
        Ok(snap)
    }
}

fn read_foreground() -> anyhow::Result<FocusSnapshot> {
    let hwnd: HWND = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        return Ok(FocusSnapshot::default());
    }
    let mut buf = [0u16; 512];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    let title = if len > 0 {
        String::from_utf16_lossy(&buf[..len as usize])
    } else {
        String::new()
    };
    let mut pid: u32 = 0;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
    }
    Ok(FocusSnapshot {
        // No quick + cheap way to get the process executable name without
        // opening the process handle (which can fail in low-IL apps). Leave
        // `app` empty for now; `desktop.rs` keys uniqueness off window title
        // + pid so we still emit `focus_changed` correctly.
        app: String::new(),
        pid: pid as i32,
        window_title: title,
        focused_role: String::new(),
        focused_name: String::new(),
        focused_value: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_metadata_is_stable() {
        let b = WindowsBackend::new();
        assert_eq!(b.name(), "windows");
        assert!(b.is_supported());
    }
}
