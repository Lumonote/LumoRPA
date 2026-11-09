# Reliability and workflow completion

Implementation order requested by the user on 2026-11-09. Work in the current
`dev` checkout; do not create Git worktrees or commit/push without a new request.

1. [x] Persist background-job node checkpoints and outputs; recover completed work
   without repeating side effects. Mark uncertain operations for human review.
2. [x] Give scheduled jobs a durable approval lifecycle and job/run association;
   approval waiting must not consume failure retries.
3. [x] Persist editor drafts, restore after restart, and protect normal exit.
4. [x] Add an explicit follow-latest mode and refresh Mission Control history.
5. [x] Exercise editing, saving, execution, approval and results through a real
   browser/desktop boundary; add runnable CI smoke coverage.
6. [x] Share MCP failure accounting, rate limits and circuit breaking across
   Agent and manual calls, including connection failures and timeouts.
7. [x] Expose scheduled-job creation/editing from desktop flows with inputs,
   timezones, retry settings and linked run results.
8. [x] Distinguish OCR download completeness, dependencies and inference readiness;
   connect document OCR to the supported local inference backend.

For each item, first add a regression or acceptance check that demonstrates the
gap, then implement and run the affected tests. Finish with frontend tests/lint,
affected Rust suites, formatting and workspace Clippy. Record material limits
and verification results here as work completes.


Implementation completed on 2026-11-10. No commit or push was performed for this batch.

Verification and delivered behavior:

- Stages 1–2: durable dispatch/result checkpoints, stable run association,
  conservative unknown-side-effect recovery, v6 migration, approval CAS and
  restart-safe partial/strengthened approval. Completed outputs are restored;
  approval waits return leases and do not consume retry attempts. Late terminal
  events cannot reopen a cancelled run. Background controls bind the current
  run so stale history controls cannot affect a later cron occurrence.
- Stage 3: SQLite draft storage, invalid-YAML recovery, source mutation capture,
  serialized saves, changed-disk conflict choices and an explicit exit/restart
  handshake. A failed draft listing cannot delete unseen recovery data.
- Stage 4: separate live/history projections, follow-latest selection, fresh
  history and asynchronous selection/sequence regression coverage.
- Stage 5: real Chromium workflow acceptance passed. The Python W3C WebDriver
  script and Linux Tauri CI cover editor save/autosave, execution/error results,
  history, L3 approval, scheduling/edit conflicts and live/history switching.
  Python syntax, workflow YAML and runner checks passed. The Linux WebKit UI
  suite has not run on this macOS host; its first Linux CI execution is pending.
- Stage 6: Agent/manual/discovery/health-test MCP calls share rate limits,
  circuit state and outcome accounting. RAII covers connection errors, timeouts,
  cancellation and dropped futures; rate rejection cannot strand a half-open
  probe.
- Stage 7: editor scheduling, validated/frozen flow inputs, pinned capability
  versions, timezone/misfire/retry/timeout/priority controls, safe idle editing,
  CAS conflict handling and previous-occurrence result links. DOM regressions
  cover double-click, save/switch/new-edit races, busy forms and millisecond/DST
  preservation.
- Stage 8: model file/shard validation, staged publication, dependency and local
  inference probes, three independent UI states and offline document OCR.
  Explicit desktop home is shared by download and execution. Local output is
  marked ocr_local. AI 38, document 39, OCR routing 18, local OCR integration 3
  and existing remote OCR integration 7 tests passed without real model weights.

User instructions are in docs/testing/background-jobs.md and
local-ocr.md. Desktop acceptance commands and environment limitations are in
docs/testing/desktop-e2e.md.

Material verification limits:

- No GB-scale OCR model was downloaded. Actual device memory requirements,
  model loading and recognition accuracy require the explicit inference probe
  and representative documents on the target device.
- Source fingerprints bind the invoked main flow/skill file. Dynamic subflows
  and external resources do not yet have a complete dependency graph snapshot.
- Restored budgets count recorded invocation attempts and successful returned
  usage. An external call that crashes before returning may have unobservable
  usage. Parallel recorded durations are summed conservatively; offline and
  durable approval waiting are excluded.


Final integrated verification:

- Frontend: 216/216 tests passed; ESLint passed.
- Desktop library: 117/117 tests passed after the final scheduling/result-link
  and background-control changes. A later compatibility-only regression is
  covered by the Agent service test suite (25/25 passed).
- Storage/Agent/Skills: 183 tests passed across the full affected package suites;
  the subsequent real-VM-usage regression group passed all 14 checks.
- Core library: 50 tests passed. AI/document libraries: 77 tests passed.
- OCR routing: 18 tests passed; local document OCR integration: 3 passed;
  existing remote OCR integration: 7 passed.
- Real headless Chrome workflow: 1 acceptance test passed.
- Workspace Clippy with all targets and -D warnings, cargo fmt --check and
  git diff --check passed before final documentation-only edits.

Flow/Skill invocation results now carry the successful VM run's recorded token
and cost rollup, with the underlying flow run ID. Without a repository, or if a
provider does not report usage / best-effort ledger persistence fails, unknown
usage remains zero rather than being estimated. Failed/cancelled invocations
that return no InvocationResult retain the observability limit above.
