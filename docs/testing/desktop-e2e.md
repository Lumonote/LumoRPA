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
upload, clicks, iframe typing/clicking and persisted outputs. Clicks and typed input must
be trusted browser input. A second fixture exercises cross-site, nested
out-of-process frames with Unicode input, scrolling and scaled frame owners. Missing Chrome or a blocked localhost port fails the
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

## Verification recorded on 2026-11-10

Real Chrome passed both workflow cases on macOS, including cross-site nested
out-of-process frames, Unicode typing and trusted input events. Native Linux
recording has a separate Xvfb/AT-SPI acceptance workflow and fixture documented
in [the recorder README](../../crates/lumo-recorder/README.md). Linux Tauri and
Linux native recorder execution still require a Linux runner; no pass is
inferred from the macOS regression suite. Representative local OCR devices can
run [the offline accuracy and memory acceptance tool](local-ocr.md).

## First Linux CI result for `ffd4bc5`

The [real Chrome job](https://github.com/Lumonote/LumoRPA/actions/runs/42750642598/job/129613172325)
passed on Ubuntu. The [Tauri job](https://github.com/Lumonote/LumoRPA/actions/runs/42750642598/job/129613172084)
built the application but failed in the production IPC/webview acceptance step.
The separate [Linux recorder job](https://github.com/Lumonote/LumoRPA/actions/runs/42750642631/job/129613171822)
also built its harness and failed in native capture acceptance. These are failed
native acceptance results, not unavailable tests or passes. Public job annotations
only supplied exit code 1; detailed logs require GitHub sign-in, and the development
host has neither a running Linux environment nor an authenticated GitHub CLI.

Both Python entry points now publish the original exception as a GitHub Actions
error annotation while retaining the traceback and nonzero exit status. Workflow
command characters are escaped so page text cannot create additional annotations.
This makes future native failures actionable from public job summaries; it does
not establish that either outstanding native failure is fixed.
