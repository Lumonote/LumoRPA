# Desktop acceptance tests

The `desktop acceptance` workflow exercises real Chrome and the Tauri webview
on pushes and pull requests to `main` and `dev`.

## Customer acquisition

From the repository root, run:

```sh
cargo test -p lumorpa-desktop --locked --lib acquisition_commands::workflow_tests
cargo test -p lumo-storage --locked --test acquisition
cargo test -p lumorpa-desktop --locked --lib acquisition_chromium_ -- --ignored
```

The worker tests use the production approval, quota, evidence validation and
SQLite code. An in-process model and platform fixture supply posts and replies
for Xiaohongshu, Douyin and Weibo. The cases verify separate identities,
draft allocation on repeated runs, single first contact, ambiguous receipts,
partial platform failure, repeated reply sync and campaign-specific needs after
reopening the database. A model failure leaves the raw reply and the last
committed batch intact so a manual retry can resume after reopening SQLite.

The last command requires Chrome/Chromium. Requests are intercepted locally;
the test checks each platform's page adapter, recipient identity, scoped
gestures and outbound receipt. A second Chrome case loads the production
workbench with fixture IPC responses to check lead filtering, demand-to-profile
navigation, and follow-up queues intersected with demand-evidence searches. It
switches between customers, overlaps a detail read with a refresh, fails a draft
write, and edits the next reply while the prior save is pending. It also checks
that unresolved receipts block sibling draft review, that reconciliation removes
the customer from that queue, and that offline analysis can be retried. No credentials,
real recipients or model keys are used. Changes to live platform pages and OS-level control permissions
require separate validation.

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
- Add a canvas node through the action palette and undo it; drag a node and
  undo the layout; connect a result port, confirm replacement, rename its source
  and check the migrated binding; copy/paste/undo a node, execute and inspect the
  selected node's result through production IPC.
- Create a future cron job, check timezone/inputs/retry settings, edit it, reject
  a stale edit and cancel the fixture before it can run.
- Create a three-platform acquisition campaign, review its scope and template,
  approve and run outreach against an empty database, inspect the execution
  log, then edit its audience and verify authorization is revoked.
- Approve an L3 local plan with the confirmation card and native second prompt.
- Follow new Agent runs, pin historical results and return to the latest run.
- Submit a saved flow's exact alias through the companion's text input, observe
  completion and result without microphone capture, then open that exact run in
  the main window's Mission Control. Observe a slow local task running, cancel
  it from the companion, and submit another task successfully afterward.

The local supplied Agent plan removes the remote planner dependency while
retaining production validation, execution, approval, event transport and
storage. Read-only assertions use the application's real Tauri commands. No
mock Tauri bridge, fake DOM or test-only backend command is injected.
The companion case uses the production local alias router and does not inject
a supplied plan. Canvas dragging uses W3C pointer actions, and copy/paste uses
keyboard actions. These suites are also required by the same-commit
[release delivery gate](release-delivery.md).

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
