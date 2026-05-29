//! P1-2: browser sessions must be reclaimed at end-of-run. A flow that fails
//! (or forgets `browser.close`) must not leak a headless Chrome process. The
//! VM fires a registered teardown hook (`BrowserTeardown`) that force-closes
//! and reaps any session keyed by the finished run's id.

use lumo_actions::{browser, register_all};
use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_dsl::parse_str;

/// CI-safe: tearing down a run that never launched a browser is a harmless
/// no-op and must not panic. Runs without Chrome.
#[tokio::test]
async fn close_run_sessions_unknown_run_is_noop() {
    let run = "never-launched-run-xyz";
    assert!(!browser::session_exists(run));
    browser::close_run_sessions(run).await;
    assert!(!browser::session_exists(run));
}

/// End-to-end proof that the VM's teardown hook reaps a launched browser at
/// run end. Requires a real Chrome, so it is `#[ignore]`d by default and run
/// locally with `cargo test -p lumo-actions --test browser_teardown -- --ignored`.
#[tokio::test]
#[ignore = "launches a real headless Chrome; run with --ignored"]
async fn vm_teardown_reaps_browser_on_run_end() {
    let mut reg = ActionRegistry::new();
    register_all(&mut reg);
    let vm = FlowVm::new(reg, None);

    let flow = parse_str(
        r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: t }
spec:
  steps:
    - { id: launch, action: browser.launch, with: { headless: true } }
"#,
    )
    .expect("parse");

    let report = vm.run(&flow, RunOptions::default()).await.expect("run");
    assert!(report.success);

    // The teardown hook must have removed (and reaped) the session by the time
    // run() returns — without it the headless Chrome would orphan.
    assert!(
        !browser::session_exists(&report.run_id),
        "browser session for run {} should have been reclaimed at end-of-run",
        report.run_id
    );
}
