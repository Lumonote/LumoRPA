//! Linux capture uses XInput2 for pointer/shortcut events and AT-SPI for text.
//! Its interpreter and libraries are system packages, not downloaded code.

use async_trait::async_trait;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::Backend;
use crate::desktop::input::{CaptureGuard, InputEmitter};
use crate::desktop::FocusSnapshot;

#[derive(Debug, Default)]
pub struct LinuxBackend;

#[async_trait]
impl Backend for LinuxBackend {
    fn name(&self) -> &'static str {
        "linux-x11-atspi"
    }
    fn is_supported(&self) -> bool {
        session_supported(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var("DISPLAY").ok().as_deref(),
        )
    }
    async fn poll(&self) -> anyhow::Result<FocusSnapshot> {
        // Focus notifications arrive on the AT-SPI capture stream.
        Ok(FocusSnapshot::default())
    }
    fn start_input(&self, emitter: InputEmitter) -> anyhow::Result<Option<CaptureGuard>> {
        anyhow::ensure!(self.is_supported(), "Desktop recording requires an X11 desktop session; Wayland global recording is not supported.");
        let mut child = Command::new("/usr/bin/python3")
            .args(["-u", "-c", include_str!("linux_capture.py")])
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null())
            .process_group(0).spawn()
            .map_err(|e| anyhow::anyhow!("Cannot start the X11 recorder. Install system python3, python3-gi, gir1.2-atspi-2.0, at-spi2-core and xinput: {e}"))?;
        let pid = child.id();
        let output = child.stdout.take().expect("piped helper stdout");
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let reader = thread::Builder::new()
            .name("lumo-input-linux".into())
            .spawn(move || {
                let mut ready = Some(ready_tx);
                for line in BufReader::new(output).lines() {
                    let value = match line
                        .ok()
                        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
                    {
                        Some(value) => value,
                        None => continue,
                    };
                    let kind = value["kind"].as_str().unwrap_or("");
                    let payload = &value["payload"];
                    if let Some(tx) = ready.take() {
                        if kind == "ready" {
                            let _ = tx.send(Ok(()));
                        } else {
                            let _ = tx.send(Err(payload["message"]
                                .as_str()
                                .unwrap_or("X11 input capture failed to initialize")
                                .to_owned()));
                            break;
                        }
                        continue;
                    }
                    handle_event(&emitter, kind, payload);
                }
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {}
            result => {
                terminate_group(pid);
                let _ = child.wait();
                let _ = reader.join();
                anyhow::bail!(match result {
                    Ok(Err(message)) => message,
                    _ => "X11 recorder did not initialize within 10 seconds; check the desktop accessibility bus and required packages".into(),
                });
            }
        }
        Ok(Some(CaptureGuard::new(move || {
            terminate_group(pid);
            let _ = child.wait();
            let _ = reader.join();
        })))
    }
}

fn session_supported(session: Option<&str>, display: Option<&str>) -> bool {
    !session.is_some_and(|s| s.eq_ignore_ascii_case("wayland"))
        && display.is_some_and(|d| !d.is_empty())
}

fn terminate_group(pid: u32) {
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // The helper owns a freshly created process group. Both the helper and its
    // xinput child terminate together; no global listener survives a session.
    unsafe {
        kill(-(pid as i32), 15);
    }
}

fn handle_event(emitter: &InputEmitter, kind: &str, p: &Value) {
    let now = chrono::Utc::now().timestamp_millis();
    match kind {
        "pointer_down" | "pointer_up" => {
            let (Some(button), Some(x), Some(y)) =
                (p["button"].as_str(), p["x"].as_f64(), p["y"].as_f64())
            else {
                return;
            };
            emitter.with(|r| {
                if kind == "pointer_down" {
                    r.pointer_down(button, x, y, now);
                    None
                } else {
                    r.pointer_up(button, x, y, now)
                }
            });
        }
        "scroll" => {
            emitter.with(|r| r.scroll(p["dx"].as_i64().unwrap_or(0), p["dy"].as_i64().unwrap_or(0)))
        }
        "key" => {
            let key = p["key"].as_str().unwrap_or("");
            let modifiers: Vec<_> = p["modifiers"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            emitter.with(|r| r.key(key, &modifiers, p["secure"].as_bool().unwrap_or(true)));
        }
        "text" => emitter.with(|r| {
            r.text(
                p["field"].as_str().unwrap_or(""),
                p["text"].as_str().unwrap_or(""),
                p["safe"].as_bool().unwrap_or(false),
                "ctrl+a",
            )
        }),
        "focus_field" | "capture_error" => emitter.event(kind, p.clone()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_x11_support_and_wayland_rejection() {
        assert!(session_supported(Some("x11"), Some(":0")));
        assert!(!session_supported(Some("wayland"), Some(":0")));
        assert!(!session_supported(None, None));
        assert!(!session_supported(Some("x11"), Some("")));
    }
}
