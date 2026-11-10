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

Item 7 remains unverified on this macOS host: there is no usable Linux runtime
or logged-in CI access, and no installed real OCR weights/inference dependencies.
The Linux Tauri, Linux native recorder and document OCR acceptance entry points
are ready for the target environment; no skipped or unavailable test is counted
as a pass. OCR device reports must include representative documents, CER,
cold-start runtime and sampled process-tree memory.

All changes were developed in the primary `dev` checkout without a worktree.
Commit and push follow the user's explicit request; the outstanding native
platform and real-model acceptance above still requires its target environment.
