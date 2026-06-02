//! Browser automation actions over the Chrome DevTools Protocol via
//! `chromiumoxide`. M1 implements the minimal surface needed to drive a
//! login → click → extract flow; the multi-strategy selector engine
//! (CSS / XPath / A11y / Vision) lands in M2.

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::network::CookieParam;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, RunTeardown, StepCtx};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
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
    // F-10 browser action completions.
    r.register(EvalAction);
    r.register(ScreenshotAction);
    r.register(ScrollAction);
    r.register(HoverAction);
    r.register(SelectAction);
    r.register(CookiesAction);
    r.register(SetCookieAction);
    // P1-2: reclaim any browser process left open by a run that failed (or
    // forgot `browser.close`) once the VM finishes that run.
    r.register_teardown(Arc::new(BrowserTeardown));
}

// ─── Browser sessions ────────────────────────────────────────────────────────
// Sessions are keyed by flow run id, so repeated or concurrent runs don't share
// the same active page.

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

type SessionMap = Arc<Mutex<HashMap<String, Arc<Session>>>>;

static SESSIONS: once_cell::sync::OnceCell<SessionMap> = once_cell::sync::OnceCell::new();

fn sessions() -> SessionMap {
    SESSIONS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

async fn ensure_session(run_id: &str, headless: bool) -> Result<Arc<Session>, StepError> {
    {
        let lock = sessions();
        let g = lock.lock();
        if let Some(s) = g.get(run_id).cloned() {
            return Ok(s);
        }
    }
    let cfg = if headless {
        BrowserConfig::builder().build()
    } else {
        BrowserConfig::builder().with_head().build()
    }
    .map_err(|e| StepError::msg(format!("chrome cfg: {e}")))?;

    let (browser, mut handler) = Browser::launch(cfg)
        .await
        .map_err(|e| StepError::msg(format!("chrome launch: {e}")))?;
    let handle = tokio::spawn(async move { while let Some(_evt) = handler.next().await {} });
    let session = Arc::new(Session {
        browser: tokio::sync::Mutex::new(browser),
        handler: Mutex::new(Some(handle)),
        page: Mutex::new(None),
    });
    {
        let lock = sessions();
        lock.lock().insert(run_id.to_string(), session.clone());
    }
    Ok(session)
}

fn session_for_run(run_id: &str) -> Result<Arc<Session>, StepError> {
    let lock = sessions();
    let session = lock.lock().get(run_id).cloned();
    session.ok_or_else(|| StepError::msg("browser not launched"))
}

fn current_page(s: &Session) -> Result<Page, StepError> {
    s.page
        .lock()
        .clone()
        .ok_or_else(|| StepError::msg("no browser page open; call `browser.open` first"))
}

// ─── session teardown (P1-2) ──────────────────────────────────────────────────

/// Force-close and reap the browser session for `run_id`, aborting its CDP
/// event-pump task. Idempotent — a no-op when no session exists. Called both by
/// the explicit `browser.close` action and by the end-of-run teardown hook, so
/// a flow that fails (or omits `browser.close`) can't orphan a headless Chrome.
pub async fn close_run_sessions(run_id: &str) {
    let removed = sessions().lock().remove(run_id);
    if let Some(s) = removed {
        teardown_session(s).await;
    }
}

/// Whether a browser session is currently registered for `run_id`. Exposed for
/// tests; not part of the action surface.
#[doc(hidden)]
pub fn session_exists(run_id: &str) -> bool {
    sessions().lock().contains_key(run_id)
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
        let _ = ensure_session(ctx.run_id(), headless).await?;
        Ok(ActionResult::from(
            serde_json::json!({ "ok": true, "headless": headless }),
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
        // Force-close and reap the session so the Chrome process is terminated,
        // not just unlinked from the map (P1-2).
        close_run_sessions(ctx.run_id()).await;
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
        let s = ensure_session(ctx.run_id(), headless).await?;
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
        let s = session_for_run(_ctx.run_id())?;
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
        let s = session_for_run(_ctx.run_id())?;
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
            timeout_ms,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.extract input invalid: {e}")))?;
        let s = session_for_run(_ctx.run_id())?;
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

        let eval = tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js)).await;
        let result: Value = match eval {
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

        let s = session_for_run(ctx.run_id())?;
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

// ─── browser.eval (F-10) ──────────────────────────────────────────────────────

pub struct EvalAction;
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EvalIn {
    /// JavaScript evaluated in the page; its result is returned as JSON. Runs in
    /// the same page context as `browser.extract` — arbitrary script, no new
    /// capability (the flow already drives this browser).
    expr: String,
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
        let EvalIn { expr, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.eval input invalid: {e}")))?;
        let s = session_for_run(ctx.run_id())?;
        let page = current_page(&s)?;
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
        let s = session_for_run(ctx.run_id())?;
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
        let s = session_for_run(ctx.run_id())?;
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
        let s = session_for_run(ctx.run_id())?;
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
        let s = session_for_run(ctx.run_id())?;
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
                return Err(StepError::msg(format!("browser.select `{hint}`: timed out")));
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
        let s = session_for_run(ctx.run_id())?;
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
        let s = session_for_run(ctx.run_id())?;
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
