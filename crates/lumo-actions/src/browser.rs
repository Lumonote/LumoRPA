//! Browser automation actions over the Chrome DevTools Protocol via
//! `chromiumoxide`. M1 implements the minimal surface needed to drive a
//! login → click → extract flow; the multi-strategy selector engine
//! (CSS / XPath / A11y / Vision) lands in M2.

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::browser::{
    SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
};
use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::cdp::browser_protocol::page::{
    EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
};
use chromiumoxide::cdp::js_protocol::runtime::{EvaluateParams, ExecutionContextId};
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, ResourceFactory, RunTeardown, StepCtx};
use lumo_dsl::ResourceDecl;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::selectors::{clear_marker, resolve_element, MultiSelector};
use crate::vision::resolve_via_vision;

pub fn register(r: &mut ActionRegistry) {
    r.register(LaunchAction);
    r.register(CloseAction);
    r.register(OpenAction);
    r.register(ClickAction);
    r.register(TypeAction);
    r.register(ExtractAction);
    r.register(WaitAction);
    r.register(InfoAction);
    // F-10 browser action completions.
    r.register(EvalAction);
    r.register(ScreenshotAction);
    r.register(ScrollAction);
    r.register(HoverAction);
    r.register(SelectAction);
    r.register(CookiesAction);
    r.register(SetCookieAction);
    // F-10 part 2: multi-tab + file upload.
    r.register(TabsAction);
    r.register(TabAction);
    r.register(UploadAction);
    // 批次B: download / dialog / frame switch / table extract.
    r.register(DownloadWaitAction);
    r.register(DialogAction);
    r.register(FrameAction);
    r.register(ExtractTableAction);
    // P1-2: reclaim any browser process left open by a run that failed (or
    // forgot `browser.close`) once the VM finishes that run.
    r.register_teardown(Arc::new(BrowserTeardown));
    // T3: open `spec.resources.<name>` browsers (kind `chromium.cdp`) lazily,
    // keyed by the declared name, reusing one session across every step that
    // binds to it.
    r.register_resource_factory(Arc::new(BrowserFactory));
}

// ─── Browser sessions ────────────────────────────────────────────────────────
// Sessions are keyed by (flow run id, *slot*). The slot is the declared resource
// name when the step binds to a `chromium.cdp` resource (T3: `spec.resources.<name>`
// + `Step.resource`), so one run can hold several independent browsers; a step
// that binds nothing uses the run's `DEFAULT_SLOT`. `run_id` thus stays the
// fallback key and flows that never declare a browser resource behave exactly as
// before. Live handles live here, never in `StepCtx` (keeps the `!Send` CDP `Page`
// out of the fork-able context).

struct Session {
    /// The Chrome handle. Behind a tokio mutex because the graceful close/reap
    /// path (`close`/`wait`/`kill`) needs `&mut Browser` and is async — the
    /// guard is held across `.await`, which a sync mutex can't do safely.
    browser: tokio::sync::Mutex<Browser>,
    /// CDP event-pump task. Kept so teardown can `abort()` it instead of
    /// leaving it detached until the socket happens to close.
    handler: Mutex<Option<JoinHandle<()>>>,
    page: Mutex<Option<Page>>,
}

/// Launch options for a browser session — resolved either from a step's own
/// `with:` (unbound step, back-compat) or from a `chromium.cdp` resource
/// declaration (T3). A `profile` makes the session **persistent**: it launches
/// against a stable per-name Chrome user-data-dir (cookies / logins / localStorage
/// survive across runs) and applies a small **stealth baseline** — see
/// [`build_browser_config`]. No profile ⇒ an ephemeral browser with chromiumoxide's
/// default args (byte-for-byte the pre-T3 launch). `headless` always governs
/// head/headless either way.
#[derive(Clone, Debug)]
struct LaunchOpts {
    headless: bool,
    profile: Option<String>,
}

impl Default for LaunchOpts {
    fn default() -> Self {
        Self {
            headless: true,
            profile: None,
        }
    }
}

/// The run's default browser slot — used by any step that does not bind to a
/// declared `chromium.cdp` resource (preserves the old run_id-keyed behavior).
const DEFAULT_SLOT: &str = "";

/// `spec.resources.<name>.kind` selector for a CDP/Chromium browser resource.
pub const BROWSER_KIND: &str = "chromium.cdp";

/// Sessions for one run, keyed by slot. Dropping this bucket tears down all of
/// the run's browsers (see [`close_run_sessions`]).
type RunSessions = HashMap<String, Arc<Session>>;
type SessionMap = Arc<Mutex<HashMap<String, RunSessions>>>;

static SESSIONS: once_cell::sync::OnceCell<SessionMap> = once_cell::sync::OnceCell::new();

fn sessions() -> SessionMap {
    SESSIONS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

// ─── slot bookkeeping (pure, generic over the value so it's unit-testable
// without a real `Session`/Chrome — guards the nested-map invariants that the
// teardown paths rely on) ─────────────────────────────────────────────────────

/// Insert `value` at `(run_id, slot)`, creating the run bucket if absent.
/// Returns any value previously stored at that slot — the caller must reap it
/// (a same-slot overwrite, e.g. the loser of a concurrent open).
fn put_slot<V>(
    map: &mut HashMap<String, HashMap<String, V>>,
    run_id: &str,
    slot: &str,
    value: V,
) -> Option<V> {
    map.entry(run_id.to_string())
        .or_default()
        .insert(slot.to_string(), value)
}

/// Remove and return the value at `(run_id, slot)`, dropping the run bucket once
/// its last slot is gone (so `session_exists`/teardown stay accurate).
fn take_slot<V>(
    map: &mut HashMap<String, HashMap<String, V>>,
    run_id: &str,
    slot: &str,
) -> Option<V> {
    let removed = map.get_mut(run_id).and_then(|run| run.remove(slot));
    if map.get(run_id).is_some_and(|run| run.is_empty()) {
        map.remove(run_id);
    }
    removed
}

/// Remove and return all of a run's slots (end-of-run teardown).
fn take_run<V>(
    map: &mut HashMap<String, HashMap<String, V>>,
    run_id: &str,
) -> Option<HashMap<String, V>> {
    map.remove(run_id)
}

// ─── browser profiles (T3 Phase 4) ──────────────────────────────────────────
// A `profile: <name>` resource sub-field makes the browser persistent: it runs
// against a stable on-disk Chrome user-data-dir keyed by the name, so cookies /
// logins / localStorage carry across runs. The dir lives under `$LUMO_HOME`,
// alongside the rest of the app's state (selector-stats.json, lumo.db, …).

/// The persistent-browser-profile root, `$LUMO_HOME/browser-profiles`. Mirrors the
/// app's state-rooting convention (cf. `selector_stats::default_path`): prefer
/// `$LUMO_HOME`, else `$HOME`/`$USERPROFILE` + `.lumorpa`, else a relative
/// `.lumorpa`. This is **app-managed infrastructure** — like `lumo.db` or
/// `selector-stats.json`, and like Chrome's own default temp user-data-dir — so it
/// is *not* a user-supplied path and is not subject to per-step `fs.write` gating.
fn browser_profiles_root() -> PathBuf {
    let base = std::env::var_os("LUMO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".lumorpa"))
        })
        .unwrap_or_else(|| PathBuf::from(".lumorpa"));
    base.join("browser-profiles")
}

/// The stable Chrome user-data-dir for the named profile,
/// `<profiles-root>/<sanitized-name>`. The name is reduced to a single safe path
/// component (ASCII alphanumerics / `-` / `_`; everything else → `_`) so it can
/// never inject a separator or `..` and escape the root; an all-stripped name
/// falls back to `default`.
fn profile_user_data_dir(name: &str) -> PathBuf {
    let mut safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        safe.push_str("default");
    }
    browser_profiles_root().join(safe)
}

/// Build the Chrome launch config from `opts`.
///
/// With a `profile` the browser is **persistent** — a stable per-name
/// `--user-data-dir` ([`profile_user_data_dir`]) — and gets a minimal **stealth
/// baseline**: `--disable-blink-features=AutomationControlled` (drops the
/// `navigator.webdriver` signal that chromiumoxide's default `--enable-automation`
/// otherwise raises) plus `--no-default-browser-check` (no first-run prompt on a
/// fresh persistent profile). Deeper anti-detection (UA / canvas / JS fingerprint
/// patches) is intentionally out of scope for the baseline. With no profile the
/// browser is ephemeral with chromiumoxide's defaults — byte-for-byte the pre-T3
/// launch, so unbound flows are unaffected.
fn build_browser_config(opts: &LaunchOpts) -> Result<BrowserConfig, StepError> {
    let mut builder = BrowserConfig::builder();
    if !opts.headless {
        builder = builder.with_head();
    }
    if let Some(profile) = &opts.profile {
        let dir = profile_user_data_dir(profile);
        // Pre-create so an unwritable data root surfaces here with a clear path,
        // rather than as an opaque Chrome launch failure. Chrome creates it too;
        // best-effort, so a warning (not an error) if it can't be made yet.
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!(
                target: "lumo::browser",
                "browser profile dir {} not creatable ({e}); launching anyway",
                dir.display()
            );
        }
        builder = builder
            .user_data_dir(dir)
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--no-default-browser-check");
    }
    builder
        .build()
        .map_err(|e| StepError::msg(format!("chrome cfg: {e}")))
}

/// Open the `(run_id, slot)` browser session lazily, reusing the existing one if
/// already launched (open-once / reuse — "启动一次"). On a concurrent open race
/// for the same slot, the first writer wins and the loser we just launched is
/// reaped here (honoring the `ResourceFactory::open` idempotency contract);
/// serializing *use* across parallel branches is Phase 5.
async fn ensure_session(
    run_id: &str,
    slot: &str,
    opts: &LaunchOpts,
) -> Result<Arc<Session>, StepError> {
    {
        let lock = sessions();
        let g = lock.lock();
        if let Some(s) = g.get(run_id).and_then(|run| run.get(slot)).cloned() {
            return Ok(s);
        }
    }
    let cfg = build_browser_config(opts)?;

    let (browser, mut handler) = Browser::launch(cfg).await.map_err(|e| {
        // A persistent profile locks its user-data-dir, so only one run (or one
        // other local Chrome) can hold a given profile at a time — the usual cause
        // of a launch failure once profiles are in play. Surface that as a hint.
        let hint = if opts.profile.is_some() {
            " (a persistent browser profile can be used by only one run at a time — Chrome locks its user-data-dir; close other users of this profile)"
        } else {
            ""
        };
        StepError::msg(format!("chrome launch: {e}{hint}"))
    })?;
    let handle = tokio::spawn(async move { while let Some(_evt) = handler.next().await {} });
    let session = Arc::new(Session {
        browser: tokio::sync::Mutex::new(browser),
        handler: Mutex::new(Some(handle)),
        page: Mutex::new(None),
    });
    // Re-check under the write lock: another task may have opened this slot while
    // we were launching. If so, keep theirs and reap ours, so a race can't orphan
    // a headless Chrome (the loser would otherwise be dropped from the map and
    // escape end-of-run teardown). Decide *under* the lock into an owned value,
    // then await teardown *after* releasing it — a `parking_lot` guard is `!Send`
    // and must never be held across `.await`.
    let race_winner = {
        let lock = sessions();
        let mut g = lock.lock();
        match g.get(run_id).and_then(|run| run.get(slot)).cloned() {
            Some(existing) => Some(existing),
            None => {
                put_slot(&mut g, run_id, slot, session.clone());
                None
            }
        }
    };
    if let Some(existing) = race_winner {
        teardown_session(session).await;
        return Ok(existing);
    }
    Ok(session)
}

/// Look up an already-open `(run_id, slot)` session, or a "browser not launched"
/// error. The internal half of the consumer path.
fn session_for(run_id: &str, slot: &str) -> Result<Arc<Session>, StepError> {
    let lock = sessions();
    let session = lock.lock().get(run_id).and_then(|run| run.get(slot).cloned());
    session.ok_or_else(|| {
        if slot == DEFAULT_SLOT {
            StepError::msg("browser not launched")
        } else {
            // A resource-bound step: the run's default browser may be up while
            // this resource's slot isn't — name it so the error isn't confusing.
            StepError::msg(format!("browser not launched for resource `{slot}`"))
        }
    })
}

/// Resolve the browser session for the step described by `ctx` — its declared
/// `chromium.cdp` resource slot when bound, else the run's default slot. Every
/// read-only browser action calls this instead of keying by `run_id` directly.
fn session_for_ctx(ctx: &StepCtx) -> Result<Arc<Session>, StepError> {
    session_for(ctx.run_id(), &browser_slot(ctx))
}

/// The browser slot the step binds to: the current resource's name when it
/// references a declared `chromium.cdp` resource, else [`DEFAULT_SLOT`]. A
/// current-resource ref of a *non-browser* kind (or an undeclared one) falls
/// back to the default slot rather than keying a browser by an unrelated name.
fn browser_slot(ctx: &StepCtx) -> String {
    match ctx.current_resource() {
        Some(name) => match ctx.resource_decl(&name) {
            Ok(decl) if decl.kind == BROWSER_KIND => name,
            _ => DEFAULT_SLOT.to_string(),
        },
        None => DEFAULT_SLOT.to_string(),
    }
}

/// Build [`LaunchOpts`] from a `chromium.cdp` resource declaration: `headless`
/// from the flattened kind-specific `config` (default `true`, matching the
/// per-step `with.headless` default) and `profile` from the resource sub-field.
fn launch_opts_from_decl(decl: &ResourceDecl) -> LaunchOpts {
    let headless = decl
        .config
        .get("headless")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    LaunchOpts {
        headless,
        profile: decl.profile.clone(),
    }
}

/// Resolve the `(slot, opts)` a browser *opener* (`browser.launch` /
/// `browser.open`) should use: a step bound to a `chromium.cdp` resource opens
/// that named slot with the resource's declared opts; an unbound step opens the
/// default slot with `fallback` (the step's own `with:`). Back-compat: unbound
/// steps open exactly as before.
fn resolve_open_target(ctx: &StepCtx, fallback: LaunchOpts) -> (String, LaunchOpts) {
    if let Some(name) = ctx.current_resource() {
        if let Ok(decl) = ctx.resource_decl(&name) {
            if decl.kind == BROWSER_KIND {
                return (name, launch_opts_from_decl(&decl));
            }
        }
    }
    (DEFAULT_SLOT.to_string(), fallback)
}

fn current_page(s: &Session) -> Result<Page, StepError> {
    s.page
        .lock()
        .clone()
        .ok_or_else(|| StepError::msg("no browser page open; call `browser.open` first"))
}

// ─── session teardown (P1-2) ──────────────────────────────────────────────────

/// Force-close and reap *every* browser session for `run_id` (all slots),
/// aborting each CDP event-pump task. Idempotent — a no-op when no session
/// exists. This is the end-of-run teardown hook: a flow that fails (or omits
/// `browser.close`) can't orphan a headless Chrome, no matter how many browser
/// resources it declared.
pub async fn close_run_sessions(run_id: &str) {
    let removed = {
        let lock = sessions();
        let mut g = lock.lock();
        take_run(&mut g, run_id)
    };
    if let Some(run) = removed {
        for (_slot, s) in run {
            teardown_session(s).await;
        }
    }
}

/// Force-close and reap a single `(run_id, slot)` session, dropping the run
/// bucket once its last slot is gone. Idempotent. This is what the explicit
/// `browser.close` action calls, so closing a resource-bound step's browser
/// leaves the run's other browsers untouched.
async fn close_slot(run_id: &str, slot: &str) {
    let removed = {
        let lock = sessions();
        let mut g = lock.lock();
        take_slot(&mut g, run_id, slot)
    };
    if let Some(s) = removed {
        teardown_session(s).await;
    }
}

/// Whether any browser session is currently registered for `run_id` (any slot).
/// Exposed for tests; not part of the action surface.
#[doc(hidden)]
pub fn session_exists(run_id: &str) -> bool {
    sessions()
        .lock()
        .get(run_id)
        .is_some_and(|run| !run.is_empty())
}

/// Drop the active page, gracefully close the Chrome process and reap it, then
/// stop the event-pump task. Holds the last `Arc<Session>` so the `Browser` is
/// dropped only after Chrome has actually exited.
async fn teardown_session(s: Arc<Session>) {
    // Release the active page handle first.
    *s.page.lock() = None;
    // Graceful CDP `Browser.close`, falling back to `kill` if that fails, then
    // `wait` to reap the child so it can't linger as a zombie.
    {
        let mut browser = s.browser.lock().await;
        if browser.close().await.is_err() {
            let _ = browser.kill().await;
        }
        let _ = browser.wait().await;
    }
    // Stop the CDP event pump (take the handle out before aborting so the sync
    // guard isn't held across anything).
    let handle = s.handler.lock().take();
    if let Some(h) = handle {
        h.abort();
    }
}

/// End-of-run hook: reclaims the browser session keyed by the finished run.
struct BrowserTeardown;

#[async_trait]
impl RunTeardown for BrowserTeardown {
    async fn teardown(&self, run_id: &str) {
        close_run_sessions(run_id).await;
    }
}

/// T3 resource factory for `chromium.cdp` browsers. Opening a declared browser
/// resource is `ensure_session` keyed by the resource name (the slot) with the
/// declaration's profile/config as launch opts; the live `Session` stays in this
/// module's `SESSIONS` map, never in `StepCtx`. Reclamation is the existing
/// [`BrowserTeardown`] run hook (drops every slot for the run).
struct BrowserFactory;

#[async_trait]
impl ResourceFactory for BrowserFactory {
    fn kind(&self) -> &str {
        BROWSER_KIND
    }

    async fn open(&self, decl: &ResourceDecl, run_id: &str, name: &str) -> Result<(), StepError> {
        let _ = ensure_session(run_id, name, &launch_opts_from_decl(decl)).await?;
        Ok(())
    }
}

// ─── browser.launch ─────────────────────────────────────────────────────────

pub struct LaunchAction;
#[derive(Deserialize, Default, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LaunchIn {
    #[serde(default = "default_true")]
    headless: bool,
}
fn default_true() -> bool {
    true
}

#[async_trait]
impl Action for LaunchAction {
    fn id(&self) -> &'static str {
        "browser.launch"
    }
    fn summary(&self) -> &'static str {
        "Launch (or attach to) a Chromium browser session"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<LaunchIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LaunchIn { headless } = serde_json::from_value(input).unwrap_or_default();
        let (slot, opts) = resolve_open_target(
            ctx,
            LaunchOpts {
                headless,
                profile: None,
            },
        );
        let _ = ensure_session(ctx.run_id(), &slot, &opts).await?;
        Ok(ActionResult::from(
            serde_json::json!({ "ok": true, "headless": opts.headless }),
        ))
    }
}

// ─── browser.close ──────────────────────────────────────────────────────────

pub struct CloseAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseIn {}

#[async_trait]
impl Action for CloseAction {
    fn id(&self) -> &'static str {
        "browser.close"
    }
    fn summary(&self) -> &'static str {
        "Close the current browser session"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<CloseIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        // Force-close and reap this step's session (its bound resource slot, or
        // the run default) so the Chrome process is terminated, not just unlinked
        // from the map (P1-2). A resource-bound close leaves the run's other
        // browsers untouched, and is a no-op if this step's slot was never opened
        // (the run's default browser, if any, keeps running). End-of-run teardown
        // still reaps every slot regardless.
        close_slot(ctx.run_id(), &browser_slot(ctx)).await;
        Ok(ActionResult::null())
    }
}

// ─── browser.open ───────────────────────────────────────────────────────────

pub struct OpenAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenIn {
    url: String,
    #[serde(default = "default_true")]
    headless: bool,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
    #[serde(default)]
    wait_for: Option<String>,
}
fn default_timeout_ms() -> u64 {
    30_000
}

/// Reconcile the back-compat `selector: String` field with the new
/// `selectors: { ... }` object. If both are absent, error out — every action
/// requires at least one strategy.
fn build_selector(
    css_string: Option<String>,
    spec: Option<MultiSelector>,
) -> Result<MultiSelector, StepError> {
    let mut out = spec.unwrap_or_default();
    if let Some(css) = css_string {
        if out.css.is_none() && !css.is_empty() {
            out.css = Some(css);
        }
    }
    if out.is_empty() {
        return Err(StepError::msg(
            "browser action requires `selector:` (CSS) or `selectors: { ... }` with at least one strategy",
        ));
    }
    Ok(out)
}

/// Resolve with DOM strategies first; on `SelectorNotFound`, fall through to
/// the Vision-LLM router (S-11/S-12) when the step provides:
///
/// * an `AiHookProvider` on the context (router configured + flow opted in),
/// * a natural-language `prompt:` describing the target.
///
/// Without either, vision is skipped and the original DOM failure surfaces
/// — so back-compat with M1 stays intact. The strategy name returned
/// becomes `vision_bbox` / `vision_som` so step output records *which*
/// fingerprint kept the flow alive.
async fn resolve_with_vision_fallback(
    ctx: &lumo_core::StepCtx,
    page: &chromiumoxide::Page,
    spec: &MultiSelector,
    prompt: Option<&str>,
    model: Option<&str>,
    timeout_ms: u64,
) -> Result<(chromiumoxide::Element, &'static str), StepError> {
    match resolve_element(page, spec, timeout_ms).await {
        Ok(pair) => Ok(pair),
        Err(dom_err) => {
            let (Some(provider), Some(prompt)) = (ctx.ai_provider(), prompt) else {
                return Err(dom_err);
            };
            let prompt = prompt.trim();
            if prompt.is_empty() {
                return Err(dom_err);
            }
            tracing::warn!(
                target: "lumo::vision",
                "DOM resolve failed for `{}`; trying vision fallback: {dom_err}",
                spec.first_hint()
            );
            resolve_via_vision(page, provider, prompt, model, timeout_ms).await
        }
    }
}

#[async_trait]
impl Action for OpenAction {
    fn id(&self) -> &'static str {
        "browser.open"
    }
    fn summary(&self) -> &'static str {
        "Navigate to a URL (launching browser if needed)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<OpenIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let OpenIn {
            url,
            headless,
            timeout_ms,
            wait_for,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.open input invalid: {e}")))?;
        ctx.ensure_network_url(&url)?;
        let (slot, opts) = resolve_open_target(
            ctx,
            LaunchOpts {
                headless,
                profile: None,
            },
        );
        let s = ensure_session(ctx.run_id(), &slot, &opts).await?;
        let page = {
            let browser = s.browser.lock().await;
            tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                browser.new_page(url.as_str()),
            )
            .await
            .map_err(|_| StepError::msg(format!("timeout opening {url}")))?
            .map_err(|e| StepError::msg(format!("new_page: {e}")))?
        };
        let _ = page.wait_for_navigation().await;
        if let Some(selector) = wait_for {
            tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                page.find_element(&selector),
            )
            .await
            .map_err(|_| StepError::SelectorNotFound(selector.clone()))?
            .map_err(|_| StepError::SelectorNotFound(selector.clone()))?;
        }
        *s.page.lock() = Some(page);
        Ok(ActionResult::from(serde_json::json!({ "url": url })))
    }
}

// ─── browser.click ──────────────────────────────────────────────────────────

pub struct ClickAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClickIn {
    /// Single CSS selector (back-compat). Either this or `selectors` must be set.
    #[serde(default)]
    selector: Option<String>,
    /// Multi-strategy selector spec. The runner tries fingerprints in cost
    /// order and surfaces which one matched in the step output.
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// Natural-language target description. When DOM strategies fail and an
    /// AI hook provider is attached, the Vision-LLM (S-11/S-12) uses this
    /// prompt to locate the element by sight.
    #[serde(default)]
    prompt: Option<String>,
    /// Optional model override for the vision fallback. Empty ⇒ inherit
    /// from `metadata.ai.model`.
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ClickAction {
    fn id(&self) -> &'static str {
        "browser.click"
    }
    fn summary(&self) -> &'static str {
        "Click the first element matching a CSS selector or multi-strategy selectors"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ClickIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ClickIn {
            selector,
            selectors,
            prompt,
            model,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.click input invalid: {e}")))?;
        let spec = build_selector(selector, selectors)?;
        let s = session_for_ctx(_ctx)?;
        let page = current_page(&s)?;
        let hint = spec.first_hint();
        let (element, strategy) = resolve_with_vision_fallback(
            _ctx,
            &page,
            &spec,
            prompt.as_deref(),
            model.as_deref(),
            timeout_ms,
        )
        .await?;
        element
            .click()
            .await
            .map_err(|e| StepError::msg(format!("click `{hint}`: {e}")))?;
        clear_marker(&page).await;
        Ok(ActionResult::from(serde_json::json!({
            "resolved_by": strategy,
            "matched": hint,
        })))
    }
}

// ─── browser.type ───────────────────────────────────────────────────────────

pub struct TypeAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TypeIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    text: String,
    #[serde(default)]
    clear: bool,
    /// Natural-language target description for vision fallback (S-11/S-12).
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for TypeAction {
    fn id(&self) -> &'static str {
        "browser.type"
    }
    fn summary(&self) -> &'static str {
        "Type text into the first element matching a selector spec"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<TypeIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TypeIn {
            selector,
            selectors,
            text,
            clear,
            prompt,
            model,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.type input invalid: {e}")))?;
        let spec = build_selector(selector, selectors)?;
        let s = session_for_ctx(_ctx)?;
        let page = current_page(&s)?;
        let hint = spec.first_hint();
        let (element, strategy) = resolve_with_vision_fallback(
            _ctx,
            &page,
            &spec,
            prompt.as_deref(),
            model.as_deref(),
            timeout_ms,
        )
        .await?;
        if clear {
            let _ = element.focus().await;
            let _ = page
                .evaluate("document.querySelector('[data-lumo-resolved=\"1\"]').value = ''")
                .await;
        }
        element.click().await.ok();
        element
            .type_str(&text)
            .await
            .map_err(|e| StepError::msg(format!("type: {e}")))?;
        clear_marker(&page).await;
        Ok(ActionResult::from(serde_json::json!({
            "resolved_by": strategy,
            "matched": hint,
            "typed": text.len(),
        })))
    }
}

// ─── browser.extract ────────────────────────────────────────────────────────

pub struct ExtractAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExtractIn {
    /// CSS selector. If `map` is provided, each value is treated as a sub-selector
    /// rooted at the matched element; otherwise innerText is returned.
    selector: String,
    #[serde(default)]
    map: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    attr: Option<String>,
    #[serde(default)]
    all: bool,
    /// Extract from inside this iframe instead of the main frame.
    #[serde(default)]
    frame: Option<FrameSel>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ExtractAction {
    fn id(&self) -> &'static str {
        "browser.extract"
    }
    fn summary(&self) -> &'static str {
        "Extract innerText (or a field map) from matching elements"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ExtractIn>);
        &SCHEMA
    }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtractIn {
            selector,
            map,
            attr,
            all,
            frame,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.extract input invalid: {e}")))?;
        if frame.as_ref().is_some_and(FrameSel::is_empty) {
            return Err(StepError::msg(
                "browser.extract: `frame` requires `url_includes` or `name`",
            ));
        }
        let s = session_for_ctx(_ctx)?;
        let page = current_page(&s)?;

        // On any extraction failure, stash a page screenshot so the VM's
        // `extract_visual` AI hook (step.ai.mode: fallback/primary) can pass it
        // to a vision model for true multimodal extraction.
        async fn stash_on_extract_fail(ctx: &StepCtx, page: &chromiumoxide::Page) {
            if let Ok(png) = crate::vision::screenshot_png(page).await {
                ctx.stash_screenshot(png);
            }
        }

        // Build a JS function returning the extracted JSON shape, then evaluate.
        let map_json = serde_json::to_string(&map.unwrap_or_default()).unwrap_or("{}".into());
        let attr_json = serde_json::to_string(&attr).unwrap_or("null".into());
        let js = format!(
            r#"
(() => {{
  const sel = {sel};
  const all = {all};
  const map = {map};
  const attr = {attr};
  const read = (el, specAttr) => {{
    if (!el) return null;
    if (specAttr) return el.getAttribute(specAttr);
    return el.innerText;
  }};
  const pick = (el) => {{
    if (!el) return null;
    if (Object.keys(map).length === 0) return read(el, attr);
    const out = {{}};
    for (const [k, v] of Object.entries(map)) {{
      const subSelector = typeof v === 'string' ? v : v.selector;
      const subAttr = typeof v === 'object' ? v.attr : null;
      const sub = el.querySelector(subSelector);
      out[k] = read(sub, subAttr);
    }}
    return out;
  }};
  if (all) {{
    return Array.from(document.querySelectorAll(sel)).map(pick);
  }}
  return pick(document.querySelector(sel));
}})()
"#,
            sel = serde_json::to_string(&selector).unwrap(),
            all = all,
            map = map_json,
            attr = attr_json
        );

        let result: Value = if let Some(sel) = frame.as_ref() {
            // iframe-scoped: run the same extraction script in the frame's context.
            match eval_in_frame(&page, js, sel, timeout_ms).await {
                Ok(v) => v,
                Err(e) => {
                    stash_on_extract_fail(_ctx, &page).await;
                    return Err(StepError::ExtractFailed(format!(
                        "frame extract `{selector}`: {e}"
                    )));
                }
            }
        } else {
            let eval =
                tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js)).await;
            match eval {
                Err(_) => {
                    stash_on_extract_fail(_ctx, &page).await;
                    return Err(StepError::ExtractFailed(format!(
                        "timeout extracting `{selector}`"
                    )));
                }
                Ok(Err(e)) => {
                    stash_on_extract_fail(_ctx, &page).await;
                    return Err(StepError::ExtractFailed(format!(
                        "extract eval `{selector}`: {e}"
                    )));
                }
                Ok(Ok(v)) => v.into_value().unwrap_or(Value::Null),
            }
        };
        if result.is_null() {
            stash_on_extract_fail(_ctx, &page).await;
            return Err(StepError::ExtractFailed(format!(
                "selector `{selector}` matched no element"
            )));
        }
        if all {
            if let Value::Array(a) = &result {
                if a.is_empty() {
                    stash_on_extract_fail(_ctx, &page).await;
                    return Err(StepError::ExtractFailed(format!(
                        "selector `{selector}` matched no elements"
                    )));
                }
            }
        }
        Ok(ActionResult::from(result))
    }
}

// ─── browser.wait (F-9) ───────────────────────────────────────────────────────

/// Per-poll JS-eval budget. The query itself is cheap; this only bounds a
/// pathological evaluate() call, not the overall wait (that's `timeout_ms`).
const WAIT_EVAL_TIMEOUT_MS: u64 = 2_000;

/// Browser-wait condition. An enum (not a free `String`) keeps the
/// `["present","visible","clickable","hidden"]` constraint in the derived schema
/// and folds the old `WAIT_CONDITIONS` runtime check into the type system.
#[derive(Deserialize, JsonSchema, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum WaitCondition {
    Present,
    #[default]
    Visible,
    Clickable,
    Hidden,
}
impl WaitCondition {
    fn as_str(&self) -> &'static str {
        match self {
            WaitCondition::Present => "present",
            WaitCondition::Visible => "visible",
            WaitCondition::Clickable => "clickable",
            WaitCondition::Hidden => "hidden",
        }
    }
}

/// Self-contained matcher: locates the element by the same strategy order as the
/// resolver, then returns a single boolean for the requested condition. Kept
/// separate from `resolve_element` so the poll loop never writes SelectorStats.
const WAIT_JS_TEMPLATE: &str = r#"
((spec, condition, needle) => {
  const escape = (s) => (window.CSS && CSS.escape) ? CSS.escape(String(s)) : String(s).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
  const find = () => {
    if (spec.id) { const e = document.getElementById(spec.id); if (e) return e; }
    if (spec.data_testid) { const e = document.querySelector(`[data-testid="${escape(spec.data_testid)}"]`); if (e) return e; }
    if (spec.css) { const e = document.querySelector(spec.css); if (e) return e; }
    if (spec.aria_label) {
      const e = document.querySelector(`[aria-label="${escape(spec.aria_label)}"]`);
      if (e) return e;
      const m = Array.from(document.querySelectorAll('*')).find((el) => el.getAttribute && el.getAttribute('aria-label') === spec.aria_label);
      if (m) return m;
    }
    if (spec.text_includes) {
      const t = String(spec.text_includes).trim();
      const cands = document.querySelectorAll('button, a, span, label, div, li, td, th, h1, h2, h3, h4, h5, h6, p');
      for (const el of cands) { if ((el.innerText || '').trim().includes(t)) return el; }
    }
    if (spec.xpath) {
      try { const r = document.evaluate(spec.xpath, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null); if (r.singleNodeValue) return r.singleNodeValue; } catch (_) {}
    }
    return null;
  };
  const hasSpec = !!(spec.id || spec.data_testid || spec.css || spec.aria_label || spec.text_includes || spec.xpath);
  const visible = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    if (!(r.width > 0 && r.height > 0)) return false;
    const st = window.getComputedStyle(el);
    if (st.visibility === 'hidden' || st.display === 'none' || parseFloat(st.opacity) === 0) return false;
    return true;
  };
  const clickable = (el) => visible(el) && !el.disabled && el.getAttribute('aria-disabled') !== 'true';
  const containsText = (el, n) => !!el && (el.innerText || '').includes(n);
  if (!hasSpec) {
    return !!(document.body && (document.body.innerText || '').includes(needle || ''));
  }
  const el = find();
  switch (condition) {
    case 'present': return !!el;
    case 'visible': return visible(el) && (needle ? containsText(el, needle) : true);
    case 'clickable': return clickable(el) && (needle ? containsText(el, needle) : true);
    case 'hidden': return !el || !visible(el);
    default: return false;
  }
})(__SPEC__, "__COND__", __NEEDLE__)
"#;

pub struct WaitAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    #[serde(default)]
    condition: WaitCondition,
    #[serde(default)]
    text: Option<String>,
    #[serde(default = "default_wait_timeout_ms")]
    timeout_ms: u64,
}
fn default_wait_timeout_ms() -> u64 {
    30_000
}

async fn wait_matches(
    page: &Page,
    spec: Option<&MultiSelector>,
    condition: &str,
    text: Option<&str>,
) -> Result<bool, StepError> {
    let spec_json = match spec {
        Some(s) => serde_json::json!({
            "id": s.id, "data_testid": s.data_testid, "css": s.css,
            "aria_label": s.aria_label, "text_includes": s.text_includes, "xpath": s.xpath,
        }),
        None => serde_json::json!({}),
    };
    let needle_json = serde_json::to_string(text.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    let js = WAIT_JS_TEMPLATE
        .replace("__SPEC__", &spec_json.to_string())
        .replace("__COND__", condition)
        .replace("__NEEDLE__", &needle_json);
    let val = tokio::time::timeout(
        Duration::from_millis(WAIT_EVAL_TIMEOUT_MS),
        page.evaluate(js),
    )
    .await
    .map_err(|_| StepError::msg("browser.wait: page eval timed out"))?
    .map_err(|e| StepError::msg(format!("browser.wait eval: {e}")))?;
    Ok(val.into_value::<bool>().unwrap_or(false))
}

#[async_trait]
impl Action for WaitAction {
    fn id(&self) -> &'static str {
        "browser.wait"
    }
    fn summary(&self) -> &'static str {
        "Wait until an element is present/visible/clickable/hidden, or text appears"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WaitIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WaitIn {
            selector,
            selectors,
            condition,
            text,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.wait input invalid: {e}")))?;
        // `condition` is a `WaitCondition` enum, so the schema/deserializer have
        // already rejected anything outside present/visible/clickable/hidden —
        // the old runtime `WAIT_CONDITIONS` check is now unrepresentable.
        let cond = condition.as_str();
        let has_selector = selector.as_ref().is_some_and(|s| !s.is_empty())
            || selectors.as_ref().is_some_and(|s| !s.is_empty());
        if !has_selector && text.is_none() {
            return Err(StepError::msg(
                "browser.wait requires `selector`/`selectors` or `text`",
            ));
        }
        let spec = if has_selector {
            Some(build_selector(selector, selectors)?)
        } else {
            None
        };

        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        let deadline = Duration::from_millis(timeout_ms);
        let start = std::time::Instant::now();
        let poll = Duration::from_millis(100);
        loop {
            if wait_matches(&page, spec.as_ref(), cond, text.as_deref()).await? {
                let matched = spec
                    .as_ref()
                    .map(|s| s.first_hint())
                    .unwrap_or_else(|| format!("text:{}", text.as_deref().unwrap_or("")));
                return Ok(ActionResult::from(serde_json::json!({
                    "condition": cond,
                    "matched": matched,
                    "waited_ms": start.elapsed().as_millis() as u64,
                })));
            }
            if start.elapsed() >= deadline {
                let what = spec
                    .as_ref()
                    .map(|s| s.first_hint())
                    .unwrap_or_else(|| format!("text `{}`", text.as_deref().unwrap_or("")));
                return Err(StepError::msg(format!(
                    "browser.wait: condition `{cond}` not met within {timeout_ms}ms for {what}"
                )));
            }
            tokio::time::sleep(poll).await;
        }
    }
}

// ─── iframe-scoped eval (F-10 part 2) ──────────────────────────────────────────
// chromiumoxide's high-level API (find_element/evaluate) only sees the main frame.
// To run JS inside an <iframe> we match the frame (by URL substring or name), take
// its JS execution context, and issue Runtime.evaluate against that context. This
// backs the optional `frame:` on `browser.eval` / `browser.extract`. Driving the
// selector engine *inside* a frame isn't supported by chromiumoxide 0.7, so DOM
// strategies (click/type) stay main-frame; reach into frames via JS here.

/// Address an iframe to run script in. Exactly one field is used (`url_includes`
/// wins, then `name`, then `index`).
#[derive(Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
struct FrameSel {
    /// Match the first frame whose URL contains this substring.
    #[serde(default)]
    url_includes: Option<String>,
    /// Match the first frame whose name equals this.
    #[serde(default)]
    name: Option<String>,
    /// Match the frame at this zero-based position in the page's frame list.
    #[serde(default)]
    index: Option<usize>,
}

impl FrameSel {
    fn is_empty(&self) -> bool {
        self.url_includes.is_none() && self.name.is_none() && self.index.is_none()
    }
}

/// Resolve the JS execution context of the iframe matching `sel`.
async fn resolve_frame_context(
    page: &Page,
    sel: &FrameSel,
) -> Result<ExecutionContextId, StepError> {
    let frames = page
        .frames()
        .await
        .map_err(|e| StepError::msg(format!("frames: {e}")))?;
    // Index addressing: pick directly by position (chromiumoxide's frame list is
    // ordered by the CDP frame tree, so index is stable within a page load).
    if let Some(idx) = sel.index {
        let fid = frames.get(idx).cloned().ok_or_else(|| {
            StepError::msg(format!(
                "frame index {idx} out of range (page has {} frames)",
                frames.len()
            ))
        })?;
        return page
            .frame_execution_context(fid)
            .await
            .map_err(|e| StepError::msg(format!("frame context: {e}")))?
            .ok_or_else(|| {
                StepError::msg("matched iframe has no execution context yet (still loading?)")
            });
    }
    for fid in frames {
        let hit = if let Some(sub) = sel.url_includes.as_deref() {
            page.frame_url(fid.clone())
                .await
                .ok()
                .flatten()
                .is_some_and(|u| u.contains(sub))
        } else if let Some(name) = sel.name.as_deref() {
            page.frame_name(fid.clone())
                .await
                .ok()
                .flatten()
                .is_some_and(|n| n == name)
        } else {
            false
        };
        if hit {
            return page
                .frame_execution_context(fid)
                .await
                .map_err(|e| StepError::msg(format!("frame context: {e}")))?
                .ok_or_else(|| {
                    StepError::msg("matched iframe has no execution context yet (still loading?)")
                });
        }
    }
    Err(StepError::msg("no iframe matched `frame`"))
}

/// Evaluate `expr` inside the iframe matched by `sel`, returning its JSON value.
/// A thrown JS exception surfaces as an error rather than a silent null.
async fn eval_in_frame(
    page: &Page,
    expr: String,
    sel: &FrameSel,
    timeout_ms: u64,
) -> Result<Value, StepError> {
    let ctx_id = resolve_frame_context(page, sel).await?;
    let params = EvaluateParams::builder()
        .expression(expr)
        .context_id(ctx_id)
        .return_by_value(true)
        .await_promise(true)
        .build()
        .map_err(|e| StepError::msg(format!("frame eval params: {e}")))?;
    let resp = tokio::time::timeout(Duration::from_millis(timeout_ms), page.execute(params))
        .await
        .map_err(|_| StepError::msg("timed out"))?
        .map_err(|e| StepError::msg(format!("{e}")))?;
    let returns = resp.result;
    if let Some(exc) = returns.exception_details {
        return Err(StepError::msg(format!("threw: {}", exc.text)));
    }
    Ok(returns.result.value.unwrap_or(Value::Null))
}

// ─── browser.info ───────────────────────────────────────────────────────────

pub struct InfoAction;

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum InfoField {
    Url,
    Title,
    Html,
    Text,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InfoIn {
    #[serde(default = "default_info_fields")]
    fields: Vec<InfoField>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

fn default_info_fields() -> Vec<InfoField> {
    vec![
        InfoField::Url,
        InfoField::Title,
        InfoField::Html,
        InfoField::Text,
    ]
}

#[async_trait]
impl Action for InfoAction {
    fn id(&self) -> &'static str {
        "browser.info"
    }
    fn summary(&self) -> &'static str {
        "Read URL, title, HTML, or visible text from the current page"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<InfoIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let InfoIn { fields, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.info input invalid: {e}")))?;
        let fields = if fields.is_empty() {
            default_info_fields()
        } else {
            fields
        };
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let mut out = Map::new();
        for field in fields {
            match field {
                InfoField::Url => {
                    out.insert(
                        "url".into(),
                        Value::String(page_url(&page, timeout_ms).await?),
                    );
                }
                InfoField::Title => {
                    out.insert(
                        "title".into(),
                        Value::String(page_title(&page, timeout_ms).await?),
                    );
                }
                InfoField::Html => {
                    out.insert(
                        "html".into(),
                        Value::String(
                            page_eval_string(
                                &page,
                                "document.documentElement ? document.documentElement.outerHTML : ''",
                                timeout_ms,
                            )
                            .await?,
                        ),
                    );
                }
                InfoField::Text => {
                    out.insert(
                        "text".into(),
                        Value::String(
                            page_eval_string(
                                &page,
                                "document.body ? document.body.innerText : ''",
                                timeout_ms,
                            )
                            .await?,
                        ),
                    );
                }
            }
        }
        Ok(ActionResult::from(Value::Object(out)))
    }
}

async fn page_url(page: &Page, timeout_ms: u64) -> Result<String, StepError> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), page.url())
        .await
        .map_err(|_| StepError::msg("browser.info url: timed out"))?
        .map_err(|e| StepError::msg(format!("browser.info url: {e}")))?
        .ok_or_else(|| StepError::msg("browser.info url: unavailable"))
}

async fn page_title(page: &Page, timeout_ms: u64) -> Result<String, StepError> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), page.get_title())
        .await
        .map_err(|_| StepError::msg("browser.info title: timed out"))?
        .map_err(|e| StepError::msg(format!("browser.info title: {e}")))?
        .ok_or_else(|| StepError::msg("browser.info title: unavailable"))
}

async fn page_eval_string(page: &Page, expr: &str, timeout_ms: u64) -> Result<String, StepError> {
    let eval = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        page.evaluate(expr.to_string()),
    )
    .await
    .map_err(|_| StepError::msg("browser.info evaluate: timed out"))?
    .map_err(|e| StepError::msg(format!("browser.info evaluate: {e}")))?;
    Ok(match eval.into_value().unwrap_or(Value::Null) {
        Value::String(s) => s,
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

// ─── browser.eval (F-10) ──────────────────────────────────────────────────────

pub struct EvalAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EvalIn {
    /// JavaScript evaluated in the page; its result is returned as JSON. Runs in
    /// the same page context as `browser.extract` — arbitrary script, no new
    /// capability (the flow already drives this browser).
    expr: String,
    /// Run the script inside this iframe instead of the main frame.
    #[serde(default)]
    frame: Option<FrameSel>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for EvalAction {
    fn id(&self) -> &'static str {
        "browser.eval"
    }
    fn summary(&self) -> &'static str {
        "Evaluate a JavaScript expression in the current page, returning its JSON result"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<EvalIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let EvalIn {
            expr,
            frame,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.eval input invalid: {e}")))?;
        // Validate the frame address up front so a malformed call fails before Chrome.
        if frame.as_ref().is_some_and(FrameSel::is_empty) {
            return Err(StepError::msg(
                "browser.eval: `frame` requires `url_includes` or `name`",
            ));
        }
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        if let Some(sel) = frame.as_ref() {
            let result = eval_in_frame(&page, expr, sel, timeout_ms)
                .await
                .map_err(|e| StepError::msg(format!("browser.eval: {e}")))?;
            return Ok(ActionResult::from(result));
        }
        let eval =
            tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(expr)).await;
        let result = match eval {
            Err(_) => return Err(StepError::msg("browser.eval: timed out")),
            Ok(Err(e)) => return Err(StepError::msg(format!("browser.eval: {e}"))),
            Ok(Ok(v)) => v.into_value().unwrap_or(Value::Null),
        };
        Ok(ActionResult::from(result))
    }
}

// ─── browser.screenshot (F-10) ─────────────────────────────────────────────────

pub struct ScreenshotAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScreenshotIn {
    /// Destination PNG path (gated by the fs-write capability).
    path: String,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ScreenshotAction {
    fn id(&self) -> &'static str {
        "browser.screenshot"
    }
    fn summary(&self) -> &'static str {
        "Capture a full-page PNG screenshot of the current page to a file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ScreenshotIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ScreenshotIn { path, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.screenshot input invalid: {e}")))?;
        // Gate the write BEFORE touching the browser, so an ungranted dest fails
        // fast (and without a live Chrome) — same order as `http.download`.
        let dest = PathBuf::from(&path);
        ctx.ensure_fs_write(&dest)?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let png = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            crate::vision::screenshot_png(&page),
        )
        .await
        .map_err(|_| StepError::msg("browser.screenshot: timed out"))??;
        if let Some(parent) = dest.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&dest, &png).await.map_err(|e| {
            StepError::msg(format!("browser.screenshot write {}: {e}", dest.display()))
        })?;
        Ok(ActionResult::from(serde_json::json!({
            "path": path,
            "bytes": png.len(),
        })))
    }
}

// ─── browser.scroll (F-10) ─────────────────────────────────────────────────────

/// Named window-scroll target for `browser.scroll` when no selector is given.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ScrollTo {
    Top,
    Bottom,
}

pub struct ScrollAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScrollIn {
    /// Scroll this element into view. When absent, the window is scrolled.
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// Window target when no selector: "top" or "bottom" (overrides x/y).
    #[serde(default)]
    to: Option<ScrollTo>,
    /// Window scroll delta in pixels when no selector and no `to`.
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ScrollAction {
    fn id(&self) -> &'static str {
        "browser.scroll"
    }
    fn summary(&self) -> &'static str {
        "Scroll an element into view, or scroll the window to top/bottom or by a delta"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ScrollIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ScrollIn {
            selector,
            selectors,
            to,
            x,
            y,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.scroll input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        let has_selector = selector.as_ref().is_some_and(|s| !s.is_empty())
            || selectors.as_ref().is_some_and(|s| !s.is_empty());
        if has_selector {
            let spec = build_selector(selector, selectors)?;
            let (element, strategy) = resolve_element(&page, &spec, timeout_ms).await?;
            let hint = spec.first_hint();
            element
                .scroll_into_view()
                .await
                .map_err(|e| StepError::msg(format!("browser.scroll `{hint}`: {e}")))?;
            clear_marker(&page).await;
            return Ok(ActionResult::from(serde_json::json!({
                "scrolled_to": hint,
                "resolved_by": strategy,
            })));
        }

        let (js, label) = match to {
            Some(ScrollTo::Top) => ("window.scrollTo(0, 0)".to_string(), "top"),
            Some(ScrollTo::Bottom) => (
                "window.scrollTo(0, document.body.scrollHeight)".to_string(),
                "bottom",
            ),
            None => (format!("window.scrollBy({x}, {y})"), "delta"),
        };
        page.evaluate(js)
            .await
            .map_err(|e| StepError::msg(format!("browser.scroll: {e}")))?;
        Ok(ActionResult::from(
            serde_json::json!({ "scrolled": label, "x": x, "y": y }),
        ))
    }
}

// ─── browser.hover (F-10) ──────────────────────────────────────────────────────

pub struct HoverAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HoverIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// Natural-language target for the vision fallback (S-11/S-12), like `browser.click`.
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for HoverAction {
    fn id(&self) -> &'static str {
        "browser.hover"
    }
    fn summary(&self) -> &'static str {
        "Hover the pointer over the first element matching a selector spec"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<HoverIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let HoverIn {
            selector,
            selectors,
            prompt,
            model,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.hover input invalid: {e}")))?;
        let spec = build_selector(selector, selectors)?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let hint = spec.first_hint();
        let (element, strategy) = resolve_with_vision_fallback(
            ctx,
            &page,
            &spec,
            prompt.as_deref(),
            model.as_deref(),
            timeout_ms,
        )
        .await?;
        element
            .hover()
            .await
            .map_err(|e| StepError::msg(format!("hover `{hint}`: {e}")))?;
        clear_marker(&page).await;
        Ok(ActionResult::from(serde_json::json!({
            "resolved_by": strategy,
            "matched": hint,
        })))
    }
}

// ─── browser.select (F-10) ─────────────────────────────────────────────────────

pub struct SelectAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SelectIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// Pick the option whose `value` equals this.
    #[serde(default)]
    value: Option<String>,
    /// Pick the option whose visible text equals this (trimmed).
    #[serde(default)]
    label: Option<String>,
    /// Pick the option at this zero-based index.
    #[serde(default)]
    index: Option<i64>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for SelectAction {
    fn id(&self) -> &'static str {
        "browser.select"
    }
    fn summary(&self) -> &'static str {
        "Choose an option in a <select> by value, visible label, or index"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SelectIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SelectIn {
            selector,
            selectors,
            value,
            label,
            index,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.select input invalid: {e}")))?;
        if value.is_none() && label.is_none() && index.is_none() {
            return Err(StepError::msg(
                "browser.select requires one of `value`, `label`, or `index`",
            ));
        }
        let spec = build_selector(selector, selectors)?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let hint = spec.first_hint();
        // resolve_element marks the winner with data-lumo-resolved="1"; the JS
        // below drives that marked <select>, then we clear the marker.
        let (_element, strategy) = resolve_element(&page, &spec, timeout_ms).await?;

        let value_json = serde_json::to_string(&value).unwrap_or_else(|_| "null".into());
        let label_json = serde_json::to_string(&label).unwrap_or_else(|_| "null".into());
        let index_json = serde_json::to_string(&index).unwrap_or_else(|_| "null".into());
        let js = format!(
            r#"
(() => {{
  const el = document.querySelector('[data-lumo-resolved="1"]');
  if (!el || el.tagName !== 'SELECT') return {{ ok: false, error: 'not a <select>' }};
  const wantValue = {value_json};
  const wantLabel = {label_json};
  const wantIndex = {index_json};
  const opts = Array.from(el.options);
  let chosen = -1;
  if (wantIndex !== null) {{
    if (wantIndex >= 0 && wantIndex < opts.length) chosen = wantIndex;
  }} else if (wantValue !== null) {{
    chosen = opts.findIndex((o) => o.value === wantValue);
  }} else if (wantLabel !== null) {{
    const want = String(wantLabel).trim();
    chosen = opts.findIndex((o) => (o.textContent || '').trim() === want);
  }}
  if (chosen < 0) return {{ ok: false, error: 'no matching option' }};
  el.selectedIndex = chosen;
  el.dispatchEvent(new Event('input', {{ bubbles: true }}));
  el.dispatchEvent(new Event('change', {{ bubbles: true }}));
  return {{ ok: true, value: el.value, label: (opts[chosen].textContent || '').trim(), index: chosen }};
}})()
"#
        );
        let eval = tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js)).await;
        let result: Value = match eval {
            Err(_) => {
                clear_marker(&page).await;
                return Err(StepError::msg(format!(
                    "browser.select `{hint}`: timed out"
                )));
            }
            Ok(Err(e)) => {
                clear_marker(&page).await;
                return Err(StepError::msg(format!("browser.select `{hint}`: {e}")));
            }
            Ok(Ok(v)) => v.into_value().unwrap_or(Value::Null),
        };
        clear_marker(&page).await;
        if result.get("ok").and_then(Value::as_bool) != Some(true) {
            let err = result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("selection failed");
            return Err(StepError::msg(format!("browser.select `{hint}`: {err}")));
        }
        Ok(ActionResult::from(serde_json::json!({
            "resolved_by": strategy,
            "matched": hint,
            "selected": result,
        })))
    }
}

// ─── browser.cookies (F-10) ────────────────────────────────────────────────────

pub struct CookiesAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CookiesIn {
    /// If set, return only the cookie(s) with this exact name.
    #[serde(default)]
    name: Option<String>,
}

#[async_trait]
impl Action for CookiesAction {
    fn id(&self) -> &'static str {
        "browser.cookies"
    }
    fn summary(&self) -> &'static str {
        "Read the current page's cookies (optionally filtered by name)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<CookiesIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let CookiesIn { name } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.cookies input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let cookies = page
            .get_cookies()
            .await
            .map_err(|e| StepError::msg(format!("browser.cookies: {e}")))?;
        let out: Vec<Value> = cookies
            .iter()
            .filter(|c| name.as_deref().is_none_or(|n| c.name.as_str() == n))
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .collect();
        Ok(ActionResult::from(Value::Array(out)))
    }
}

// ─── browser.set_cookie (F-10) ─────────────────────────────────────────────────

pub struct SetCookieAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetCookieIn {
    name: String,
    value: String,
    /// Target URL (CDP derives domain/path from it). Defaults to the current page
    /// URL when neither `url` nor `domain` is given.
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    secure: Option<bool>,
    #[serde(default)]
    http_only: Option<bool>,
}

#[async_trait]
impl Action for SetCookieAction {
    fn id(&self) -> &'static str {
        "browser.set_cookie"
    }
    fn summary(&self) -> &'static str {
        "Set a cookie on the current browser session"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<SetCookieIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let SetCookieIn {
            name,
            value,
            url,
            domain,
            path,
            secure,
            http_only,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.set_cookie input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        // CDP scopes a cookie by url or domain; fall back to the current page URL
        // when the caller supplies neither.
        let url = match (url, &domain) {
            (Some(u), _) => Some(u),
            (None, Some(_)) => None,
            (None, None) => page.url().await.ok().flatten(),
        };

        let mut param = CookieParam::new(name.clone(), value);
        param.url = url;
        param.domain = domain;
        param.path = path;
        param.secure = secure;
        param.http_only = http_only;
        page.set_cookie(param)
            .await
            .map_err(|e| StepError::msg(format!("browser.set_cookie: {e}")))?;
        Ok(ActionResult::from(
            serde_json::json!({ "ok": true, "name": name }),
        ))
    }
}

// ─── browser.tabs / browser.tab (F-10 part 2) ──────────────────────────────────
// chromiumoxide's `Browser` already tracks every open target — including tabs the
// page spawns itself (`window.open`, `target=_blank`) — so tab support reads
// `browser.pages()` rather than maintaining a parallel list. The session's `page`
// pointer remains the "active tab" every other action drives; `activate` simply
// repoints it. Tabs are addressed by Chrome's stable `target_id` or a URL
// substring — never by position, since `pages()` iterates a `HashMap` (unordered).

/// How `browser.tab` names the tab to act on. Exactly one form is accepted.
enum TabBy {
    Id(String),
    Url(String),
}

/// All currently open pages for the run's browser session.
async fn list_pages(s: &Session) -> Result<Vec<Page>, StepError> {
    let browser = s.browser.lock().await;
    browser
        .pages()
        .await
        .map_err(|e| StepError::msg(format!("browser tabs: {e}")))
}

/// Find the page matching `by` among `pages`, consuming the list and returning
/// the matched page so the caller can `activate`/`close` it.
async fn resolve_tab(pages: Vec<Page>, by: &TabBy) -> Result<Page, StepError> {
    match by {
        TabBy::Id(id) => pages
            .into_iter()
            .find(|p| p.target_id().as_ref() == id.as_str())
            .ok_or_else(|| {
                StepError::msg(format!("browser.tab: no open tab with target_id `{id}`"))
            }),
        TabBy::Url(sub) => {
            for p in pages {
                if let Ok(Some(u)) = p.url().await {
                    if u.contains(sub.as_str()) {
                        return Ok(p);
                    }
                }
            }
            Err(StepError::msg(format!(
                "browser.tab: no open tab whose URL contains `{sub}`"
            )))
        }
    }
}

pub struct TabsAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TabsIn {}

#[async_trait]
impl Action for TabsAction {
    fn id(&self) -> &'static str {
        "browser.tabs"
    }
    fn summary(&self) -> &'static str {
        "List open browser tabs (target_id, url, title, and which is active)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<TabsIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TabsIn {} = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.tabs input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let pages = list_pages(&s).await?;
        // The active tab is the one the session pointer currently targets.
        let active_id = s
            .page
            .lock()
            .as_ref()
            .map(|p| p.target_id().as_ref().to_string());
        let mut out = Vec::with_capacity(pages.len());
        for p in &pages {
            let url = p.url().await.ok().flatten().unwrap_or_default();
            let title = p.get_title().await.ok().flatten().unwrap_or_default();
            let id = p.target_id().as_ref().to_string();
            let active = active_id.as_deref() == Some(id.as_str());
            out.push(serde_json::json!({
                "target_id": id,
                "url": url,
                "title": title,
                "active": active,
            }));
        }
        Ok(ActionResult::from(Value::Array(out)))
    }
}

/// `browser.tab` operations. A derived enum so the schema carries the
/// `["activate","close"]` constraint (F-23), like `browser.wait`'s condition.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum TabOp {
    /// Make the matched tab active: bring it to front; later actions target it.
    Activate,
    /// Close the matched tab; if it was active, activate another open tab (if any).
    Close,
}

pub struct TabAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TabIn {
    op: TabOp,
    /// Match the tab whose Chrome target id equals this (from `browser.tabs`).
    #[serde(default)]
    target_id: Option<String>,
    /// Match the first tab whose URL contains this substring.
    #[serde(default)]
    url_includes: Option<String>,
}

#[async_trait]
impl Action for TabAction {
    fn id(&self) -> &'static str {
        "browser.tab"
    }
    fn summary(&self) -> &'static str {
        "Activate or close a browser tab, addressed by target_id or url_includes"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<TabIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TabIn {
            op,
            target_id,
            url_includes,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.tab input invalid: {e}")))?;
        // Resolve the addressing BEFORE any session work, so a malformed call
        // fails fast (and is CI-testable) without ever launching Chrome.
        let by = match (target_id, url_includes) {
            (Some(id), None) => TabBy::Id(id),
            (None, Some(url)) => TabBy::Url(url),
            (None, None) => {
                return Err(StepError::msg(
                    "browser.tab requires `target_id` or `url_includes`",
                ))
            }
            (Some(_), Some(_)) => {
                return Err(StepError::msg(
                    "browser.tab: set only one of `target_id` or `url_includes`",
                ))
            }
        };
        let s = session_for_ctx(ctx)?;
        let matched = resolve_tab(list_pages(&s).await?, &by).await?;
        let id = matched.target_id().as_ref().to_string();
        match op {
            TabOp::Activate => {
                matched
                    .bring_to_front()
                    .await
                    .map_err(|e| StepError::msg(format!("browser.tab activate `{id}`: {e}")))?;
                *s.page.lock() = Some(matched);
                Ok(ActionResult::from(serde_json::json!({ "activated": id })))
            }
            TabOp::Close => {
                // Note whether we're closing the active tab before consuming it.
                let was_active = {
                    let g = s.page.lock();
                    g.as_ref().map(|p| p.target_id().as_ref() == id.as_str()) == Some(true)
                };
                matched
                    .close()
                    .await
                    .map_err(|e| StepError::msg(format!("browser.tab close `{id}`: {e}")))?;
                if was_active {
                    // Repoint the active pointer to another open tab, if any remain.
                    let next = list_pages(&s)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .find(|p| p.target_id().as_ref() != id.as_str());
                    if let Some(p) = &next {
                        let _ = p.bring_to_front().await;
                    }
                    *s.page.lock() = next;
                }
                Ok(ActionResult::from(serde_json::json!({ "closed": id })))
            }
        }
    }
}

// ─── browser.upload (F-10 part 2) ──────────────────────────────────────────────
// `<input type=file>` can't be driven by typing — its value is set out-of-band via
// CDP `DOM.setFileInputFiles` against the input's backend node. chromiumoxide ships
// no high-level wrapper, so we resolve the input with the normal selector engine
// (whose `Element` exposes `backend_node_id`) and issue the raw command. Every local
// path is gated by fs-read BEFORE the session, so an ungranted file fails fast
// (capability error, no Chrome) — the same ordering as `browser.screenshot`.

pub struct UploadAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UploadIn {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// Local file path(s) to attach to the file input. Each is gated by fs-read.
    files: Vec<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for UploadAction {
    fn id(&self) -> &'static str {
        "browser.upload"
    }
    fn summary(&self) -> &'static str {
        "Set the file(s) on an <input type=file>, addressed by a selector spec"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<UploadIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let UploadIn {
            selector,
            selectors,
            files,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.upload input invalid: {e}")))?;
        if files.is_empty() {
            return Err(StepError::msg(
                "browser.upload requires at least one path in `files`",
            ));
        }
        let spec = build_selector(selector, selectors)?;
        // Gate every path on fs-read BEFORE touching Chrome — an ungranted file
        // fails fast with a capability error, same ordering as browser.screenshot.
        for f in &files {
            ctx.ensure_fs_read(std::path::Path::new(f))?;
        }
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let hint = spec.first_hint();
        let (element, strategy) = resolve_element(&page, &spec, timeout_ms).await?;
        let file_count = files.len();
        page.execute(
            SetFileInputFilesParams::builder()
                .files(files)
                .backend_node_id(element.backend_node_id)
                .build()
                .map_err(|e| StepError::msg(format!("browser.upload: {e}")))?,
        )
        .await
        .map_err(|e| StepError::msg(format!("browser.upload `{hint}`: {e}")))?;
        clear_marker(&page).await;
        Ok(ActionResult::from(serde_json::json!({
            "resolved_by": strategy,
            "matched": hint,
            "files": file_count,
        })))
    }
}

// ─── browser.download_wait (批次B) ──────────────────────────────────────────────
// Point Chrome's downloads at a gated directory (CDP Browser.setDownloadBehavior),
// optionally trigger the download by clicking a selector, then poll the directory
// until a file finishes (no trailing `.crdownload`/`.tmp`). The download dir is
// gated by fs-write BEFORE any session — an ungranted dir fails fast (capability
// error, no Chrome) — same ordering as browser.screenshot/upload.

pub struct DownloadWaitAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DownloadWaitIn {
    /// Directory Chrome saves the download into (gated by fs-write).
    dir: String,
    /// Optional selector to click to start the download. When absent, the caller
    /// is expected to have already triggered an in-flight download.
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    /// How long to wait for a completed file to appear.
    #[serde(default = "default_download_timeout_ms")]
    timeout_ms: u64,
}
fn default_download_timeout_ms() -> u64 {
    60_000
}

/// Chrome marks an in-progress download with one of these suffixes; a file
/// without them is considered complete.
fn is_partial(name: &str) -> bool {
    name.ends_with(".crdownload") || name.ends_with(".tmp") || name.ends_with(".part")
}

/// File names (not dirs) directly under `dir`. Empty on any IO error.
async fn list_dir_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false)
            {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    out
}

#[async_trait]
impl Action for DownloadWaitAction {
    fn id(&self) -> &'static str {
        "browser.download_wait"
    }
    fn summary(&self) -> &'static str {
        "Route downloads to a gated dir, optionally click to start, wait for the file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<DownloadWaitIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DownloadWaitIn {
            dir,
            selector,
            selectors,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.download_wait input invalid: {e}")))?;
        // Gate the download dir BEFORE touching Chrome — an ungranted dir fails
        // fast (capability error, no session), same order as browser.screenshot.
        let dir_path = PathBuf::from(&dir);
        ctx.ensure_fs_write(&dir_path)?;
        let _ = tokio::fs::create_dir_all(&dir_path).await;

        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        // Snapshot existing files so we only report a file that appears AFTER we
        // start (avoids returning a stale pre-existing download in the dir).
        let before = list_dir_files(&dir_path).await;

        // Route downloads to the gated dir for this page's target.
        let abs = std::fs::canonicalize(&dir_path).unwrap_or_else(|_| dir_path.clone());
        page.execute(
            SetDownloadBehaviorParams::builder()
                .behavior(SetDownloadBehaviorBehavior::Allow)
                .download_path(abs.to_string_lossy().to_string())
                .build()
                .map_err(|e| StepError::msg(format!("browser.download_wait set behavior: {e}")))?,
        )
        .await
        .map_err(|e| StepError::msg(format!("browser.download_wait set behavior: {e}")))?;

        // Optionally click the trigger.
        if selector.as_ref().is_some_and(|s| !s.is_empty())
            || selectors.as_ref().is_some_and(|s| !s.is_empty())
        {
            let spec = build_selector(selector, selectors)?;
            let (element, _strategy) = resolve_element(&page, &spec, timeout_ms).await?;
            element
                .click()
                .await
                .map_err(|e| StepError::msg(format!("browser.download_wait click: {e}")))?;
            clear_marker(&page).await;
        }

        // Poll for a new, complete file.
        let deadline = Duration::from_millis(timeout_ms);
        let start = std::time::Instant::now();
        loop {
            let now = list_dir_files(&dir_path).await;
            let candidate = now
                .iter()
                .find(|f| !before.contains(*f) && !is_partial(f))
                .cloned();
            if let Some(name) = candidate {
                let path = dir_path.join(&name);
                let bytes = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                return Ok(ActionResult::from(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "name": name,
                    "bytes": bytes,
                    "waited_ms": start.elapsed().as_millis() as u64,
                })));
            }
            if start.elapsed() >= deadline {
                return Err(StepError::msg(format!(
                    "browser.download_wait: no completed download appeared in `{dir}` within {timeout_ms}ms"
                )));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

// ─── browser.dialog (批次B) ──────────────────────────────────────────────────────
// Pre-arm a handler for the NEXT JS dialog (alert/confirm/prompt). Subscribe to
// Page.javascriptDialogOpening, optionally click a trigger that raises the dialog,
// then answer it with Page.handleJavaScriptDialog (accept/dismiss + promptText).

pub struct DialogAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DialogIn {
    /// Accept (true) or dismiss (false) the dialog.
    #[serde(default = "default_true")]
    accept: bool,
    /// Text to type into a prompt() before accepting.
    #[serde(default)]
    prompt_text: Option<String>,
    /// Optional selector to click to raise the dialog after subscribing.
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    selectors: Option<MultiSelector>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for DialogAction {
    fn id(&self) -> &'static str {
        "browser.dialog"
    }
    fn summary(&self) -> &'static str {
        "Handle the next JS dialog (alert/confirm/prompt): accept or dismiss"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<DialogIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DialogIn {
            accept,
            prompt_text,
            selector,
            selectors,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.dialog input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        // Subscribe BEFORE triggering so we can't miss a fast dialog.
        let mut events = page
            .event_listener::<EventJavascriptDialogOpening>()
            .await
            .map_err(|e| StepError::msg(format!("browser.dialog subscribe: {e}")))?;

        // Optionally click a trigger that raises the dialog.
        if selector.as_ref().is_some_and(|s| !s.is_empty())
            || selectors.as_ref().is_some_and(|s| !s.is_empty())
        {
            let spec = build_selector(selector, selectors)?;
            let (element, _strategy) = resolve_element(&page, &spec, timeout_ms).await?;
            // A click that opens an alert can block the click promise, so don't
            // hard-fail on its result — the dialog event is the real signal.
            let _ = element.click().await;
            clear_marker(&page).await;
        }

        // Wait for the dialog to open (bounded by timeout_ms).
        let opened = tokio::time::timeout(Duration::from_millis(timeout_ms), events.next())
            .await
            .map_err(|_| StepError::msg("browser.dialog: no dialog opened within timeout"))?
            .ok_or_else(|| StepError::msg("browser.dialog: dialog event stream closed"))?;

        let mut params = HandleJavaScriptDialogParams::new(accept);
        params.prompt_text = prompt_text;
        page.execute(params)
            .await
            .map_err(|e| StepError::msg(format!("browser.dialog handle: {e}")))?;

        Ok(ActionResult::from(serde_json::json!({
            "accepted": accept,
            "type": opened.r#type.as_ref(),
            "message": opened.message,
        })))
    }
}

// ─── browser.frame (批次B) ──────────────────────────────────────────────────────
// First-class iframe switch for eval/extract. chromiumoxide 0.7 can't drive the
// DOM selector engine *inside* a frame (click/type stay main-frame — see the
// iframe-scoped eval note above), so this exposes the supported frame ops: run a
// JS expression, or extract a frame element's innerText/attribute. Address the
// frame by url_includes / name / index.

pub struct FrameAction;

/// What to do inside the matched frame.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum FrameOp {
    /// Evaluate `expr` in the frame, returning its JSON value.
    Eval,
    /// Read `selector`'s innerText (or `attr`) from inside the frame.
    Extract,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FrameIn {
    op: FrameOp,
    // Frame address — exactly one of these (url_includes wins, then name, index).
    // Inlined (not a flattened FrameSel) so `deny_unknown_fields` stays effective
    // and the derived schema is flat/closed.
    #[serde(default)]
    url_includes: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    index: Option<usize>,
    /// For `op: eval` — the JS expression.
    #[serde(default)]
    expr: Option<String>,
    /// For `op: extract` — the CSS selector inside the frame.
    #[serde(default)]
    selector: Option<String>,
    /// For `op: extract` — read this attribute instead of innerText.
    #[serde(default)]
    attr: Option<String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for FrameAction {
    fn id(&self) -> &'static str {
        "browser.frame"
    }
    fn summary(&self) -> &'static str {
        "Eval or extract inside an iframe addressed by url_includes/name/index"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<FrameIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let FrameIn {
            op,
            url_includes,
            name,
            index,
            expr,
            selector,
            attr,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.frame input invalid: {e}")))?;
        let frame = FrameSel {
            url_includes,
            name,
            index,
        };
        // Validate the frame address up front (CI-testable without Chrome).
        if frame.is_empty() {
            return Err(StepError::msg(
                "browser.frame: `frame` requires `url_includes`, `name`, or `index`",
            ));
        }
        // Build the JS for the chosen op (also validates the op's required field
        // before any session work).
        let js = match op {
            FrameOp::Eval => expr
                .filter(|e| !e.is_empty())
                .ok_or_else(|| StepError::msg("browser.frame: `op: eval` requires `expr`"))?,
            FrameOp::Extract => {
                let sel = selector.filter(|s| !s.is_empty()).ok_or_else(|| {
                    StepError::msg("browser.frame: `op: extract` requires `selector`")
                })?;
                let sel_json = serde_json::to_string(&sel).unwrap();
                let attr_json = serde_json::to_string(&attr).unwrap_or_else(|_| "null".into());
                format!(
                    r#"
(() => {{
  const el = document.querySelector({sel_json});
  if (!el) return null;
  const a = {attr_json};
  return a ? el.getAttribute(a) : el.innerText;
}})()
"#
                )
            }
        };
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;
        let result = eval_in_frame(&page, js, &frame, timeout_ms)
            .await
            .map_err(|e| StepError::msg(format!("browser.frame: {e}")))?;
        Ok(ActionResult::from(result))
    }
}

// ─── browser.extract_table (批次B) ──────────────────────────────────────────────
// Extract an HTML <table> into an array of header-keyed row objects via an eval
// wrapper. Picks the table by `selector`; uses the row at `header_row` (default 0)
// as the header, then maps each other row's cells onto those keys.

pub struct ExtractTableAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExtractTableIn {
    /// CSS selector for the <table> (or a container holding one).
    selector: String,
    /// Zero-based index of the row to use as the header. Default 0.
    #[serde(default)]
    header_row: usize,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ExtractTableAction {
    fn id(&self) -> &'static str {
        "browser.extract_table"
    }
    fn summary(&self) -> &'static str {
        "Extract an HTML <table> into an array of header-keyed row objects"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ExtractTableIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtractTableIn {
            selector,
            header_row,
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.extract_table input invalid: {e}")))?;
        let s = session_for_ctx(ctx)?;
        let page = current_page(&s)?;

        let sel_json = serde_json::to_string(&selector).unwrap();
        let js = format!(
            r#"
(() => {{
  const root = document.querySelector({sel_json});
  if (!root) return null;
  const table = root.tagName === 'TABLE' ? root : root.querySelector('table');
  if (!table) return null;
  const rows = Array.from(table.rows);
  if (rows.length === 0) return [];
  const headerIdx = {header_row};
  if (headerIdx >= rows.length) return [];
  const headers = Array.from(rows[headerIdx].cells).map((c, i) => (c.innerText || '').trim() || ('col' + i));
  const out = [];
  for (let r = 0; r < rows.length; r++) {{
    if (r === headerIdx) continue;
    const cells = Array.from(rows[r].cells);
    const obj = {{}};
    for (let i = 0; i < headers.length; i++) {{
      obj[headers[i]] = cells[i] ? (cells[i].innerText || '').trim() : null;
    }}
    out.push(obj);
  }}
  return out;
}})()
"#
        );
        let eval = tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js)).await;
        let result: Value = match eval {
            Err(_) => {
                return Err(StepError::ExtractFailed(format!(
                    "timeout extracting table `{selector}`"
                )))
            }
            Ok(Err(e)) => {
                return Err(StepError::ExtractFailed(format!(
                    "extract_table `{selector}`: {e}"
                )))
            }
            Ok(Ok(v)) => v.into_value().unwrap_or(Value::Null),
        };
        if result.is_null() {
            return Err(StepError::ExtractFailed(format!(
                "browser.extract_table: no <table> found at `{selector}`"
            )));
        }
        Ok(ActionResult::from(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn decl(yaml: &str) -> ResourceDecl {
        serde_yaml::from_str(yaml).expect("valid ResourceDecl yaml")
    }

    /// A ctx carrying the given `spec.resources` (name → YAML decl) and an
    /// optional current-step `resource:` binding — the two inputs the T3 slot /
    /// launch-opt resolution reads.
    fn ctx_with(resources: &[(&str, &str)], current: Option<&str>) -> StepCtx {
        let map: BTreeMap<String, ResourceDecl> = resources
            .iter()
            .map(|(name, yaml)| (name.to_string(), decl(yaml)))
            .collect();
        let ctx = StepCtx::new(
            "run-1".into(),
            "flow-1".into(),
            ActionRegistry::new(),
            None,
            Value::Null,
            lumo_dsl::Capabilities::default(),
            Vec::new(),
        )
        .with_resources(map);
        ctx.set_current_resource(current);
        ctx
    }

    #[test]
    fn browser_slot_uses_resource_name_only_for_browser_kind() {
        let resources = &[("browser", "kind: chromium.cdp\n"), ("db", "kind: sqlite\n")];
        // Bound to a chromium.cdp resource ⇒ slot is the resource name.
        assert_eq!(browser_slot(&ctx_with(resources, Some("browser"))), "browser");
        // Unbound ⇒ default slot (run-scoped, back-compat with the old model).
        assert_eq!(browser_slot(&ctx_with(resources, None)), DEFAULT_SLOT);
        // Bound to a NON-browser kind ⇒ default slot (never key a browser by an
        // unrelated db resource's name).
        assert_eq!(browser_slot(&ctx_with(resources, Some("db"))), DEFAULT_SLOT);
        // Bound to an undeclared name ⇒ default slot.
        assert_eq!(browser_slot(&ctx_with(resources, Some("ghost"))), DEFAULT_SLOT);
    }

    #[test]
    fn launch_opts_from_decl_reads_headless_and_profile() {
        // `headless` honored from the flattened config; `profile` from the sub-field.
        let o = launch_opts_from_decl(&decl(
            "kind: chromium.cdp\nprofile: stealth\nheadless: false\n",
        ));
        assert!(!o.headless);
        assert_eq!(o.profile.as_deref(), Some("stealth"));
        // `headless` defaults to true when omitted (matches per-step with.headless).
        let d = launch_opts_from_decl(&decl("kind: chromium.cdp\n"));
        assert!(d.headless);
        assert_eq!(d.profile, None);
    }

    #[test]
    fn profile_user_data_dir_sanitizes_to_one_safe_component_under_the_root() {
        // A normal name is the leaf; the parent is the browser-profiles root.
        let p = profile_user_data_dir("stealth-default");
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("stealth-default"));
        assert_eq!(
            p.parent().and_then(|d| d.file_name()).and_then(|s| s.to_str()),
            Some("browser-profiles"),
        );
        // Separators / spaces / dots collapse to `_`, so a crafted name can never
        // add a path component or `..`-escape the root — it stays a single leaf.
        let evil = profile_user_data_dir("../../etc/passwd");
        assert_eq!(
            evil.file_name().and_then(|s| s.to_str()),
            Some("______etc_passwd"),
        );
        assert_eq!(
            evil.parent().and_then(|d| d.file_name()).and_then(|s| s.to_str()),
            Some("browser-profiles"),
            "sanitized name must remain directly under the root (no escape)",
        );
        // An empty name falls back to `default` rather than the empty root. (A
        // non-empty name always yields a non-empty leaf — every char maps to itself
        // or `_` — so `default` is reached only for `""`.)
        assert_eq!(
            profile_user_data_dir("").file_name().and_then(|s| s.to_str()),
            Some("default"),
        );
    }

    #[test]
    fn build_browser_config_wires_user_data_dir_only_when_a_profile_is_set() {
        // No profile ⇒ ephemeral: no persistent user-data-dir (pre-T3 behavior).
        let ephemeral = build_browser_config(&LaunchOpts {
            headless: true,
            profile: None,
        })
        .expect("config builds");
        assert!(
            ephemeral.user_data_dir.is_none(),
            "an unbound/profile-less launch must stay ephemeral",
        );
        // A profile ⇒ persistent: the per-name user-data-dir is wired onto the
        // config. (The stealth `--disable-blink-features=…` args are applied in the
        // same branch but `BrowserConfig.args` is private, so we assert the
        // observable persistence wiring here; arg application is covered by review.)
        let persistent = build_browser_config(&LaunchOpts {
            headless: true,
            profile: Some("t3-profile".into()),
        })
        .expect("config builds");
        let dir = persistent
            .user_data_dir
            .expect("a profile must set a persistent user-data-dir");
        assert_eq!(dir.file_name().and_then(|s| s.to_str()), Some("t3-profile"));
    }

    #[test]
    fn resolve_open_target_prefers_decl_over_step_fallback() {
        let resources = &[("browser", "kind: chromium.cdp\nheadless: false\n")];
        // Bound: slot = name, opts from the decl (headless:false) — NOT the
        // step's own fallback (headless:true).
        let (slot, opts) = resolve_open_target(
            &ctx_with(resources, Some("browser")),
            LaunchOpts {
                headless: true,
                profile: None,
            },
        );
        assert_eq!(slot, "browser");
        assert!(!opts.headless, "decl opts must win over the step's with.headless");

        // Unbound: default slot, the step's fallback opts preserved unchanged.
        let (slot, opts) = resolve_open_target(
            &ctx_with(resources, None),
            LaunchOpts {
                headless: false,
                profile: None,
            },
        );
        assert_eq!(slot, DEFAULT_SLOT);
        assert!(!opts.headless, "unbound step keeps its own with.headless");
    }

    #[test]
    fn browser_factory_kind_is_chromium_cdp() {
        assert_eq!(BrowserFactory.kind(), BROWSER_KIND);
        assert_eq!(BROWSER_KIND, "chromium.cdp");
    }

    // Map-bookkeeping invariants the teardown paths depend on, exercised over a
    // trivial value type so they run in CI without launching Chrome.

    #[test]
    fn slot_bookkeeping_isolates_slots_and_drops_empty_run_bucket() {
        let mut map: HashMap<String, HashMap<String, i32>> = HashMap::new();
        // Two independent slots within one run.
        assert_eq!(put_slot(&mut map, "run", "a", 1), None);
        assert_eq!(put_slot(&mut map, "run", "b", 2), None);
        // A same-slot overwrite returns the previous value (the loser to reap).
        assert_eq!(put_slot(&mut map, "run", "a", 11), Some(1));
        // Removing one slot leaves the sibling and keeps the run bucket alive.
        assert_eq!(take_slot(&mut map, "run", "a"), Some(11));
        assert!(map.get("run").is_some_and(|r| r.contains_key("b")));
        // Removing the last slot drops the whole run bucket.
        assert_eq!(take_slot(&mut map, "run", "b"), Some(2));
        assert!(
            map.get("run").is_none(),
            "empty run bucket must be removed so session_exists/teardown stay accurate"
        );
        // Unknown run / slot are clean Nones (idempotent close).
        assert_eq!(take_slot(&mut map, "run", "gone"), None);
        assert_eq!(take_slot(&mut map, "missing", "x"), None);
    }

    #[test]
    fn take_run_drains_all_slots_and_leaves_other_runs() {
        let mut map: HashMap<String, HashMap<String, i32>> = HashMap::new();
        put_slot(&mut map, "run", "a", 1);
        put_slot(&mut map, "run", "b", 2);
        put_slot(&mut map, "other", "a", 9);
        let drained = take_run(&mut map, "run").expect("run present");
        assert_eq!(drained.len(), 2, "end-of-run teardown reaps every slot");
        assert!(map.get("run").is_none());
        assert!(map.get("other").is_some(), "a different run is untouched");
        assert!(
            take_run(&mut map, "run").is_none(),
            "second drain of the same run is a clean None"
        );
    }
}
