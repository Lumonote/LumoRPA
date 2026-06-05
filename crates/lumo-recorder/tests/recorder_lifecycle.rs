//! F-22 recorder lifecycle tests.
//!
//! The CI-safe tests below exercise the parts that don't need a browser:
//! the recorder's mode selection and the no-op teardown of an unstarted
//! recorder. The end-to-end teardown / connect proofs launch (or attach to) a
//! real Chrome, so they are `#[ignore]`d by default and run locally with
//! `cargo test -p lumo-recorder --test recorder_lifecycle -- --ignored`.

use lumo_recorder::{BrowserMode, BrowserRecorder, Recorder};

/// CI-safe: a recorder that never started has no live session, and dropping or
/// stopping it is a harmless no-op (no runtime panic, nothing to reap).
#[tokio::test]
async fn unstarted_recorder_stop_is_noop() {
    let rec = BrowserRecorder::new();
    assert!(!rec.is_running());
    let events = rec.stop().await.expect("stop");
    assert!(events.is_empty());
    assert!(!rec.is_running());
}

/// CI-safe: dropping an unstarted recorder must not panic even off a runtime.
#[test]
fn dropping_unstarted_recorder_off_runtime_is_safe() {
    let rec = BrowserRecorder::connect_to("ws://127.0.0.1:9222/devtools/browser/x");
    assert!(!rec.is_running());
    drop(rec); // exercises Drop's no-runtime branch with no session present
}

/// CI-safe: the explicit constructors select the expected mode.
#[test]
fn constructors_select_mode() {
    // `new()` / `default()` are launch mode (back-compat with the Tauri app).
    let _ = BrowserRecorder::new();
    let _ = BrowserRecorder::default();
    // `connect_to` and `with_mode` build connect mode.
    let _ = BrowserRecorder::connect_to("http://127.0.0.1:9222");
    let _ = BrowserRecorder::with_mode(BrowserMode::Connect {
        endpoint: "http://127.0.0.1:9222".into(),
    });
    let _ = BrowserRecorder::with_mode(BrowserMode::Launch);
}

/// E2E: launch a real Chrome, then `stop()` must reap it (no orphan). Requires
/// a Chromium binary; `#[ignore]`d by default.
#[tokio::test]
#[ignore = "launches a real head-ful Chrome; run with --ignored"]
async fn launch_then_stop_reaps_browser() {
    let rec = BrowserRecorder::new();
    rec.start(None).await.expect("start launches Chrome");
    assert!(rec.is_running());
    let events = rec.stop().await.expect("stop reaps Chrome");
    assert!(!rec.is_running());
    // We at least got the `launched` lifecycle event.
    assert!(events.iter().any(|e| e.kind == "launched"));
}

/// E2E: launch a real Chrome, then `drop` the recorder. The Drop impl must
/// spawn the async teardown (we're inside a tokio runtime here) so the process
/// is closed rather than orphaned. Requires a Chromium binary; `#[ignore]`d.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "launches a real head-ful Chrome; run with --ignored"]
async fn drop_on_runtime_tears_down_launched_browser() {
    let rec = BrowserRecorder::new();
    rec.start(None).await.expect("start");
    assert!(rec.is_running());
    drop(rec);
    // Give the detached teardown task a moment to close/reap.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
}

/// E2E: attach to an already-running Chrome (start it yourself with
/// `chrome --headless --remote-debugging-port=9222`), record, then `stop()`.
/// Stop must only disconnect — the external Chrome keeps running. Set
/// `LUMO_TEST_CHROME_WS` to the DevTools endpoint to run this.
#[tokio::test]
#[ignore = "needs an externally-running Chrome with --remote-debugging-port; run with --ignored"]
async fn connect_to_existing_chrome_does_not_kill_it() {
    let endpoint = std::env::var("LUMO_TEST_CHROME_WS")
        .unwrap_or_else(|_| "http://127.0.0.1:9222".to_string());
    let rec = BrowserRecorder::connect_to(endpoint);
    rec.start(None).await.expect("connect to existing Chrome");
    assert!(rec.is_running());
    let events = rec.stop().await.expect("stop disconnects");
    assert!(!rec.is_running());
    // The `launched` event records that we attached, not spawned.
    let launched = events
        .iter()
        .find(|e| e.kind == "launched")
        .expect("launched event");
    assert_eq!(
        launched.payload.get("connected").and_then(|v| v.as_bool()),
        Some(true)
    );
}
