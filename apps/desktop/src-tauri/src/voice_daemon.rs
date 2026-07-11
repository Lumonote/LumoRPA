use serde::Serialize;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum VoiceDaemonState {
    Running,
    Suspended,
    Muted,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VoiceSuspendReason {
    Sleep,
    ScreenLock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VoiceDaemonAction {
    Start { device_id: String },
    Stop,
    Restart { device_id: String },
}

pub(super) struct VoiceDaemon {
    state: VoiceDaemonState,
    debounce: Duration,
    root: Option<CancellationToken>,
    capture: Option<CancellationToken>,
    selected_device: String,
    active_device: String,
    pending_device: Option<(String, Instant)>,
    sleeping: bool,
    screen_locked: bool,
    muted: bool,
}

impl Default for VoiceDaemon {
    fn default() -> Self {
        Self::new(Duration::from_millis(500))
    }
}

impl VoiceDaemon {
    pub(super) fn new(debounce: Duration) -> Self {
        Self {
            state: VoiceDaemonState::Stopped,
            debounce,
            root: None,
            capture: None,
            selected_device: "default".into(),
            active_device: "default".into(),
            pending_device: None,
            sleeping: false,
            screen_locked: false,
            muted: false,
        }
    }

    pub(super) fn state(&self) -> VoiceDaemonState {
        self.state
    }

    pub(super) fn selected_device(&self) -> &str {
        &self.selected_device
    }

    pub(super) fn active_device(&self) -> &str {
        &self.active_device
    }

    pub(super) fn is_muted(&self) -> bool {
        self.muted
    }

    pub(super) fn is_blocked(&self) -> bool {
        self.sleeping || self.screen_locked
    }

    pub(super) fn root_cancel_token(&self) -> Option<CancellationToken> {
        self.root.clone()
    }

    pub(super) fn capture_cancel_token(&self) -> Option<CancellationToken> {
        self.capture.clone()
    }

    pub(super) fn start_on_login(
        &mut self,
        enabled: bool,
        selected_device: impl Into<String>,
        active_device: impl Into<String>,
    ) -> Result<VoiceDaemonAction, String> {
        if self.root.is_some() {
            return Err("voice daemon is already started".into());
        }
        if !enabled {
            return Err("voice daemon is disabled".into());
        }
        self.selected_device = selected_device.into();
        self.active_device = active_device.into();
        self.root = Some(CancellationToken::new());
        self.state = VoiceDaemonState::Running;
        self.start_capture();
        Ok(VoiceDaemonAction::Start {
            device_id: self.active_device.clone(),
        })
    }

    pub(super) fn suspend(&mut self, reason: VoiceSuspendReason) -> Option<VoiceDaemonAction> {
        match reason {
            VoiceSuspendReason::Sleep => self.sleeping = true,
            VoiceSuspendReason::ScreenLock => self.screen_locked = true,
        }
        if self.muted || self.root.is_none() {
            return None;
        }
        self.state = VoiceDaemonState::Suspended;
        self.stop_capture()
    }

    pub(super) fn resume(&mut self, reason: VoiceSuspendReason) -> Option<VoiceDaemonAction> {
        match reason {
            VoiceSuspendReason::Sleep => self.sleeping = false,
            VoiceSuspendReason::ScreenLock => self.screen_locked = false,
        }
        if self.root.is_none() {
            self.state = VoiceDaemonState::Stopped;
            return None;
        }
        if self.muted {
            self.state = VoiceDaemonState::Muted;
            return None;
        }
        if self.is_blocked() {
            self.state = VoiceDaemonState::Suspended;
            return None;
        }
        if self.capture.is_some() {
            self.state = VoiceDaemonState::Running;
            return None;
        }
        self.state = VoiceDaemonState::Running;
        self.start_capture();
        Some(VoiceDaemonAction::Start {
            device_id: self.active_device.clone(),
        })
    }

    pub(super) fn set_muted(&mut self, muted: bool) -> Option<VoiceDaemonAction> {
        if self.root.is_none() {
            self.muted = muted;
            self.state = VoiceDaemonState::Stopped;
            return None;
        }
        self.muted = muted;
        if muted {
            self.state = VoiceDaemonState::Muted;
            return self.stop_capture();
        }
        if self.is_blocked() {
            self.state = VoiceDaemonState::Suspended;
            return None;
        }
        self.state = VoiceDaemonState::Running;
        if self.capture.is_some() {
            None
        } else {
            self.start_capture();
            Some(VoiceDaemonAction::Start {
                device_id: self.active_device.clone(),
            })
        }
    }

    pub(super) fn select_device(
        &mut self,
        selected_device: impl Into<String>,
        active_device: impl Into<String>,
        now: Instant,
    ) {
        self.selected_device = selected_device.into();
        self.pending_device = Some((active_device.into(), now + self.debounce));
    }

    pub(super) fn device_removed(
        &mut self,
        removed_device: &str,
        fallback_device: impl Into<String>,
        now: Instant,
    ) -> Option<VoiceDaemonAction> {
        if removed_device != self.active_device {
            return None;
        }
        let fallback_device = fallback_device.into();
        self.selected_device = "default".into();
        self.pending_device = Some((fallback_device, now + self.debounce));
        if self.root.is_some() && !self.muted {
            self.state = VoiceDaemonState::Suspended;
        }
        self.stop_capture()
    }

    pub(super) fn default_device_changed(
        &mut self,
        device_id: impl Into<String>,
        now: Instant,
    ) -> Option<VoiceDaemonAction> {
        if self.selected_device != "default" {
            return None;
        }
        let device_id = device_id.into();
        if device_id == self.active_device {
            return None;
        }
        self.pending_device = Some((device_id, now + self.debounce));
        None
    }

    pub(super) fn tick(&mut self, now: Instant) -> Option<VoiceDaemonAction> {
        let (device_id, deadline) = self.pending_device.as_ref()?;
        if now < *deadline {
            return None;
        }
        let device_id = device_id.clone();
        self.pending_device = None;
        self.stop_capture();
        self.active_device.clone_from(&device_id);
        if self.root.is_none() {
            self.state = VoiceDaemonState::Stopped;
            return None;
        }
        if self.muted {
            self.state = VoiceDaemonState::Muted;
            return None;
        }
        if self.is_blocked() {
            self.state = VoiceDaemonState::Suspended;
            return None;
        }
        self.state = VoiceDaemonState::Running;
        self.start_capture();
        Some(VoiceDaemonAction::Restart { device_id })
    }

    pub(super) fn stop(&mut self) -> Option<VoiceDaemonAction> {
        let root = self.root.take()?;
        let action = self.stop_capture();
        root.cancel();
        self.pending_device = None;
        self.state = VoiceDaemonState::Stopped;
        action
    }

    fn start_capture(&mut self) {
        if let Some(root) = &self.root {
            self.capture = Some(root.child_token());
        }
    }

    fn stop_capture(&mut self) -> Option<VoiceDaemonAction> {
        self.capture.take().map(|capture| {
            capture.cancel();
            VoiceDaemonAction::Stop
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const DEBOUNCE: Duration = Duration::from_millis(250);

    #[test]
    fn login_start_owns_one_root_and_rejects_duplicate_start() {
        let mut daemon = VoiceDaemon::new(DEBOUNCE);

        let action = daemon
            .start_on_login(true, "default", "mic-a")
            .expect("first start");
        assert_eq!(daemon.state(), VoiceDaemonState::Running);
        assert_eq!(
            action,
            VoiceDaemonAction::Start {
                device_id: "mic-a".into()
            }
        );
        assert!(!daemon.root_cancel_token().unwrap().is_cancelled());

        let error = daemon
            .start_on_login(true, "default", "mic-a")
            .expect_err("duplicate start must fail");
        assert!(error.contains("already started"));
    }

    #[test]
    fn sleep_and_lock_suspend_capture_until_each_reason_resumes() {
        let mut daemon = VoiceDaemon::new(DEBOUNCE);
        daemon.start_on_login(true, "default", "mic-a").unwrap();

        assert_eq!(
            daemon.suspend(VoiceSuspendReason::Sleep),
            Some(VoiceDaemonAction::Stop)
        );
        assert_eq!(daemon.state(), VoiceDaemonState::Suspended);
        assert_eq!(daemon.suspend(VoiceSuspendReason::ScreenLock), None);
        assert_eq!(daemon.resume(VoiceSuspendReason::Sleep), None);
        assert_eq!(daemon.state(), VoiceDaemonState::Suspended);
        assert_eq!(
            daemon.resume(VoiceSuspendReason::ScreenLock),
            Some(VoiceDaemonAction::Start {
                device_id: "mic-a".into()
            })
        );
        assert_eq!(daemon.state(), VoiceDaemonState::Running);
    }

    #[test]
    fn hard_mute_stops_capture_and_unmute_respects_screen_lock() {
        let mut daemon = VoiceDaemon::new(DEBOUNCE);
        daemon.start_on_login(true, "default", "mic-a").unwrap();

        assert_eq!(daemon.set_muted(true), Some(VoiceDaemonAction::Stop));
        assert_eq!(daemon.state(), VoiceDaemonState::Muted);
        daemon.suspend(VoiceSuspendReason::ScreenLock);
        assert_eq!(daemon.set_muted(false), None);
        assert_eq!(daemon.state(), VoiceDaemonState::Suspended);
        assert_eq!(
            daemon.resume(VoiceSuspendReason::ScreenLock),
            Some(VoiceDaemonAction::Start {
                device_id: "mic-a".into()
            })
        );
    }

    #[test]
    fn removed_device_stops_immediately_and_restarts_after_debounce() {
        let now = Instant::now();
        let mut daemon = VoiceDaemon::new(DEBOUNCE);
        daemon.start_on_login(true, "mic-a", "mic-a").unwrap();

        assert_eq!(
            daemon.device_removed("mic-a", "mic-b", now),
            Some(VoiceDaemonAction::Stop)
        );
        assert_eq!(daemon.tick(now + DEBOUNCE - Duration::from_millis(1)), None);
        assert_eq!(
            daemon.tick(now + DEBOUNCE),
            Some(VoiceDaemonAction::Restart {
                device_id: "mic-b".into()
            })
        );
        assert_eq!(daemon.state(), VoiceDaemonState::Running);
    }

    #[test]
    fn default_device_change_is_debounced_and_never_restarts_while_locked() {
        let now = Instant::now();
        let mut daemon = VoiceDaemon::new(DEBOUNCE);
        daemon.start_on_login(true, "default", "mic-a").unwrap();

        assert_eq!(daemon.default_device_changed("mic-b", now), None);
        daemon.suspend(VoiceSuspendReason::ScreenLock);
        assert_eq!(daemon.tick(now + DEBOUNCE), None);
        assert_eq!(daemon.state(), VoiceDaemonState::Suspended);
        assert_eq!(
            daemon.resume(VoiceSuspendReason::ScreenLock),
            Some(VoiceDaemonAction::Start {
                device_id: "mic-b".into()
            })
        );
    }

    #[test]
    fn stop_cancels_the_single_root_and_is_idempotent() {
        let mut daemon = VoiceDaemon::new(DEBOUNCE);
        daemon.start_on_login(true, "default", "mic-a").unwrap();
        let root = daemon.root_cancel_token().unwrap();

        assert_eq!(daemon.stop(), Some(VoiceDaemonAction::Stop));
        assert!(root.is_cancelled());
        assert_eq!(daemon.state(), VoiceDaemonState::Stopped);
        assert_eq!(daemon.stop(), None);
    }
}
