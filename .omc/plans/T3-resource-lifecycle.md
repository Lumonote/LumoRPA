# T3 — Per-node Resources + Profiles (full lifecycle)

**Decision (user, 2026-06-05):** Full lifecycle (doc vision) — resources instantiated once,
reused across the whole flow by every stateful action family, torn down at run end.
`profile` is a **resource sub-field** (not a separate top-level concept; avoids collision with
the existing LLM *provider profiles* in `lumo-ai/config.rs`).

Spec source of truth: `docs/02-Architecture-Design.md:187`
```yaml
spec:
  resources:            # 资源声明（启动一次，全流程复用）
    browser:
      kind: chromium.cdp
      profile: stealth-default
```

## What already exists (reuse, don't rebuild)

| Piece | Location | State |
|---|---|---|
| `spec.resources: BTreeMap<String, serde_yaml::Value>` | `lumo-dsl/src/ast.rs:69` | **parsed but DEAD** (no consumers) |
| `RunTeardown` trait + `register_teardown()` + `registry.teardowns()` | `lumo-core` registry; called `vm.rs:297` | ✅ runs on success/fail/cancel (`vm.rs:290-299`); only panic-unwind skips it |
| Browser per-run shared handle: `SESSIONS: OnceCell<HashMap<run_id, Arc<Session>>>`, `ensure_session` (lazy open-once/reuse), `BrowserTeardown` | `browser.rs:62-182` | ✅ **reference implementation** of the whole pattern |
| `StepCtx.vault_names` validated per-ref ("not declared in spec.vault") | `ctx.rs:184`, validate at `ctx.rs:1057` | ✅ exact precedent for resource-ref validation |
| ctx built via `StepCtx::new(..).with_*()` | `vm.rs:270-287` | hook point for `.with_resources(..)` |

**Per-family connection model today (what "reuse" replaces):**
- browser — run-scoped shared session already ✅
- db (`db_ops.rs`) — fresh `Connection` per step *by design* ("no pooling")
- http (`http.rs`) — `reqwest::Client` built per call (client pools internally)
- ftp/s3 (`transfer.rs`) — `ftp_connect_login` per call
- email (`email.rs`) — `AsyncSmtpTransport::relay` / IMAP `TcpStream::connect` per call

## Design decisions (defaulted — confirm or correct)

1. **Reference shape:** add `Step.resource: Option<String>` — a single named resource the step
   binds to. Minimal + extensible (a `resources: {role: name}` map can come later). Validated
   against declared `spec.resources` keys, mirroring vault.
2. **Instantiation:** **lazy-once** — opened on first referencing step, reused after (matches
   browser today). Avoids opening unused connections / failing a run for an unused resource.
   ("启动一次/start once" = opened once & reused, not necessarily eager at t=0.)
3. **Handle storage:** per-kind global keyed by `(run_id, resource_name)` (browser's proven
   model — keeps `!Send` handles like CDP `Page` out of `StepCtx`). A thin central
   `ResourceDecls` (names+decls) lives in `StepCtx` for validation + lookup only.
4. **Capability gating:** unchanged — a resource open that connects (db/ftp/smtp host, network)
   passes the existing capability checks at open time.
5. **Back-compat:** a step with no `resource:` keeps today's per-step behavior exactly.

## Typed DSL (Phase 0 target)

```rust
// lumo-dsl/src/ast.rs
pub struct ResourceDecl {
    pub kind: String,                 // "chromium.cdp" | "sqlite" | "http" | "ftp" | "smtp" | ...
    #[serde(default)]
    pub profile: Option<String>,      // resource sub-field (browser stealth profile, etc.)
    #[serde(flatten)]
    pub config: serde_yaml::Value,    // kind-specific (url, host, headless, ...)
}
// FlowSpec.resources: BTreeMap<String, ResourceDecl>   (was BTreeMap<_, serde_yaml::Value>)
// Step.resource: Option<String>
```

## Phases (each independently compiles + tests; commit per phase)

- **Phase 0 — DSL types + validation** (`lumo-dsl`): ✅ **DONE & GREEN** (2026-06-05) —
  `ResourceDecl{kind, profile, #[flatten] config}` + retyped `spec.resources` + `Step.resource`
  (`ast.rs`); undeclared-ref rejected as a hard `ValidationError::UnknownResource`, recurses into
  nested blocks (`validate.rs`, not lint — it's malformed, not advisory); `kind` validation
  deferred to Phase 1 (needs the factory registry). 4 new tests; lumo-dsl 57 pass; workspace
  `cargo check --all-targets` exit 0. Files: `ast.rs`, `validate.rs`, `tests/dsl_smoke.rs`.
- **Phase 1 — Resource manager + ctx threading** (`lumo-core`): ✅ **DONE & GREEN** (2026-06-05) —
  `ResourceFactory` trait (`kind()` + `async open(decl, run_id, name)`) in new `resource.rs`;
  factory registry on `ActionRegistry` (`register_resource_factory`/`resource_factory`, keyed by
  kind, last-write-wins, beside `teardowns`); `StepCtx` carries a thin `Arc<BTreeMap<_,ResourceDecl>>`
  (`with_resources`, `has_resource`, `resource_decl` [validating, mirrors the `spec.vault` ref check],
  `set_current_resource`/`current_resource`); VM threads `.with_resources(flow.spec.resources.clone())`
  + sets the per-step ref right after `set_current_step` (back-compat: no `resource:` ⇒ `None` ⇒
  unchanged). 3 new ctx tests; **lumo-core 79 pass; `cargo check --workspace --all-targets` exit 0,
  no warnings**. Independent code review verdict: **SHIP** (no blocking/should-fix).
  Files: `resource.rs`(new), `registry.rs`, `ctx.rs`, `vm.rs`, `lib.rs`.
  - **Trait-shape decision (locked):** `open` returns `Result<(), StepError>`, **not** `-> Handle`.
    A single object-safe `dyn ResourceFactory` registry keyed by `kind` can't return a uniform
    typed handle, and design decision #3 keeps `!Send` handles (CDP `Page`) in each family's own
    per-`(run_id,name)` storage — so `open` is an idempotent *ensure-open* lifecycle call and the
    family exposes its own typed getter to *use* the handle. Teardown stays on the existing
    `RunTeardown` hook (one per family), **not** on `ResourceFactory`. Escape hatch if ever needed:
    widen to `Result<SomeMeta, StepError>` with a `Send` meta type (additive, documented in the
    trait). Review confirmed this fits Phase 2 (browser `ensure_session`→keyed-by-name) and Phase 3
    (db/http/ftp/email).
  - **Commit-hygiene note:** `vm.rs` working tree also carries unrelated pre-existing P1-1/P1-4 WIP;
    when commits resume, stage only T3's two `vm.rs` lines (`git add -p`) to keep the phase focused.
- **Phase 2 — Generalize browser onto the manager** (`lumo-actions/browser.rs`): ✅ **DONE & GREEN**
  (2026-06-05) — sessions re-keyed `run_id` → **`(run_id, slot)`** (nested map); slot = bound
  `chromium.cdp` resource name, else run-default `""` (unbound flows byte-for-byte unchanged,
  `run_id` stays fallback). `LaunchOpts`/`launch_opts_from_decl` (headless from decl config; profile
  plumbed + debug-logged, effect deferred to Phase 4), `browser_slot`, `resolve_open_target`
  (decl opts win over step `with:` when bound), `session_for_ctx` (all 19 consumer sites swapped),
  `BrowserFactory: ResourceFactory` registered. Teardown: `close_run_sessions` reaps all slots
  (end-of-run); `close_slot` reaps one (`browser.close`). Concurrent-open race now reaps the loser
  under a re-check (honors `open` idempotency; Send-safe — decide under lock, await after). Pure
  slot bookkeeping (`put_slot`/`take_slot`/`take_run`) extracted as a generic seam + unit-tested.
  6 new unit tests; **browser suite 70 pass / 0 fail** (existing not-launched/cap-gate/teardown
  contracts intact; real-Chrome tests stay `#[ignore]`); `cargo check --workspace --all-targets`
  exit 0, no warnings. Independent review: **SHIP-WITH-NITS** → all nits applied (race reap, close
  doc, slot-aware error, bookkeeping tests). Added `serde_yaml` dev-dep (T3 tests).
- **Phase 3 — Wire remaining families** (db, http, ftp, email): ✅ **DONE & GREEN** (2026-06-05) —
  extracted a generic **`ResourceStore<H>`** (`resource_store.rs`, new) that owns the
  per-`(run_id, slot)` nested-map bookkeeping (`get`/`get_or_put`/`take_slot`/`take_run`/`has_run`,
  loser-drop on concurrent open) so each family only writes its open + teardown. All four wired,
  each with a `<Kind>Slot`/`ensure_*`/`<Kind>Teardown`(`RunTeardown`)/`<Kind>Factory`(`ResourceFactory`);
  **unbound steps keep their exact per-call behavior** (back-compat):
  - **db** (`db_ops.rs`): shared `Arc<Mutex<Connection>>`, slot = bound `sqlite` resource name, path
    from decl `path:`; `db` field made `Option` (omitted when bound). Bound query keeps read-only via
    `PRAGMA query_only` under the lock (a query can't write the shared RW handle — preserves the
    fs.read/fs.write boundary the unbound read-only open enforced). 3 unit + **9 integ** (7 back-compat
    intact + 2 T3: temp-table reuse across 3 steps, read-only enforcement).
  - **http** (`http.rs`): shared `reqwest::Client` (pool/keep-alive persists), decl `timeout_ms` wins
    when bound; per-step `timeout_ms` for unbound. 3 unit + 1 reuse unit (lazy build ⇒ no server);
    7 existing integ pass.
  - **smtp** (`email.rs`, `email.send` only): shared `AsyncSmtpTransport`; **credentials stay
    vault-resolved** — the transport is built from the FIRST bound send's inputs (NOT the decl, whose
    config is static YAML), reused after. 2 unit + 1 reuse unit. (`email.fetch`/IMAP untouched.)
  - **ftp** (`transfer.rs`): shared `Arc<tokio::Mutex<AsyncFtpStream>>` (async guard held across the
    async FTP ops; also serializes parallel-branch use); first bound step's creds establish it; bound
    steps skip QUIT — **async teardown** QUITs every session. `download_via` extracted (shared by both
    paths). 2 unit. (s3.* untouched.)
  - **Design decisions (documented in code):** (1) config-from-decl for non-secret kinds (db `path`,
    http `timeout_ms`) vs config-from-first-bound-step for secret-bearing kinds (smtp/ftp creds) — the
    decl is never template-resolved, so secrets must stay in vault-resolved step inputs. (2) The
    `ResourceFactory::open` for http/smtp/ftp is **validate-only** (returns Ok without opening): its
    needed runtime context (network grants / credentials) isn't in the decl `open` receives, and the
    VM doesn't drive `open` yet (zero callers) — the action opens lazily. db/browser `open` fully open
    (config is all in the decl). Per-step capability gating unchanged on every step (bound or not).
  - **Tests/verify:** **61 lumo-actions lib unit tests** + db(9)/http(7) integ all pass; smtp/ftp
    live-server reuse stays `#[ignore]`-class (the reuse *mechanism* is proven by the `ResourceStore`
    tests + the db end-to-end + http/smtp lazy-reuse unit tests). `cargo check --workspace
    --all-targets` exit 0, **no warnings**. Added a `run_bound` helper to `tests/common/mod.rs`.
  - **Review:** independent verdict **SHIP-WITH-NITS** (0 blocking; every invariant — back-compat
    byte-equivalence, per-step capability gating, the bound-query `query_only` hole, Send/Sync +
    async-guard placement, lifecycle/idempotency — explicitly verified). Nits applied: http `open`
    made a true no-op (was misleadingly "validating" a never-failing timeout parse);
    `ResourceStore::take_slot` doc'd as reserved (no Phase-3 consumer — only `take_run` is used);
    ftp concurrent-open loser-not-QUITed tradeoff flagged in code → **Phase 5**. Re-verified green
    (61 lib unit + db(9)/http(7) integ; `cargo check --workspace --all-targets` clean, no warnings).
    Ready to commit when commits resume (stage T3 files separately from the branch's pre-existing WIP).
- **Phase 4 — profile semantics + docs + e2e**: ✅ **DONE & GREEN** (2026-06-05) — browser
  `profile: <name>` now makes the session **persistent + stealthy** (user's chosen scope
  "Persistence + stealth baseline"). `build_browser_config(opts)` (new, replaces the old
  inline `if headless {…} else {…}.build()`): with a profile it sets a stable per-name
  `--user-data-dir` at `$LUMO_HOME/browser-profiles/<sanitized-name>` (cookies/logins survive
  across runs) + a minimal stealth baseline (`--disable-blink-features=AutomationControlled`
  to drop the `navigator.webdriver` signal chromiumoxide's default `--enable-automation` raises,
  + `--no-default-browser-check`); **no profile ⇒ byte-for-byte the pre-T3 ephemeral launch**
  (back-compat verified). `browser_profiles_root()` mirrors `selector_stats::default_path`'s
  `$LUMO_HOME → $HOME/$USERPROFILE+.lumorpa → relative .lumorpa` chain (std-only, **no `dirs`
  dep added**). `profile_user_data_dir(name)` sanitizes to ONE safe component (alnum/`-`/`_`,
  else `_`; empty → `default`) — a crafted name (`../../etc`) can't escape the root. Launch
  error gains a profile-lock hint (Chrome locks the user-data-dir ⇒ one run per profile at a
  time). Other kinds' `profile` documented as reserved/no-op. **Capability-gate review:** the
  profile dir is app-managed infra under `$LUMO_HOME` (like `lumo.db`/`selector-stats.json`,
  and Chrome's own default temp profile) ⇒ NOT `fs.write`-gated; per-step gating otherwise
  unchanged. Docs: new `## Resources` section in `docs/05-LumoFlow-Instruction-Set.md` (decl
  shape, `resource:` binding, kinds table, profile semantics, security: config never templated
  / secrets from first bound step) + a `resource` step-field row. New `examples/order-export.lumoflow.yaml`
  (reuses a persistent browser + a shared sqlite connection; **`validate` OK, steps=7**). 2 new
  unit tests (sanitizer escape-proof; profile→user_data_dir wiring; `args` is private so only
  the pub `user_data_dir` is asserted — stealth-arg application covered by review vs chromiumoxide
  source). **lumo-actions lib 63 pass / 0 fail; `cargo check --workspace --all-targets` exit 0,
  no warnings.** Independent review verdict: **SHIP** (0 blocking / 0 should-fix; 2 cosmetic nits
  left as-is — `browser_profiles_root` style vs `default_path` [behaviorally identical], redundant
  `headless: true` in the example [kept as self-doc]). Files: `browser.rs`, `docs/05-…md`,
  `examples/order-export.lumoflow.yaml`. Ready to commit when commits resume (stage T3 files
  separately from the branch's pre-existing WIP).
- **Phase 5 — lifecycle hardening**: ✅ **DONE & GREEN** (2026-06-05) — one concrete fix +
  the rest verified safe-by-construction (no speculative machinery added for leaks that can't
  happen).
  - **ftp concurrent-open loser graceful-QUIT (Phase 3 review S2) — FIXED.** New
    `ResourceStore::get_or_put_reclaiming_loser(run,slot,handle) -> (in_force, Option<loser>)`
    (one lock, `Entry` match: Occupied ⇒ keep incumbent + hand our handle back as loser; Vacant ⇒
    we win, no loser). `get_or_put` now delegates to it and drops the loser (`.0`), byte-for-byte
    preserving the old "loser silently dropped" semantics for sqlite/http/smtp (their `Drop` *is*
    the close). `ensure_ftp` uses the reclaiming variant: on a lost race it `quit().await`s the
    reclaimed session (sole owner ⇒ lock is immediate; tokio mutex ⇒ guard-across-await is legal),
    instead of a bare drop that skips the FTP goodbye and leaves the server to idle-out the
    authenticated control connection.
  - **Idempotent teardown across all kinds — VERIFIED.** Every family teardown is `take_run`-based:
    the first call drains the run, a second (double-teardown / cancel-before-open) gets an empty
    `Vec` ⇒ zero-iteration no-op, no panic. Locked in by the `ResourceStore` `take_run` tests + a new
    `close_run_ftp` empty-run idempotency test (the async-QUIT teardown, the one most worth proving).
  - **Parallel-branch concurrent *use* serialization — VERIFIED (no code needed).** Every shared
    handle is already either `Mutex`-wrapped (browser `tokio::Mutex<Browser>`+`Mutex<Page>`, db
    `parking_lot::Mutex<Connection>`, ftp `tokio::Mutex<AsyncFtpStream>`) or `Clone`+pooled and
    concurrency-safe by design (http `reqwest::Client`, smtp `AsyncSmtpTransport`). Branches first
    *converge to one handle* — proven by a new multi-threaded race test (64 tasks hammer one slot →
    single-winner) — then serialize *use* through that handle's own mutex.
  - **Resume reopens cleanly — VERIFIED (no code needed).** `run_id` is a fresh `Ulid` every run
    (`vm.rs:237`); resume is a `ResumeMemo` replay, never run_id reuse. A memoized step short-circuits
    with `Ok(())` (`vm.rs:520`, gated on input_hash in `ctx.rs`) *before* `execute`, so it does NOT
    re-open the resource; the first non-replayed bound step opens fresh under the new run_id. Teardown
    runs on the pause path too (pause is an `Err` that still reaches the teardown loop), so a
    paused→resumed flow reaps the prior run and reopens cleanly. (Bonus: with a persistent browser
    `profile`, login state still survives the close/reopen — Phase 4 synergy.)
  - **cancel-mid-open — VERIFIED safe for in-store handles (no code needed).** Teardown is sequenced
    strictly after `run_block_inline(...).await` (`vm.rs:291`→`298-300`, no early return for any
    outcome); `run_parallel` awaits branches inline via `join_all` (`vm.rs:1300`, no detached spawn);
    the dispatch `select!` is `biased;` cancel-first (`vm.rs:638-646`) and drops the in-flight
    `execute` future — where `ensure_*`/`get_or_put*` runs — *before* its `put`. So any `put` happens
    strictly before `run_block_inline` returns, hence strictly before `take_run` ⇒ **an in-store
    handle can never escape teardown across a cancel.**
  - **Residual gap (documented, accepted):** the only thing cancel-mid-open can leak is an *external*
    Chrome process spawned by chromiumoxide before an `ensure_session` launch future is dropped — it
    never reaches the store, so `take_run` can't reap it. Inherent to dropping a not-cancel-safe launch
    future (chromiumoxide doesn't expose the child until `launch` returns); no cheap in-crate fix. Sits
    alongside the pre-existing **panic-unwind** teardown gap (the `vm.rs:298` loop is skipped on a panic
    unwind, not a `Drop`/`catch_unwind`). Both are narrow and noted here, not blockers.
  - **Tests/verify:** +3 (`get_or_put_reclaiming_loser` unit; multi-threaded single-winner convergence;
    `close_run_ftp` empty-run idempotency). **lumo-actions lib 66 pass / 0 fail; `cargo check --workspace
    --all-targets` exit 0, no warnings.** Independent review verdict: **SHIP** — all five lifecycle
    properties confirmed against the code; 3 acceptable nits (reserved `take_slot` still dead [as
    documented]; convergence test doesn't assert winner stability [not the contract]; ftp loser pays a
    connect+login before learning it lost [inherent to lazy-open, narrow window]). Files: `resource_store.rs`,
    `transfer.rs`. Ready to commit when commits resume (stage T3 files separately from pre-existing WIP).

## Status: T3 COMPLETE

Phases 0–5 all ✅ DONE & GREEN (2026-06-05), each independently compiling + tested + independently
reviewed (SHIP / SHIP-WITH-NITS, all nits resolved or documented). Resources are declared once,
opened once per run (lazily on first binding), reused across every bound step, and torn down at run
end across all kinds (browser/sqlite/http/smtp/ftp); `profile` gives the browser a persistent +
stealthy session. **Commits are ON HOLD** (per user) — when they resume, stage the T3 files
separately from the branch's pre-existing WIP via `git add -p`. No code work remains for T3.

## Risks / watch-items
- `serde(flatten)` + `deny_unknown_fields` interaction on `ResourceDecl` (flatten relaxes it) —
  verify lint catches typos instead.
- Parallel branches sharing one resource handle must serialize (browser uses `Mutex<Page>` —
  reuse that discipline). ✅ **RESOLVED (Phase 5):** every shared handle is `Mutex`-wrapped or
  `Clone`+pooled; branches converge to one handle (multi-threaded race test) then serialize on it.
- DB "no long-lived connection" was deliberate for *concurrent flows* — reuse is per-run-scoped,
  so concurrent flows still get distinct handles (keyed by run_id). Preserve that. ✅ Preserved
  (the store is keyed by `(run_id, slot)`; distinct runs never share a handle).
- Resume (F-13): a resumed run has a new `run_id`? confirm — if so resources reopen cleanly.
  ✅ **CONFIRMED (Phase 5):** `run_id` is a fresh `Ulid` every run; resume is `ResumeMemo` replay
  (memoized steps short-circuit before `execute`), so resources reopen lazily under the new run_id.

## Verify (per phase)
`cargo check --workspace --all-targets` + the phase's tests. **Read the log / `test result:` lines,
never trust a wrapper exit code** (see memory: verify-scripts-must-propagate-failure). No
`declare -A` in verify scripts (macOS bash 3.2).
