# Instruction Set Consistency and Desktop P2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the open instruction-set consistency items and desktop P2 wiring in the existing uncommitted workspace without committing code.

**Architecture:** Add small shared action helpers for stable error kinds, deadlines, bounded collection outputs, and destructive-action dry runs. Reuse resource factories for long-lived IMAP/XLSX handles, keep new action implementations beside their existing families, and split the Tauri host by responsibility only after behavior is covered. Preserve existing defaults unless the report explicitly calls for a new safety bound.

**Tech Stack:** Rust 2021, Tokio, Tauri 2, serde/schemars, async-imap, zip 2.2, umya-spreadsheet, chromiumoxide, vanilla JavaScript.

---

### Task 1: Cross-cutting contracts

**Files:**
- Modify: `crates/lumo-core/src/error.rs`
- Modify: `crates/lumo-dsl/src/validate.rs`
- Create: `crates/lumo-actions/src/contracts.rs`
- Create: `crates/lumo-actions/tests/action_contracts.rs`

- [ ] Add failing tests proving `io`, `network`, and action-internal `timeout` keep stable `ErrorKind` values accepted by `retry.on`.
- [ ] Add failing table-driven tests proving bounded collection actions return `{items|rows, count, truncated}` and reject `limit = 0`.
- [ ] Add failing tests proving destructive actions return a preview and perform no mutation when `dry_run: true`.
- [ ] Implement typed constructors and shared timeout/bounded-output/dry-run helpers; keep existing human-readable messages.
- [ ] Run `cargo test -p lumo-core -p lumo-dsl -p lumo-actions --test action_contracts` and retain the red/green evidence.

### Task 2: Apply contracts to existing action families

**Files:**
- Modify: `crates/lumo-actions/src/file.rs`
- Modify: `crates/lumo-actions/src/http.rs`
- Modify: `crates/lumo-actions/src/db_ops.rs`
- Modify: `crates/lumo-actions/src/email.rs`
- Modify: `crates/lumo-actions/src/excel.rs`
- Modify: `crates/lumo-actions/src/pdf.rs`
- Modify: `crates/lumo-actions/src/docx.rs`
- Modify: `crates/lumo-actions/src/system_ops.rs`
- Modify: `crates/lumo-actions/src/browser.rs`

- [ ] Convert representative I/O and transport failures to typed errors without changing capability-denial behavior.
- [ ] Add `limit` defaults and `truncated` results to `file.list`, `excel.read_rows`, and `browser.extract_table` while preserving legacy fields.
- [ ] Add `timeout_ms` to email actions and blocking Excel/PDF/DOCX operations, using `StepError::Timeout` on expiry.
- [ ] Add `dry_run` to `file.delete`, DB execute/batch mutations, `system.process_kill`, and `email.send`; return an operation preview.
- [ ] Run each affected action test binary separately and fix regressions before proceeding.

### Task 3: Stateful IMAP and XLSX resources

**Files:**
- Modify: `crates/lumo-actions/src/email.rs`
- Modify: `crates/lumo-actions/src/excel.rs`
- Modify: `crates/lumo-actions/src/lib.rs`
- Modify: `crates/lumo-dsl/src/validate.rs`
- Modify: `crates/lumo-dsl/src/caps.rs`
- Test: `crates/lumo-actions/tests/email.rs`
- Test: `crates/lumo-actions/tests/excel_ops.rs`

- [ ] Add failing tests for `imap` and `xlsx` resource declarations and repeated action reuse within one run.
- [ ] Implement per-run resource stores and idempotent factories; teardown must logout/flush and remove handles.
- [ ] Allow legacy inline connection/path inputs when no resource is bound.
- [ ] Run email and Excel tests, including ignored remote tests only when their environment variables exist.

### Task 4: Missing first-class actions

**Files:**
- Modify: `crates/lumo-actions/src/excel.rs`
- Modify: `crates/lumo-actions/src/hash_ops.rs`
- Modify: `crates/lumo-actions/src/archive.rs`
- Modify: `crates/lumo-actions/src/desktop.rs`
- Modify: `crates/lumo-actions/src/browser.rs`
- Modify: `crates/lumo-actions/src/lib.rs`
- Modify: `crates/lumo-dsl/src/caps.rs`
- Test: corresponding files under `crates/lumo-actions/tests/`

- [ ] Add failing tests for add/delete/rename sheet and insert/delete row/column actions.
- [ ] Add failing tests for hash actions accepting exactly one of `text` or `path`.
- [ ] Add password-protected ZIP round-trip tests, enabling only the minimal pure-Rust zip crypto feature required by the pinned crate.
- [ ] Add window close/minimize/maximize and browser back/forward/reload actions with feature-gated platform behavior.
- [ ] Add `desktop.drag` with duration and button validation.
- [ ] Register every new action and update the capability single source of truth; synchronization tests must catch omissions.

### Task 5: Bounded control-flow concurrency

**Files:**
- Modify: `crates/lumo-actions/src/control.rs`
- Modify: `crates/lumo-core/src/vm.rs`
- Test: `crates/lumo-actions/tests/control_ops.rs`
- Test: `crates/lumo-core/tests/control_flow.rs`

- [ ] Add failing tests for `control.parallel.max_concurrency` and `control.for_each.parallel/max_concurrency`.
- [ ] Implement semaphore/buffered scheduling with deterministic result ordering, cancellation propagation, and the existing break/continue boundary semantics.
- [ ] Verify sequential defaults remain unchanged.

### Task 6: Desktop event consumers and host decomposition

**Files:**
- Modify: `apps/desktop/frontend/src/js/main.js`
- Modify: `apps/desktop/frontend/src/js/runs.js`
- Modify: `apps/desktop/frontend/src/js/dom.js`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Create: focused modules under `apps/desktop/src-tauri/src/`
- Test: frontend tests and desktop Rust unit tests

- [ ] Add failing frontend tests for `lumo://toast` and run progress event handling.
- [ ] Emit stable step progress payloads from `execute_flow` and consume them without duplicating persisted run refreshes.
- [ ] Display dropped-log counts when the capped ring reports loss.
- [ ] Move state/prompter, execution, registry cache, recorder, and command groups out of `lib.rs`; keep `run()` and `generate_handler!` as the composition root.
- [ ] Run desktop frontend tests/lint and desktop Rust tests after the final tree is assembled.

### Task 7: Documentation and full verification

**Files:**
- Modify: `docs/05-LumoFlow-Instruction-Set.md`
- Modify: `README.md` where aliases or environment gates are documented
- Modify: `crates/lumo-cli/tests/instruction_set_docs.rs`

- [ ] Document typed errors, bounds/truncation, timeouts, dry runs, resource kinds, aliases (`data.json_*` ↔ `json.*`), and transfer verb cross-references.
- [ ] Extend documentation/registry consistency tests for all new actions and fields.
- [ ] Run core/action/storage/skill packages together, then `lumo-cli` and `lumorpa-desktop` in separate Cargo invocations to avoid feature unification.
- [ ] Run frontend tests and ESLint.
- [ ] Run workspace clippy with `-D warnings`; if disk pressure recurs, report it separately from code failures.
- [ ] Confirm `git status` contains no commits and preserve all user changes.
