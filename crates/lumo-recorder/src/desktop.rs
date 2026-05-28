//! Desktop recorder (R-02). Captures cross-application user activity into
//! the same [`RawEvent`] stream the [`BrowserRecorder`] uses, so the YAML
//! patch converter and Studio UI treat both lanes uniformly.
//!
//! Capture pipeline:
//! - `tokio::time::interval` polls the platform layer at ~5 Hz.
//! - Each tick yields a [`FocusSnapshot`] describing the foreground app +
//!   window + (where available) the focused accessibility role / value.
//! - Comparing against the previous snapshot generates one of:
//!   * `desktop.focus_changed` — the user switched windows;
//!   * `desktop.app_changed`   — the user switched apps;
//!   * `desktop.focus_field`   — the focused control changed inside the
//!                                same window (typing focus moved).
//! - A heartbeat is emitted every 5 s so Studio can confirm the recorder
//!   is alive even when the user is idle.
//!
//! Platform back-ends (`macos`, `windows`, `stub`) all implement the same
//! [`platform::Backend`] trait. Adding Linux means dropping a file under
//! `platform/linux.rs` and wiring it up in `mod.rs` — nothing else changes.
//!
//! AccessKit proper (the Rust accessibility framework) is a *provider* API:
//! it lets apps publish a11y trees, not consume them. So this module talks
//! to the OS-native consumer APIs (NSAccessibility on macOS, UIA on
//! Windows) and translates their outputs into AccessKit-shaped fields
//! (`role`, `name`, `value`) — that's why callers see the trait surface
//! described as "AccessKit" in the design docs even though no AccessKit
//! crate is pulled in.

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::{push_event, RawEvent, RawEventSender, Recorder, SharedBuffer};

pub mod platform;

/// One read of the platform foreground state. All fields are best-effort —
/// platforms answer "unknown" with an empty string rather than `None` to
/// keep the JSON payloads uniform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusSnapshot {
    pub app: String,
    pub pid: i32,
    pub window_title: String,
    pub focused_role: String,
    pub focused_name: String,
    pub focused_value: String,
}

impl FocusSnapshot {
    /// True when *nothing* in the snapshot is interesting. Used to skip
    /// emitting an event when the platform layer has no data (e.g. the
    /// stub backend on unsupported OSes).
    pub fn is_empty(&self) -> bool {
        self.app.is_empty()
            && self.window_title.is_empty()
            && self.focused_role.is_empty()
            && self.focused_name.is_empty()
            && self.focused_value.is_empty()
    }
}

pub struct DesktopRecorder {
    buffer: SharedBuffer,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Override the platform backend (used by tests + future
    /// "remote control" deployments where the foreground state arrives
    /// from a sidecar process rather than this OS).
    backend: Mutex<Option<Arc<dyn platform::Backend>>>,
}

impl Default for DesktopRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopRecorder {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            tasks: Mutex::new(Vec::new()),
            backend: Mutex::new(None),
        }
    }

    /// Inject a test backend. Production callers don't need this; they get
    /// the platform default via [`platform::default_backend`].
    pub fn with_backend(self, backend: Arc<dyn platform::Backend>) -> Self {
        *self.backend.lock() = Some(backend);
        self
    }
}

#[async_trait]
impl Recorder for DesktopRecorder {
    async fn start(&self, live: Option<RawEventSender>) -> anyhow::Result<()> {
        self.buffer.lock().clear();
        let backend = self
            .backend
            .lock()
            .clone()
            .unwrap_or_else(platform::default_backend);
        push_event(
            &self.buffer,
            &live,
            RawEvent::new(
                "desktop",
                "launched",
                serde_json::json!({
                    "backend": backend.name(),
                    "supported": backend.is_supported(),
                }),
            ),
        );

        let buffer = self.buffer.clone();
        let live_cloned = live.clone();
        let polling_backend = backend.clone();
        let poll = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(200));
            let mut last: FocusSnapshot = FocusSnapshot::default();
            tick.tick().await;
            loop {
                tick.tick().await;
                let snap = match polling_backend.poll().await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("desktop poll: {e}");
                        continue;
                    }
                };
                if snap.is_empty() || snap == last {
                    continue;
                }
                let kind = if snap.app != last.app {
                    "app_changed"
                } else if snap.window_title != last.window_title {
                    "focus_changed"
                } else {
                    "focus_field"
                };
                let payload = serde_json::to_value(&snap).unwrap_or(serde_json::Value::Null);
                push_event(
                    &buffer,
                    &live_cloned,
                    RawEvent::new("desktop", kind, payload),
                );
                last = snap;
            }
        });

        // Heartbeat for UI liveness.
        let buffer = self.buffer.clone();
        let live_cloned = live;
        let beat = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            let mut n = 0u64;
            tick.tick().await;
            loop {
                tick.tick().await;
                n += 1;
                push_event(
                    &buffer,
                    &live_cloned,
                    RawEvent::new("desktop", "heartbeat", serde_json::json!({ "n": n })),
                );
            }
        });
        let mut tasks = self.tasks.lock();
        tasks.push(poll);
        tasks.push(beat);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>> {
        for t in self.tasks.lock().drain(..) {
            t.abort();
        }
        Ok(std::mem::take(&mut *self.buffer.lock()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct ScriptedBackend {
        snapshots: Mutex<Vec<FocusSnapshot>>,
    }

    impl ScriptedBackend {
        fn new(snaps: Vec<FocusSnapshot>) -> Arc<Self> {
            Arc::new(Self {
                snapshots: Mutex::new(snaps),
            })
        }
    }

    #[async_trait]
    impl platform::Backend for ScriptedBackend {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn is_supported(&self) -> bool {
            true
        }
        async fn poll(&self) -> anyhow::Result<FocusSnapshot> {
            let mut g = self.snapshots.lock();
            if g.is_empty() {
                return Ok(FocusSnapshot::default());
            }
            Ok(g.remove(0))
        }
    }

    fn snap(app: &str, win: &str, role: &str) -> FocusSnapshot {
        FocusSnapshot {
            app: app.into(),
            pid: 0,
            window_title: win.into(),
            focused_role: role.into(),
            focused_name: String::new(),
            focused_value: String::new(),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recorder_emits_app_change_then_focus_field() {
        let backend = ScriptedBackend::new(vec![
            snap("Chrome", "GitHub", "AXTextField"),
            snap("Chrome", "GitHub", "AXButton"),
            snap("Notes", "Notes", "AXTextArea"),
        ]);
        let rec = DesktopRecorder::new().with_backend(backend);
        rec.start(None).await.unwrap();
        // Polling cadence is 200 ms — wait long enough to drain three
        // scripted snapshots with margin for slow CI runners.
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let events = rec.stop().await.unwrap();
        let kinds: Vec<_> = events.iter().map(|e| e.kind.as_str()).collect();
        // We expect at minimum: launched, app_changed (first non-empty),
        // focus_field (button vs textfield), app_changed (Notes vs Chrome).
        assert!(kinds.contains(&"launched"), "missing launched event");
        assert!(kinds.contains(&"app_changed"), "missing app_changed event");
        assert!(
            kinds.contains(&"focus_field"),
            "missing focus_field event in {kinds:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn recorder_skips_empty_snapshots() {
        let backend = ScriptedBackend::new(vec![FocusSnapshot::default(); 5]);
        let rec = DesktopRecorder::new().with_backend(backend);
        rec.start(None).await.unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        let events = rec.stop().await.unwrap();
        // No focus events expected, only the `launched` banner (heartbeat may
        // or may not have fired given the 5s cadence vs paused time).
        for e in &events {
            assert!(
                matches!(e.kind.as_str(), "launched" | "heartbeat"),
                "unexpected event kind `{}` for empty snapshots",
                e.kind
            );
        }
    }

    #[test]
    fn focus_snapshot_default_is_empty() {
        assert!(FocusSnapshot::default().is_empty());
    }

    #[test]
    fn focus_snapshot_with_content_not_empty() {
        let s = snap("X", "win", "role");
        assert!(!s.is_empty());
    }
}
