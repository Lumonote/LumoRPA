//! Session-owned input capture and the platform-independent replay event reducer.
//!
//! Printable key presses are deliberately never recorded. Text comes only from
//! accessibility controls that the platform positively identifies as non-secret.

use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::RawEvent;

pub type EventSink = Arc<dyn Fn(RawEvent) + Send + Sync>;

/// Dropping or explicitly stopping a capture releases its native listener.
pub struct CaptureGuard(Option<Box<dyn FnOnce() + Send>>);

impl CaptureGuard {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self(Some(Box::new(stop)))
    }

    pub fn stop(mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.0.take() {
            stop();
        }
    }
}

#[derive(Default)]
pub struct InputReducer {
    pressed: Option<(String, f64, f64, i64)>,
    last_text: Option<(String, String)>,
}

impl InputReducer {
    /// Keep the pre-key value without emitting it. Paste/cut then replay their
    /// resulting value; copy/select-all do not introduce text replacement.
    pub fn baseline_text(&mut self, field: &str, value: &str, safe: bool) {
        self.last_text = safe.then(|| (field.to_owned(), value.to_owned()));
    }
    pub fn pointer_down(&mut self, button: &str, x: f64, y: f64, at_ms: i64) {
        if valid_point(x, y) && matches!(button, "left" | "right" | "middle") {
            self.pressed = Some((button.to_owned(), x, y, at_ms));
        }
        self.last_text = None;
    }

    pub fn pointer_up(&mut self, button: &str, x: f64, y: f64, at_ms: i64) -> Option<RawEvent> {
        let (pressed, from_x, from_y, started) = self.pressed.take()?;
        if pressed != button || !valid_point(x, y) {
            return None;
        }
        let distance = (x - from_x).hypot(y - from_y);
        let (kind, payload) = if distance >= 5.0 {
            (
                "drag",
                json!({"from_x": from_x, "from_y": from_y, "to_x": x, "to_y": y,
                "button": button, "duration_ms": (at_ms - started).clamp(1, 60000)}),
            )
        } else {
            ("click", json!({"x": x, "y": y, "button": button}))
        };
        Some(RawEvent::new("desktop", kind, payload))
    }

    pub fn scroll(&mut self, dx: i64, dy: i64) -> Option<RawEvent> {
        if dx == 0 && dy == 0 {
            return None;
        }
        self.last_text = None;
        Some(RawEvent::new(
            "desktop",
            "scroll",
            json!({"dx": dx, "dy": dy}),
        ))
    }

    /// Keep shortcuts and navigation; never serialize literal password keys.
    pub fn key(&mut self, key: &str, modifiers: &[&str], secure: bool) -> Option<RawEvent> {
        if secure {
            self.last_text = None;
            return None;
        }
        let modifiers: BTreeSet<_> = modifiers.iter().copied().collect();
        let shortcut =
            modifiers.contains("ctrl") || modifiers.contains("cmd") || modifiers.contains("alt");
        let navigation = matches!(
            key,
            "enter"
                | "tab"
                | "esc"
                | "up"
                | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "delete"
                | "backspace"
        );
        if !(shortcut
            || navigation
            || key.starts_with('f') && key[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)))
        {
            return None;
        }
        let mut combo: Vec<_> = modifiers.into_iter().collect();
        combo.push(key);
        self.last_text = None;
        Some(RawEvent::new(
            "desktop",
            "key",
            json!({"keys": combo.join("+")}),
        ))
    }

    /// A complete value, already screened by the platform. Replacing the field
    /// handles selection edits, IME composition and deletion without keylogging.
    pub fn text(
        &mut self,
        field: &str,
        value: &str,
        safe: bool,
        select_all: &str,
    ) -> Option<RawEvent> {
        if !safe || field.is_empty() || value.len() > 1_048_576 {
            self.last_text = None;
            return None;
        }
        let next = (field.to_owned(), value.to_owned());
        if self.last_text.as_ref() == Some(&next) {
            return None;
        }
        self.last_text = Some(next);
        Some(RawEvent::new(
            "desktop",
            "text",
            json!({"field": field, "text": value,
            "replace": true, "select_all": select_all, "safe": true}),
        ))
    }
}

fn valid_point(x: f64, y: f64) -> bool {
    x.is_finite() && y.is_finite()
}

/// A shared reducer lets native event callbacks and text notifications preserve
/// their ordering in one session, without holding app or recorder mutexes.
#[derive(Clone)]
pub struct InputEmitter {
    reducer: Arc<Mutex<InputReducer>>,
    sink: EventSink,
}

impl InputEmitter {
    pub fn new(sink: EventSink) -> Self {
        Self {
            reducer: Arc::new(Mutex::new(InputReducer::default())),
            sink,
        }
    }

    pub fn with(&self, f: impl FnOnce(&mut InputReducer) -> Option<RawEvent>) {
        let mut reducer = self.reducer.lock();
        if let Some(event) = f(&mut reducer) {
            (self.sink)(event);
        }
    }

    pub fn event(&self, kind: &str, payload: Value) {
        (self.sink)(RawEvent::new("desktop", kind, payload));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_drag_and_release_without_press() {
        let mut r = InputReducer::default();
        assert!(r.pointer_up("left", 1.0, 2.0, 1).is_none());
        r.pointer_down("left", 10.0, 20.0, 10);
        assert_eq!(r.pointer_up("left", 11.0, 20.0, 12).unwrap().kind, "click");
        r.pointer_down("left", 10.0, 20.0, 20);
        let drag = r.pointer_up("left", 100.0, 200.0, 520).unwrap();
        assert_eq!(drag.kind, "drag");
        assert_eq!(drag.payload["duration_ms"], 500);
    }

    #[test]
    fn never_records_plain_or_secure_password_keystrokes() {
        let mut r = InputReducer::default();
        assert!(r.key("p", &[], false).is_none());
        assert!(r.key("p", &["shift"], false).is_none());
        assert!(r.key("a", &["cmd"], true).is_none());
        assert!(r.text("password", "secret", false, "ctrl+a").is_none());
        assert_eq!(
            r.key("s", &["ctrl"], false).unwrap().payload["keys"],
            "ctrl+s"
        );
    }

    #[test]
    fn safe_text_is_deduplicated_and_empty_values_preserve_deletion() {
        let mut r = InputReducer::default();
        assert!(r.text("field", "中文", true, "cmd+a").is_some());
        assert!(r.text("field", "中文", true, "cmd+a").is_none());
        assert_eq!(
            r.text("field", "", true, "cmd+a").unwrap().payload["text"],
            ""
        );
        assert!(r.text("", "secret", true, "cmd+a").is_none());
    }

    #[test]
    fn non_editing_shortcuts_do_not_rewrite_text() {
        let mut r = InputReducer::default();
        r.baseline_text("notes", "hello", true);
        assert!(r.text("notes", "hello", true, "ctrl+a").is_none());
        assert_eq!(
            r.text("notes", "hello pasted", true, "ctrl+a")
                .unwrap()
                .payload["text"],
            "hello pasted"
        );
    }

    #[test]
    fn capture_guard_releases_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let n = Arc::new(AtomicUsize::new(0));
        let copy = n.clone();
        CaptureGuard::new(move || {
            copy.fetch_add(1, Ordering::SeqCst);
        })
        .stop();
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }
}
