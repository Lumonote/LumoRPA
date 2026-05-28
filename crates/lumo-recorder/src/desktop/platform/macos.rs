//! macOS desktop recorder back-end.
//!
//! Strategy:
//! - Resolve the foreground app via `NSWorkspace.frontmostApplication` —
//!   exposed by spawning a one-shot `osascript` so we don't need to pull
//!   in objc bridge crates for what is fundamentally a metadata read. The
//!   call is fast (≤ 20 ms on a warm system) and runs at the recorder's
//!   200 ms cadence comfortably.
//! - The window title comes from a second `osascript` line.
//! - For the focused control (the "AccessKit"-shaped fields) we use the
//!   `System Events` AXFocusedUIElement query. Failures here are non-fatal
//!   — apps that don't expose their a11y tree just report blank role /
//!   name / value and the recorder still emits a `focus_changed`.
//!
//! The first time osascript reads the AX tree macOS prompts the user to
//! grant Accessibility permission in System Settings. After that it runs
//! silently. We don't try to detect the prompt here — the OS UI is clearer
//! than anything we could emit.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

use super::Backend;
use crate::desktop::FocusSnapshot;

#[derive(Debug, Default)]
pub struct MacOsBackend;

impl MacOsBackend {
    pub fn new() -> Self {
        Self
    }
}

/// AppleScript that emits a `tab`-separated quintuple of
/// `app\tpid\twindow\trole\tname\tvalue`. Anything we couldn't read
/// falls back to the empty string, so the parser stays trivial.
const FOCUS_SCRIPT: &str = r#"
on safe(handler)
  try
    return handler() as text
  on error
    return ""
  end try
end safe

tell application "System Events"
  set appName to ""
  set appPid to 0
  set winTitle to ""
  set focusRole to ""
  set focusName to ""
  set focusValue to ""
  try
    set frontApp to first application process whose frontmost is true
    set appName to name of frontApp as text
    try
      set appPid to unix id of frontApp as text
    end try
    try
      set winTitle to name of front window of frontApp as text
    end try
    try
      set focused to value of attribute "AXFocusedUIElement" of frontApp
      try
        set focusRole to role of focused as text
      end try
      try
        set focusName to description of focused as text
      end try
      if focusName is "" then
        try
          set focusName to name of focused as text
        end try
      end if
      try
        set focusValue to value of focused as text
      end try
    end try
  end try
  return appName & character id 9 & appPid & character id 9 & winTitle & character id 9 & focusRole & character id 9 & focusName & character id 9 & focusValue
end tell
"#;

#[async_trait]
impl Backend for MacOsBackend {
    fn name(&self) -> &'static str {
        "macos"
    }
    fn is_supported(&self) -> bool {
        true
    }
    async fn poll(&self) -> anyhow::Result<FocusSnapshot> {
        let out = Command::new("osascript")
            .arg("-s")
            .arg("s")
            .arg("-e")
            .arg(FOCUS_SCRIPT)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if !out.status.success() {
            // Permission denied surfaces as nonzero exit + stderr "execution
            // error" — treat that as "no data" rather than poisoning the run.
            let err = String::from_utf8_lossy(&out.stderr);
            tracing::debug!("osascript focus probe: {}", err.trim());
            return Ok(FocusSnapshot::default());
        }
        let line = String::from_utf8_lossy(&out.stdout);
        Ok(parse_focus_line(&line))
    }
}

fn parse_focus_line(s: &str) -> FocusSnapshot {
    let cleaned = s.trim().trim_matches('"');
    let parts: Vec<&str> = cleaned.split('\t').collect();
    let get = |i: usize| parts.get(i).copied().unwrap_or("").trim().to_string();
    let pid = get(1).parse::<i32>().unwrap_or(0);
    FocusSnapshot {
        app: get(0),
        pid,
        window_title: get(2),
        focused_role: get(3),
        focused_name: get(4),
        focused_value: get(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_focus_line_extracts_all_fields() {
        let raw = "Chrome\t512\tGitHub - lumorpa\tAXTextField\tSearch\tinitial value";
        let snap = parse_focus_line(raw);
        assert_eq!(snap.app, "Chrome");
        assert_eq!(snap.pid, 512);
        assert_eq!(snap.window_title, "GitHub - lumorpa");
        assert_eq!(snap.focused_role, "AXTextField");
        assert_eq!(snap.focused_name, "Search");
        assert_eq!(snap.focused_value, "initial value");
    }

    #[test]
    fn parse_focus_line_tolerates_missing_fields() {
        let raw = "Notes\t\t\t\t\t";
        let snap = parse_focus_line(raw);
        assert_eq!(snap.app, "Notes");
        assert_eq!(snap.window_title, "");
        assert_eq!(snap.focused_role, "");
        assert!(snap.focused_value.is_empty());
    }

    #[test]
    fn parse_focus_line_handles_blank_input() {
        let snap = parse_focus_line("");
        assert!(snap.is_empty());
    }

    #[test]
    fn parse_focus_line_strips_outer_quotes_from_osascript() {
        // `osascript -s s` wraps strings containing spaces in quotes — make
        // sure we don't carry those quotes into the snapshot field values.
        let raw = "\"Chrome\t999\t\tAXButton\tLogin\t\"";
        let snap = parse_focus_line(raw);
        assert_eq!(snap.app, "Chrome");
        assert_eq!(snap.pid, 999);
        assert_eq!(snap.focused_role, "AXButton");
        assert_eq!(snap.focused_name, "Login");
    }
}
