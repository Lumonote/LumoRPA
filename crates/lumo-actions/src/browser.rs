//! Browser automation actions over the Chrome DevTools Protocol via
//! `chromiumoxide`. M1 implements the minimal surface needed to drive a
//! login → click → extract flow; the multi-strategy selector engine
//! (CSS / XPath / A11y / Vision) lands in M2.

use async_trait::async_trait;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
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
}

// ─── Browser sessions ────────────────────────────────────────────────────────
// Sessions are keyed by flow run id, so repeated or concurrent runs don't share
// the same active page.

struct Session {
    browser: Browser,
    _handler: JoinHandle<()>,
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
        browser,
        _handler: handle,
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

// ─── browser.launch ─────────────────────────────────────────────────────────

pub struct LaunchAction;
#[derive(Deserialize, Default)]
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": { "headless": { "type": "boolean" } },
                "additionalProperties": false
            })
        });
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

#[async_trait]
impl Action for CloseAction {
    fn id(&self) -> &'static str {
        "browser.close"
    }
    fn summary(&self) -> &'static str {
        "Close the current browser session"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let lock = sessions();
        let mut g = lock.lock();
        if let Some(s) = g.remove(ctx.run_id()) {
            *s.page.lock() = None;
            drop(s);
        }
        Ok(ActionResult::null())
    }
}

// ─── browser.open ───────────────────────────────────────────────────────────

pub struct OpenAction;
#[derive(Deserialize)]
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

/// Inline schema fragment for the `selectors:` object used by browser actions.
/// Kept here so each action's static schema can reference it without
/// duplicating the property list.
fn multi_selector_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": { "type": "string" },
            "data_testid": { "type": "string" },
            "css": { "type": "string" },
            "aria_label": { "type": "string" },
            "text_includes": { "type": "string" },
            "xpath": { "type": "string" }
        }
    })
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": { "type": "string" },
                    "headless": { "type": "boolean" },
                    "timeout_ms": { "type": "integer" },
                    "wait_for": { "type": "string" }
                },
                "additionalProperties": false
            })
        });
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
        let page = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            s.browser.new_page(url.as_str()),
        )
        .await
        .map_err(|_| StepError::msg(format!("timeout opening {url}")))?
        .map_err(|e| StepError::msg(format!("new_page: {e}")))?;
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
#[derive(Deserialize)]
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string" },
                    "selectors": multi_selector_schema(),
                    "prompt": { "type": "string" },
                    "model": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
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
#[derive(Deserialize)]
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["text"],
                "properties": {
                    "selector": { "type": "string" },
                    "selectors": multi_selector_schema(),
                    "text": { "type": "string" },
                    "clear": { "type": "boolean" },
                    "prompt": { "type": "string" },
                    "model": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
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
#[derive(Deserialize)]
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
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            serde_json::json!({
                "type": "object",
                "required": ["selector"],
                "properties": {
                    "selector": { "type": "string" },
                    "map": { "type": "object" },
                    "attr": { "type": "string" },
                    "all": { "type": "boolean" },
                    "timeout_ms": { "type": "integer" }
                },
                "additionalProperties": false
            })
        });
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
