//! Browser automation actions over the Chrome DevTools Protocol via
//! `chromiumoxide`. M1 implements the minimal surface needed to drive a
//! login → click → extract flow; the multi-strategy selector engine
//! (CSS / XPath / A11y / Vision) lands in M2.

use async_trait::async_trait;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use lumo_core::error::StepError;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

pub fn register(r: &mut ActionRegistry) {
    r.register(LaunchAction);
    r.register(CloseAction);
    r.register(OpenAction);
    r.register(ClickAction);
    r.register(TypeAction);
    r.register(ExtractAction);
}

// ─── Browser session singleton (per-process) ────────────────────────────────
// We keep a *process-global* browser session for M1 — simple to reason about,
// and matches the "one Studio, one Chrome" mental model. In M3 we'll move
// this into StepCtx so each worker has its own session.

struct Session {
    browser: Browser,
    _handler: JoinHandle<()>,
    page: Mutex<Option<Page>>,
}

static SESSION: once_cell::sync::OnceCell<Arc<Mutex<Option<Arc<Session>>>>> =
    once_cell::sync::OnceCell::new();

fn slot() -> Arc<Mutex<Option<Arc<Session>>>> {
    SESSION.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}

async fn ensure_session(headless: bool) -> Result<Arc<Session>, StepError> {
    {
        let lock = slot();
        let g = lock.lock();
        if let Some(s) = g.clone() { return Ok(s); }
    }
    let cfg = if headless {
        BrowserConfig::builder().build()
    } else {
        BrowserConfig::builder().with_head().build()
    }
    .map_err(|e| StepError::msg(format!("chrome cfg: {e}")))?;

    let (browser, mut handler) = Browser::launch(cfg).await
        .map_err(|e| StepError::msg(format!("chrome launch: {e}")))?;
    let handle = tokio::spawn(async move {
        while let Some(_evt) = handler.next().await {}
    });
    let session = Arc::new(Session { browser, _handler: handle, page: Mutex::new(None) });
    {
        let lock = slot();
        *lock.lock() = Some(session.clone());
    }
    Ok(session)
}

fn current_page(s: &Session) -> Result<Page, StepError> {
    s.page.lock().clone()
        .ok_or_else(|| StepError::msg("no browser page open; call `browser.open` first"))
}

// ─── browser.launch ─────────────────────────────────────────────────────────

pub struct LaunchAction;
#[derive(Deserialize, Default)]
struct LaunchIn { #[serde(default = "default_true")] headless: bool }
fn default_true() -> bool { true }

#[async_trait]
impl Action for LaunchAction {
    fn id(&self) -> &'static str { "browser.launch" }
    fn summary(&self) -> &'static str { "Launch (or attach to) a Chromium browser session" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let LaunchIn { headless } = serde_json::from_value(input).unwrap_or_default();
        let _ = ensure_session(headless).await?;
        Ok(ActionResult::from(serde_json::json!({ "ok": true, "headless": headless })))
    }
}

// ─── browser.close ──────────────────────────────────────────────────────────

pub struct CloseAction;

#[async_trait]
impl Action for CloseAction {
    fn id(&self) -> &'static str { "browser.close" }
    fn summary(&self) -> &'static str { "Close the current browser session" }
    async fn execute(&self, _ctx: &mut StepCtx, _input: Value) -> Result<ActionResult, StepError> {
        let lock = slot();
        let mut g = lock.lock();
        if let Some(s) = g.take() {
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
}
fn default_timeout_ms() -> u64 { 30_000 }

#[async_trait]
impl Action for OpenAction {
    fn id(&self) -> &'static str { "browser.open" }
    fn summary(&self) -> &'static str { "Navigate to a URL (launching browser if needed)" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let OpenIn { url, headless, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.open input invalid: {e}")))?;
        let s = ensure_session(headless).await?;
        let page = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            s.browser.new_page(url.as_str()),
        ).await
            .map_err(|_| StepError::msg(format!("timeout opening {url}")))?
            .map_err(|e| StepError::msg(format!("new_page: {e}")))?;
        let _ = page.wait_for_navigation().await;
        *s.page.lock() = Some(page);
        Ok(ActionResult::from(serde_json::json!({ "url": url })))
    }
}

// ─── browser.click ──────────────────────────────────────────────────────────

pub struct ClickAction;
#[derive(Deserialize)]
struct ClickIn { selector: String, #[serde(default = "default_timeout_ms")] timeout_ms: u64 }

#[async_trait]
impl Action for ClickAction {
    fn id(&self) -> &'static str { "browser.click" }
    fn summary(&self) -> &'static str { "Click the first element matching a CSS selector" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ClickIn { selector, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.click input invalid: {e}")))?;
        let s = {
            let lock = slot();
            let g = lock.lock();
            g.clone().ok_or_else(|| StepError::msg("browser not launched"))?
        };
        let page = current_page(&s)?;
        let el = tokio::time::timeout(Duration::from_millis(timeout_ms), page.find_element(&selector)).await
            .map_err(|_| StepError::msg(format!("timeout finding `{selector}`")))?
            .map_err(|e| StepError::msg(format!("selector `{selector}`: {e}")))?;
        el.click().await
            .map_err(|e| StepError::msg(format!("click `{selector}`: {e}")))?;
        Ok(ActionResult::from(serde_json::json!({ "selector": selector })))
    }
}

// ─── browser.type ───────────────────────────────────────────────────────────

pub struct TypeAction;
#[derive(Deserialize)]
struct TypeIn {
    selector: String,
    text: String,
    #[serde(default)]
    clear: bool,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for TypeAction {
    fn id(&self) -> &'static str { "browser.type" }
    fn summary(&self) -> &'static str { "Type text into the first element matching a selector" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TypeIn { selector, text, clear, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.type input invalid: {e}")))?;
        let s = {
            let lock = slot();
            let g = lock.lock();
            g.clone().ok_or_else(|| StepError::msg("browser not launched"))?
        };
        let page = current_page(&s)?;
        let el = tokio::time::timeout(Duration::from_millis(timeout_ms), page.find_element(&selector)).await
            .map_err(|_| StepError::msg(format!("timeout finding `{selector}`")))?
            .map_err(|e| StepError::msg(format!("selector `{selector}`: {e}")))?;
        if clear {
            let _ = el.focus().await;
            let _ = page.evaluate(format!(
                "document.querySelector({:?}).value = ''",
                selector
            )).await;
        }
        el.click().await.ok();
        el.type_str(&text).await
            .map_err(|e| StepError::msg(format!("type: {e}")))?;
        Ok(ActionResult::from(serde_json::json!({ "selector": selector, "typed": text.len() })))
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
    all: bool,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for ExtractAction {
    fn id(&self) -> &'static str { "browser.extract" }
    fn summary(&self) -> &'static str { "Extract innerText (or a field map) from matching elements" }
    async fn execute(&self, _ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ExtractIn { selector, map, all, timeout_ms } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("browser.extract input invalid: {e}")))?;
        let s = {
            let lock = slot();
            let g = lock.lock();
            g.clone().ok_or_else(|| StepError::msg("browser not launched"))?
        };
        let page = current_page(&s)?;

        // Build a JS function returning the extracted JSON shape, then evaluate.
        let map_json = serde_json::to_string(&map.unwrap_or_default()).unwrap_or("{}".into());
        let js = format!(r#"
(() => {{
  const sel = {sel};
  const all = {all};
  const map = {map};
  const pick = (el) => {{
    if (!el) return null;
    if (Object.keys(map).length === 0) return el.innerText;
    const out = {{}};
    for (const [k, v] of Object.entries(map)) {{
      const sub = el.querySelector(v);
      out[k] = sub ? sub.innerText : null;
    }}
    return out;
  }};
  if (all) {{
    return Array.from(document.querySelectorAll(sel)).map(pick);
  }}
  return pick(document.querySelector(sel));
}})()
"#, sel = serde_json::to_string(&selector).unwrap(), all = all, map = map_json);

        let result: Value = tokio::time::timeout(Duration::from_millis(timeout_ms), page.evaluate(js))
            .await
            .map_err(|_| StepError::msg(format!("timeout extracting `{selector}`")))?
            .map_err(|e| StepError::msg(format!("extract eval: {e}")))?
            .into_value()
            .unwrap_or(Value::Null);
        Ok(ActionResult::from(result))
    }
}
