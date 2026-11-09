# Desktop acceptance tests

Two opt-in suites cross real process boundaries. The `desktop acceptance`
workflow runs both on pushes and pull requests to `main` and `dev`.

## Chrome actions

Install Chrome/Chromium in a standard discovery location, then run from the
repository root:

```sh
CARGO_INCREMENTAL=0 cargo test -p lumo-actions --locked \
  --test browser_workflow_e2e -- --ignored --nocapture
```

The suite serves temporary pages on localhost and runs the production Flow VM
and browser actions. It verifies form typing, select changes, cookies, file
upload, clicks, iframe typing/clicking and persisted outputs. Both clicks must
be trusted browser input. Missing Chrome or a blocked localhost port fails the
test. This suite does not use the `LUMO_SKIP_BROWSER_TESTS` escape hatch.

## Tauri webview

Tauri's native WebDriver is supported on Linux, and is not supported on macOS.
On Ubuntu 24.04, install the build dependencies listed in
`.github/actions/linux-deps/action.yml`, then:

```sh
sudo apt-get install -y webkit2gtk-driver xvfb dbus-x11
cargo install tauri-driver --locked
CARGO_INCREMENTAL=0 cargo build -p lumorpa-desktop --locked
dbus-run-session -- xvfb-run -a python3 scripts/desktop-e2e.py \
  --artifacts /tmp/lumo-desktop-e2e-artifacts
```

The Python client uses the standard library and W3C WebDriver. It creates fresh
XDG data, config and cache directories, leaving the normal application data
untouched. Buttons, text fields, the history selector and the native L3 confirm
dialog are driven through WebDriver. It checks:

- Create, edit and save a user flow; execute it and read the displayed result.
- Edit then run directly, proving the new buffer was saved before execution.
- Open a persisted run in history and inspect the result.
- Execute a failing step and inspect its displayed error and persisted state.
- Create a future cron job, check timezone/inputs/retry settings, edit it, reject
  a stale edit and cancel the fixture before it can run.
- Approve an L3 local plan with the confirmation card and native second prompt.
- Follow new Agent runs, pin historical results and return to the latest run.

The local supplied Agent plan removes the remote planner dependency while
retaining production validation, execution, approval, event transport and
storage. Read-only assertions use the application's real Tauri commands. No
mock Tauri bridge, fake DOM or test-only backend command is injected.

On failure, the script saves `driver.log`, a screenshot and the webview source
under the artifact directory. CI uploads that directory. A platform or driver
that cannot run the suite exits with an error instead of reporting a pass.

## Verification recorded on 2026-11-09

The Chrome workflow was executed successfully on macOS with real headless
Chrome. The Python script passed syntax and CLI checks, and the workflow passed
YAML parsing and the runner-label guard. The full Tauri/WebKit acceptance suite
could not run on that macOS host; its first Linux CI execution is still needed
to establish the desktop end-to-end result.
