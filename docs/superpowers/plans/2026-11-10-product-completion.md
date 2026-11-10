# Product completion and native workflow acceptance

Work in the current `dev` checkout. Never create a Git worktree. Preserve all
existing edits. Do not commit or push this batch without a new user request.

Requested scope, in implementation order:

1. [x] Capture desktop input and convert recordings into executable desktop
   actions; protect sensitive fields and stop native listeners reliably.
2. [x] Edit conditions, bindings, retry and error policies in the node inspector,
   preserving nested paths, other YAML fields, undo and recovery drafts.
3. [x] Bind invoked flows, nested flows, skills and declared file dependencies to
   a durable source snapshot, including dynamic invocation and restart recovery.
4. [x] Account for failed/cancelled attempts, preserve known usage, mark unknown
   usage explicitly and reconcile budgets without double counting.
5. [x] Use trusted native browser input inside cross-origin/nested frames.
6. [x] Implement Linux desktop recording with explicit platform prerequisites.
7. [ ] Verify Linux native Tauri acceptance and representative real OCR models;
   distinguish unavailable environments from passing results.
8. [x] Add a bounded visual observation/action/verification loop through the
   production action runtime and existing permission/cancellation controls.
9. [x] Add SFTP and explicit FTPS with authentication, host verification,
   permission gates, timeouts and transfer acceptance tests.

Validation: meaningful regressions for each behavior, targeted Rust suites and
frontend DOM tests, real Chrome acceptance, frontend lint, Rust formatting and
workspace Clippy. Coordinate Cargo builds because this host currently has about
2.6 GiB free. Record external verification evidence and remaining limitations
honestly; mocks do not establish native platform or model accuracy.

## Implementation and verification recorded on 2026-11-10

Items marked complete describe implemented behavior with regression coverage.
Desktop capture on Windows/macOS still requires interactive platform acceptance;
Linux implementation is accompanied by a native Xvfb/AT-SPI fixture. No native
platform or OCR accuracy claim is inferred from reducer or mock-model tests.

- Frontend: 238 tests passed; ESLint passed. Includes advanced inspector edits,
  nested paths, unknown YAML preservation, undo/draft behavior, usage resolution
  and exact replay permissions when merging recordings.
- Rust core suites: 550 passed, 4 explicitly ignored; desktop host: 119 passed.
  Actions library and browser/schema/transfer regressions: 183 passed,
  17 explicitly ignored platform/service cases. Full workspace Clippy with
  `--all-targets --locked -- -D warnings`, Rust formatting, diff whitespace
  checks and current CLI build passed.
- Final focused suites after boundary fixes: visual Agent 9/9; source snapshot,
  invocation ledger and durable restart 21/21; native cancellation 6/6; desktop
  action/schema gates 32 passed with 5 opt-in native actuation cases ignored.
  Native cancellation tests use injected event sinks and never actuate the
  developer's desktop.
- Real Chrome: 2/2 passed, including nested cross-site renderer frames,
  Unicode keyboard/IME input, scrolling, CSS scaling and `isTrusted` events.
- Real SFTP/FTPS: 2/2 passed against temporary loopback OpenSSH/TLS servers,
  including identity/certificate failures and preserving existing destinations.
  An additional unchanged-suite rerun was not started because automatic
  permission review timed out twice; the earlier live evidence remains valid.
- OCR acceptance runner: 12 contract/metric/process tests passed. A freshly
  built CLI explicitly rejected absent local model files without cloud fallback.
  Neither result establishes recognition accuracy with actual weights.

Reproducible commands and limitations are documented in
[desktop acceptance](../../testing/desktop-e2e.md),
[recording](../../../crates/lumo-recorder/README.md),
[background jobs](../../testing/background-jobs.md),
[visual Agent](../../testing/visual-agent.md),
[secure transfers](../../testing/secure-transfers.md) and
[local OCR acceptance](../../testing/local-ocr.md).

Item 7 remains open for Linux native acceptance. The first Linux CI runs for
`ffd4bc5` built successfully but failed during native recorder/Tauri acceptance;
real Chrome and secure-transfer CI passed. Public annotations exposed only the
exit code, and the authorized Chrome session was also signed out, so the exact
native failures have not been diagnosed. Subsequent Python acceptance failures
now publish escaped, actionable exception annotations. No failed or unavailable
test is counted as a pass.

The real OCR portion now has a verified GLM-OCR baseline: five clean/degraded
synthetic documents passed through the production CLI and official weights,
with whitespace-normalized CER 0, process elapsed 3.73–4.74 s and sampled
process-tree RSS 2.72–2.73 GiB. See the [raw device report and reusable fixtures](../../testing/reports/2026-11-10-glm-ocr/README.md).
This does not establish accuracy for other models, handwriting, complex tables
or private business documents.

## Task assistant follow-up, 2026-11-10

The user selected text/voice instructions, execution progress and results as
the companion's priority. Implemented [the task assistant](../../testing/companion-assistant.md):

- [x] Text submission through the existing Agent planning, permissions and
  approval path, without requiring microphone activation.
- [x] Run-specific progress, result summaries, failures, stop and exact history
  navigation; closing the companion preserves unrelated Agent sessions.
- [x] Open the text assistant from the main toolbar without microphone access;
  selected task details keep receiving approval/progress events even when
  another task becomes the globally latest run.
- [x] Complete revisioned host snapshots, durable event replay on binding,
  race-safe submission/cancellation and terminal-state recovery.
- [x] Stable draft/focus, frame-coalesced audio levels, subscription cleanup,
  hidden/reduced-motion behavior and screen-constrained expanded layout.
- [x] Linux recorder focus/password fail-closed fixes and replayable key filtering.
- [x] Real OCR dependency diagnosis: GLM/Paddle require working torchvision;
  supported local OCR profiles no longer generate an unknown chat-kind warning.

Validation for this follow-up: frontend 264/264 and lint passed; desktop Rust
132/132, AI Rust 40/40, Linux helper 9/9 and CI annotation 3/3 passed. The OCR
runtime tests include missing and broken torchvision and preserve DeepSeek's
existing dependency contract. Formatting, whitespace checks, a current CLI
build and workspace Clippy (`--all-targets --locked -- -D warnings`) passed.
With the user's explicit localhost authorization, browser acceptance of the
actual companion frontend with simulated interfaces passed: text submission,
progress/results, retry/stop, muted text input, simulated voice, run-specific
details and hide/recall. It exposed and fixed Esc focus loss and a clipped send
button. Idle, completed, long-result and failed layouts keep every input/action
control visible at 390×390; results scroll independently without horizontal
overflow. An additional regression protects composing IME Escape. See the
[screenshots and measured layout evidence](../../testing/reports/2026-11-10-companion/README.md).
The fixture does not establish native Tauri IPC, real microphone/STT or actual
end-to-end task execution acceptance.

All changes were developed in the primary `dev` checkout without a worktree.
Commit and push follow the user's explicit request; the outstanding native
platform and real-model acceptance above still requires its target environment.
