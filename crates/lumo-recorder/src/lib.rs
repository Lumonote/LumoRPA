//! LumoRPA recorder.
//!
//! M1 shipped a [`NoopRecorder`] placeholder. M2 widens the surface:
//! - A live `mpsc` channel lets callers stream events to the desktop WebView.
//! - [`BrowserRecorder`] launches a head-ful Chromium via `chromiumoxide`,
//!   installs a CDP `Runtime.addBinding` hook, injects a JS recorder script
//!   on every navigation, and forwards DOM-level click / input / change /
//!   keydown events along with `FrameNavigated` and a heartbeat tick.
//! - [`condense_events`] applies a 200 ms ActionBuffer that merges consecutive
//!   typing into a single `input` event per selector — the same debouncing
//!   the design doc spells out for R-08.
//! - [`events_to_yaml_patch`] turns a captured event log into a LumoFlow YAML
//!   fragment ready to splice into `spec.steps`.
//! - [`desktop::DesktopRecorder`] is the R-02 cross-app capture lane: it
//!   reads the OS accessibility tree (NSAccessibility on macOS, UIA hooks
//!   on Windows) on a 200 ms polling loop and emits `desktop.*` events
//!   into the same channel the browser recorder uses.
//!
//! F-22 hardens the browser lane in three ways:
//! - [`BrowserRecorder`] now tears its Chrome session down gracefully on
//!   `stop()` *and* on `Drop` — close the CDP connection, then reap the child
//!   process (`close` → fallback `kill` → `wait`) so a dropped recorder can't
//!   orphan a head-ful Chrome. This mirrors `lumo-actions`' `BrowserTeardown`.
//! - It can [`connect`](BrowserMode::Connect) to an *already-running* Chrome
//!   via its DevTools endpoint instead of launching its own; in connect mode
//!   teardown only disconnects — it never kills the user's browser.
//! - The captured-element → YAML path scores candidate selectors
//!   ([`selector_score`]) so the emitted `selectors:` block leads with the
//!   most stable fingerprint (id / data-* > stable class > positional path).

pub mod desktop;
pub mod selector_score;

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, EventFrameNavigated,
};
use chromiumoxide::cdp::js_protocol::runtime::{AddBindingParams, EventBindingCalled};
use chromiumoxide::{Browser, BrowserConfig};
use futures::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEvent {
    pub source: String,
    pub kind: String,
    pub at_ms: i64,
    pub payload: serde_json::Value,
}

impl RawEvent {
    pub fn new(source: &str, kind: &str, payload: serde_json::Value) -> Self {
        Self {
            source: source.into(),
            kind: kind.into(),
            at_ms: chrono::Utc::now().timestamp_millis(),
            payload,
        }
    }
}

/// Sender side of the live event channel. Callers pass this into
/// [`Recorder::start`] when they want events streamed back in real time.
pub type RawEventSender = mpsc::Sender<RawEvent>;

#[async_trait]
pub trait Recorder: Send + Sync {
    /// Start the recorder. If `live` is provided, events are forwarded as
    /// they happen (non-blocking — overflow drops the event silently).
    async fn start(&self, live: Option<RawEventSender>) -> anyhow::Result<()>;

    /// Stop the recorder and return everything captured since `start`.
    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>>;
}

type SharedBuffer = Arc<Mutex<Vec<RawEvent>>>;

pub(crate) fn push_event(buffer: &SharedBuffer, live: &Option<RawEventSender>, evt: RawEvent) {
    if let Some(tx) = live {
        // `try_send` drops on full channel; better than blocking the recorder loop.
        let _ = tx.try_send(evt.clone());
    }
    buffer.lock().push(evt);
}

// ─── NoopRecorder ────────────────────────────────────────────────────────────

pub struct NoopRecorder {
    buffer: SharedBuffer,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for NoopRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl NoopRecorder {
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            tasks: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Recorder for NoopRecorder {
    async fn start(&self, live: Option<RawEventSender>) -> anyhow::Result<()> {
        tracing::info!("recorder: noop start (heartbeat only)");
        self.buffer.lock().clear();
        let buffer = self.buffer.clone();
        let live_cloned = live;
        let task = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            let mut n = 0u64;
            tick.tick().await; // skip immediate fire
            loop {
                tick.tick().await;
                n += 1;
                push_event(
                    &buffer,
                    &live_cloned,
                    RawEvent::new(
                        "noop",
                        "heartbeat",
                        serde_json::json!({ "n": n, "msg": "NoopRecorder heartbeat" }),
                    ),
                );
            }
        });
        self.tasks.lock().push(task);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>> {
        for t in self.tasks.lock().drain(..) {
            t.abort();
        }
        Ok(std::mem::take(&mut *self.buffer.lock()))
    }
}

// ─── BrowserRecorder (chromiumoxide / CDP) ───────────────────────────────────

/// Name of the CDP binding exposed to every frame. Browser-side JS calls
/// `window.__lumoRpaEvent(JSON.stringify({kind, payload}))` to deliver one
/// event into the recorder.
const BINDING_NAME: &str = "__lumoRpaEvent";

/// JS recorder injected on every new document. Computes a "good enough" CSS
/// selector, a positional XPath, and a small a11y label for the target, then
/// pushes a structured event back through the CDP binding. The real
/// multi-strategy fingerprinting (CSS / XPath / A11y / Visual) lives in the
/// future selector engine — this is the input it consumes.
fn recorder_injected_script() -> String {
    format!(
        r##"
(() => {{
  if (window.__lumoRpaInjected) return;
  window.__lumoRpaInjected = true;
  const SEND = window.{binding};
  if (typeof SEND !== "function") return;

  const cssEscape = (s) => (window.CSS && CSS.escape ? CSS.escape(String(s)) : String(s).replace(/[^a-zA-Z0-9_-]/g, "\\$&"));

  const pathFor = (el) => {{
    if (!(el instanceof Element)) return null;
    if (el.id) return "#" + cssEscape(el.id);
    const dtid = el.getAttribute && el.getAttribute("data-testid");
    if (dtid) return `[data-testid="${{cssEscape(dtid)}}"]`;
    const parts = [];
    let node = el;
    let depth = 0;
    while (node && node.nodeType === 1 && depth < 5) {{
      let part = node.nodeName.toLowerCase();
      if (node.classList && node.classList.length) {{
        const cls = Array.from(node.classList).slice(0, 2).map(cssEscape).join(".");
        if (cls) part += "." + cls;
      }}
      const parent = node.parentNode;
      if (parent && parent.children) {{
        const sibs = Array.from(parent.children).filter((s) => s.nodeName === node.nodeName);
        if (sibs.length > 1) {{
          part += `:nth-of-type(${{sibs.indexOf(node) + 1}})`;
        }}
      }}
      parts.unshift(part);
      node = parent;
      depth += 1;
    }}
    return parts.join(" > ");
  }};

  const xpathFor = (el) => {{
    if (!(el instanceof Element)) return null;
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && node.nodeName.toLowerCase() !== "html") {{
      let idx = 1;
      let sibling = node.previousElementSibling;
      while (sibling) {{ if (sibling.nodeName === node.nodeName) idx += 1; sibling = sibling.previousElementSibling; }}
      parts.unshift(`${{node.nodeName.toLowerCase()}}[${{idx}}]`);
      node = node.parentNode;
    }}
    return "//" + parts.join("/");
  }};

  const labelOf = (el) => {{
    if (!el) return null;
    try {{
      const aria = el.getAttribute && el.getAttribute("aria-label");
      if (aria) return aria;
      const id = el.id;
      if (id) {{
        const lab = document.querySelector(`label[for="${{cssEscape(id)}}"]`);
        if (lab && lab.innerText) return lab.innerText.trim();
      }}
      const placeholder = el.getAttribute && el.getAttribute("placeholder");
      if (placeholder) return placeholder;
      const txt = el.innerText || el.value;
      return txt ? String(txt).trim().slice(0, 80) : null;
    }} catch (_) {{ return null; }}
  }};

  const fire = (kind, el, extra) => {{
    if (!el || el.nodeType !== 1) return;
    const payload = Object.assign({{
      selector: pathFor(el),
      xpath: xpathFor(el),
      tag: el.tagName ? el.tagName.toLowerCase() : null,
      label: labelOf(el),
      url: location.href,
    }}, extra || {{}});
    try {{ SEND(JSON.stringify({{ kind, payload }})); }} catch (_) {{ /* dropped */ }}
  }};

  // R-09: when the user alt-clicks an element, look for sibling nodes with
  // the same tag + dominant class set. If two-or-more matches exist we treat
  // it as a "grab similar" gesture (YingDao headliner) and ship a
  // generalized selector so the YAML patch becomes `browser.extract` with all=true.
  const generalizeSimilar = (el) => {{
    if (!el || el.nodeType !== 1) return null;
    const parent = el.parentNode;
    if (!parent || parent.nodeType !== 1) return null;
    const sameTag = Array.from(parent.children).filter((c) => c.tagName === el.tagName);
    if (sameTag.length < 2) return null;
    const myClasses = el.classList ? Array.from(el.classList) : [];
    const shared = myClasses.filter((cls) => {{
      const hits = sameTag.filter((s) => s.classList && s.classList.contains(cls)).length;
      return hits / sameTag.length >= 0.8;
    }}).slice(0, 2);
    const parentSel = pathFor(parent);
    const tag = el.tagName.toLowerCase();
    const clsTail = shared.length ? "." + shared.map(cssEscape).join(".") : "";
    const selector = parentSel ? `${{parentSel}} > ${{tag}}${{clsTail}}` : `${{tag}}${{clsTail}}`;
    const sample = sameTag.slice(0, 3).map((s) => ((s.innerText || s.textContent || "").trim().slice(0, 40)));
    return {{ selector, count: sameTag.length, sample }};
  }};

  document.addEventListener("click", (e) => {{
    if (e.altKey) {{
      const sim = generalizeSimilar(e.target);
      if (sim) {{
        e.preventDefault();
        e.stopPropagation();
        fire("similar_grab", e.target, {{
          generalized_selector: sim.selector,
          sibling_count: sim.count,
          sample_values: sim.sample,
        }});
        return;
      }}
    }}
    fire("click", e.target, {{ button: e.button }});
  }}, true);

  document.addEventListener("change", (e) => {{
    const t = e.target;
    if (!t) return;
    const tag = (t.tagName || "").toLowerCase();
    if (tag === "input" || tag === "textarea" || tag === "select") {{
      const value = t.type === "password" ? "(redacted)" : String((tag === "select" ? t.value : t.value) || "").slice(0, 256);
      fire("change", t, {{ value }});
    }}
  }}, true);

  const inputTimers = new WeakMap();
  document.addEventListener("input", (e) => {{
    const t = e.target;
    if (!t) return;
    const tag = (t.tagName || "").toLowerCase();
    if (tag === "select") return;
    if (t.type === "password") {{ fire("input", t, {{ value: "(redacted)" }}); return; }}
    // Coalesce IME / fast-typing in 100ms before crossing CDP.
    if (inputTimers.has(t)) clearTimeout(inputTimers.get(t));
    inputTimers.set(t, setTimeout(() => {{
      fire("input", t, {{ value: String(t.value || "").slice(0, 1024) }});
      inputTimers.delete(t);
    }}, 100));
  }}, true);

  document.addEventListener("keydown", (e) => {{
    if (["Enter", "Tab", "Escape"].includes(e.key)) {{
      fire("keydown", e.target || document.body, {{ key: e.key }});
    }}
  }}, true);

  try {{ SEND(JSON.stringify({{ kind: "binding_ready", payload: {{ url: location.href }} }})); }} catch (_) {{}}
}})();
"##,
        binding = BINDING_NAME
    )
}

#[derive(Debug, Deserialize)]
struct DomBindingPayload {
    kind: String,
    #[serde(default)]
    payload: serde_json::Value,
}

pub struct BrowserRecorder {
    buffer: SharedBuffer,
    /// How to obtain the Chrome handle: launch a fresh head-ful instance, or
    /// attach to an already-running one over its DevTools endpoint (F-22).
    mode: BrowserMode,
    /// The live session, present only while recording. Behind a sync mutex for
    /// cheap `is_some()` checks; the `Browser` inside is itself behind a tokio
    /// mutex so the async teardown can hold the guard across `.await`.
    session: Mutex<Option<BrowserSession>>,
}

/// Where a [`BrowserRecorder`] gets its Chrome from.
#[derive(Debug, Clone)]
pub enum BrowserMode {
    /// Launch a new head-ful Chromium and own its child process. Teardown
    /// closes the CDP connection then reaps the process.
    Launch,
    /// Attach to an already-running Chrome via its DevTools endpoint
    /// (`ws://…` or `http://127.0.0.1:9222`). Teardown only disconnects — the
    /// user's browser is left running.
    Connect { endpoint: String },
}

struct BrowserSession {
    /// Chrome handle behind a tokio mutex: the graceful close/reap path
    /// (`close`/`kill`/`wait`) needs `&mut Browser` and is async, so the guard
    /// is held across `.await` — a sync mutex can't do that safely. Wrapped in
    /// an `Arc` so `Drop` can move it onto a teardown task without `&mut self`.
    browser: Arc<tokio::sync::Mutex<Browser>>,
    /// The CDP event-pump and per-stream listener tasks. Aborted on teardown
    /// so they don't linger detached after the socket closes.
    tasks: Vec<JoinHandle<()>>,
    /// `true` when we attached to an existing Chrome (connect mode): teardown
    /// must only disconnect, never `kill` the user's browser.
    connected: bool,
}

impl Default for BrowserRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserRecorder {
    /// A recorder that launches its own head-ful Chromium (the default).
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            mode: BrowserMode::Launch,
            session: Mutex::new(None),
        }
    }

    /// A recorder that attaches to an already-running Chrome at `endpoint`
    /// (a DevTools `ws://…` URL, or an `http://host:port` address whose
    /// `json/version` is queried for the websocket). Teardown will disconnect
    /// but never kill that browser (F-22).
    pub fn connect_to(endpoint: impl Into<String>) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            mode: BrowserMode::Connect {
                endpoint: endpoint.into(),
            },
            session: Mutex::new(None),
        }
    }

    /// Build with an explicit [`BrowserMode`].
    pub fn with_mode(mode: BrowserMode) -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            mode,
            session: Mutex::new(None),
        }
    }

    /// Whether a session is currently live. Exposed for tests.
    #[doc(hidden)]
    pub fn is_running(&self) -> bool {
        self.session.lock().is_some()
    }
}

/// Gracefully tear a session down: abort its tasks, then close+reap (launch
/// mode) or just disconnect (connect mode). Idempotent and panic-free so it's
/// safe from both `stop()` and `Drop`.
async fn teardown_session(mut session: BrowserSession) {
    for t in session.tasks.drain(..) {
        t.abort();
    }
    let mut browser = session.browser.lock().await;
    if session.connected {
        // Connect mode: don't touch the user's browser. Closing the CDP
        // connection is enough — dropping the `Browser` after this releases
        // the socket. We deliberately skip `close()` (which would tell the
        // *remote* Chrome to quit) and `kill()`/`wait()` (no child to reap).
    } else {
        // Launch mode: graceful CDP `Browser.close`, fall back to `kill`, then
        // `wait` to reap the child so it can't linger as a zombie.
        if browser.close().await.is_err() {
            let _ = browser.kill().await;
        }
        let _ = browser.wait().await;
    }
}

impl Drop for BrowserRecorder {
    fn drop(&mut self) {
        let Some(session) = self.session.lock().take() else {
            return;
        };
        // Abort the pump tasks synchronously so nothing keeps polling the
        // (about-to-close) socket while we hand teardown to the runtime.
        for t in &session.tasks {
            t.abort();
        }
        // The async close/reap needs a runtime. If we're inside one, spawn a
        // detached teardown task; otherwise (e.g. dropped on a plain thread)
        // best-effort the reap on a throwaway current-thread runtime so a
        // launched Chrome still gets cleaned up instead of orphaned.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(teardown_session(session));
            }
            Err(_) => {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(teardown_session(session));
                }
            }
        }
    }
}

#[async_trait]
impl Recorder for BrowserRecorder {
    async fn start(&self, live: Option<RawEventSender>) -> anyhow::Result<()> {
        self.buffer.lock().clear();
        if self.session.lock().is_some() {
            return Err(anyhow::anyhow!("BrowserRecorder already running"));
        }

        // Launch a fresh Chrome, or attach to an existing one (F-22).
        let (browser, mut handler, connected) = match &self.mode {
            BrowserMode::Launch => {
                let cfg = BrowserConfig::builder()
                    .with_head()
                    .build()
                    .map_err(|e| anyhow::anyhow!("chrome config: {e}"))?;
                let (b, h) = Browser::launch(cfg)
                    .await
                    .map_err(|e| anyhow::anyhow!("chrome launch: {e} (is Chromium installed?)"))?;
                (b, h, false)
            }
            BrowserMode::Connect { endpoint } => {
                let (b, h) = Browser::connect(endpoint.clone()).await.map_err(|e| {
                    anyhow::anyhow!("chrome connect to `{endpoint}`: {e} (is it listening with --remote-debugging-port?)")
                })?;
                (b, h, true)
            }
        };

        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        // Driver: pump the chromiumoxide event handler so the browser stays alive.
        tasks.push(tokio::spawn(async move {
            while let Some(_evt) = handler.next().await {}
        }));

        // Open a starter page so the user immediately sees the recorder is alive.
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| anyhow::anyhow!("new_page: {e}"))?;

        // CDP binding: window.__lumoRpaEvent becomes a callable in every frame.
        page.execute(AddBindingParams::new(BINDING_NAME))
            .await
            .map_err(|e| anyhow::anyhow!("addBinding: {e}"))?;
        // Inject the recorder script on every new document (handles SPA + navigations).
        page.execute(AddScriptToEvaluateOnNewDocumentParams::new(
            recorder_injected_script(),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("addScriptToEvaluateOnNewDocument: {e}"))?;
        // Also eval into the current about:blank so the user can drive immediately.
        let _ = page.evaluate(recorder_injected_script()).await;

        let launch_msg = if connected {
            "Attached to running Chrome. DOM hook installed. Navigate or interact freely."
        } else {
            "Chromium launched. DOM hook installed. Navigate or interact freely."
        };
        push_event(
            &self.buffer,
            &live,
            RawEvent::new(
                "browser",
                "launched",
                serde_json::json!({ "msg": launch_msg, "connected": connected }),
            ),
        );

        // FrameNavigated → re-inject not needed (addScriptToEvaluateOnNewDocument
        // does it automatically) but we still record the URL change.
        if let Ok(mut stream) = page.event_listener::<EventFrameNavigated>().await {
            let buffer = self.buffer.clone();
            let live_cloned = live.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(evt) = stream.next().await {
                    let frame = &evt.frame;
                    let payload = serde_json::json!({
                        "url": frame.url,
                        "frameId": frame.id.inner(),
                    });
                    push_event(
                        &buffer,
                        &live_cloned,
                        RawEvent::new("browser", "navigate", payload),
                    );
                }
            }));
        }

        // Runtime.bindingCalled → DOM-level events from the injected script.
        if let Ok(mut stream) = page.event_listener::<EventBindingCalled>().await {
            let buffer = self.buffer.clone();
            let live_cloned = live.clone();
            tasks.push(tokio::spawn(async move {
                while let Some(evt) = stream.next().await {
                    if evt.name != BINDING_NAME {
                        continue;
                    }
                    match serde_json::from_str::<DomBindingPayload>(&evt.payload) {
                        Ok(p) => {
                            push_event(
                                &buffer,
                                &live_cloned,
                                RawEvent::new("dom", &p.kind, p.payload),
                            );
                        }
                        Err(e) => {
                            push_event(
                                &buffer,
                                &live_cloned,
                                RawEvent::new(
                                    "dom",
                                    "bind_error",
                                    serde_json::json!({
                                        "error": e.to_string(),
                                        "raw": evt.payload,
                                    }),
                                ),
                            );
                        }
                    }
                }
            }));
        }

        // Heartbeat so the UI always shows life signs.
        {
            let buffer = self.buffer.clone();
            let live_cloned = live.clone();
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(3));
                let mut n = 0u64;
                tick.tick().await; // skip immediate fire
                loop {
                    tick.tick().await;
                    n += 1;
                    push_event(
                        &buffer,
                        &live_cloned,
                        RawEvent::new("browser", "heartbeat", serde_json::json!({ "n": n })),
                    );
                }
            }));
        }

        *self.session.lock() = Some(BrowserSession {
            browser: Arc::new(tokio::sync::Mutex::new(browser)),
            tasks,
            connected,
        });
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<Vec<RawEvent>> {
        let session = self.session.lock().take();
        if let Some(s) = session {
            // Graceful close/reap (or disconnect, in connect mode). Awaited
            // here so the caller knows the browser is actually down on return.
            teardown_session(s).await;
        }
        Ok(std::mem::take(&mut *self.buffer.lock()))
    }
}

// ─── Post-processing ────────────────────────────────────────────────────────

/// ActionBuffer window for merging consecutive typing on the same target —
/// matches the 200 ms rule from the design doc (R-08).
const INPUT_MERGE_WINDOW_MS: i64 = 200;
/// Drop a `change` event that follows an `input` on the same selector inside
/// this window — browsers fire `change` on blur with the same value the user
/// already typed, so it's pure redundancy in a recorded flow (R-08).
const CHANGE_DROP_AFTER_INPUT_MS: i64 = 500;
/// Drop a `click` immediately followed by `input` on the same selector —
/// the click was just focus-acquisition, the meaningful action is the typing.
const CLICK_DROP_BEFORE_INPUT_MS: i64 = 250;
/// Two clicks closer than this on the same selector collapse into one
/// (dblclick browser quirk). Keep the later timestamp.
const DUP_CLICK_MS: i64 = 60;

/// Collapse a raw event log into the form that humans (and the YAML
/// converter) want to see:
/// - drop `heartbeat`, `binding_ready`, `bind_error`
/// - merge consecutive `input` events on the same selector if they fall
///   inside a 200ms window — keep the final value, anchor on the last ts
/// - drop redundant `click`→`input` (focus before typing) and `input`→`change`
///   (blur echo) pairs on the same selector
/// - collapse near-duplicate clicks on the same selector (dblclick artifact)
/// - keep everything else verbatim, preserving order
pub fn condense_events(events: &[RawEvent]) -> Vec<RawEvent> {
    let merged = merge_inputs(events);
    coalesce_redundant(merged)
}

fn merge_inputs(events: &[RawEvent]) -> Vec<RawEvent> {
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    for evt in events {
        match evt.kind.as_str() {
            "heartbeat" | "binding_ready" | "bind_error" => continue,
            "input" => {
                let selector = evt
                    .payload
                    .get("selector")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Some(prev) = out.last_mut() {
                    let same_selector = prev
                        .payload
                        .get("selector")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        == selector;
                    if prev.kind == "input"
                        && same_selector
                        && (evt.at_ms - prev.at_ms).abs() <= INPUT_MERGE_WINDOW_MS
                    {
                        if let Some(obj) = prev.payload.as_object_mut() {
                            if let Some(value) = evt.payload.get("value").cloned() {
                                obj.insert("value".into(), value);
                            }
                        }
                        prev.at_ms = evt.at_ms;
                        continue;
                    }
                }
                out.push(evt.clone());
            }
            _ => out.push(evt.clone()),
        }
    }
    out
}

fn coalesce_redundant(events: Vec<RawEvent>) -> Vec<RawEvent> {
    let mut out: Vec<RawEvent> = Vec::with_capacity(events.len());
    for evt in events {
        // input following a click on the same selector → drop the click.
        if evt.kind == "input" {
            if let Some(last) = out.last() {
                if last.kind == "click"
                    && same_selector(last, &evt)
                    && (evt.at_ms - last.at_ms).abs() <= CLICK_DROP_BEFORE_INPUT_MS
                {
                    out.pop();
                }
            }
        }
        // change after input on the same selector → drop the change.
        if evt.kind == "change" {
            if let Some(last) = out.last() {
                if last.kind == "input"
                    && same_selector(last, &evt)
                    && (evt.at_ms - last.at_ms).abs() <= CHANGE_DROP_AFTER_INPUT_MS
                {
                    continue;
                }
            }
        }
        // Near-duplicate clicks (dblclick) → keep the later.
        if evt.kind == "click" {
            if let Some(last) = out.last() {
                if last.kind == "click"
                    && same_selector(last, &evt)
                    && (evt.at_ms - last.at_ms).abs() <= DUP_CLICK_MS
                {
                    out.pop();
                }
            }
        }
        out.push(evt);
    }
    out
}

fn same_selector(a: &RawEvent, b: &RawEvent) -> bool {
    let sa = a.payload.get("selector").and_then(|v| v.as_str());
    let sb = b.payload.get("selector").and_then(|v| v.as_str());
    sa.is_some() && sa == sb
}

/// Build the multi-strategy selector block emitted into the YAML patch. The
/// recorder captures CSS path, XPath and a small label per DOM event — this
/// promotes them into a `selectors:` object that `browser.click` /
/// `browser.type` consume with built-in self-healing fallback. We omit any
/// strategy whose value is empty.
///
/// F-22: the captured CSS path is *scored* ([`selector_score`]). When the
/// best-ranked candidate is an `#id` or `[data-testid]` path, we hoist it into
/// the dedicated `id` / `data_testid` field (the runtime tries those first and
/// they're the most stable), rather than burying it in the `css` string. The
/// raw CSS path is still emitted as a lower-priority fallback. Brittle,
/// volatile-class paths therefore never lead a selector block when a stronger
/// anchor exists.
fn selectors_block(payload: &serde_json::Value) -> serde_yaml::Mapping {
    use selector_score::{best_candidate, SelectorKind};

    let mut sel = serde_yaml::Mapping::new();
    let css = payload
        .get("selector")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let xpath = payload
        .get("xpath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let label = payload
        .get("label")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() < 80);

    // Rank candidates. The DOM-walk path may itself be `#id` / `[data-testid]`;
    // when the best-ranked one is such a single-token anchor, hoist it into the
    // dedicated field so the emitted block leads with the most durable
    // fingerprint. (The scorer routes those shapes to the strong-anchor score
    // even when they arrive in the `css` field, so the winner reflects that.)
    if let Some(best) = best_candidate(None, None, css, label, xpath) {
        let s = best.selector.trim();
        let is_anchor_kind =
            matches!(best.kind, SelectorKind::Id | SelectorKind::DataTestId);
        // Either it came in as a dedicated kind, or it's a css path whose whole
        // value is just `#id` / `[data-testid=...]`.
        if is_anchor_kind || s.starts_with('#') {
            if let Some(id) = id_from_selector(s) {
                sel.insert("id".into(), id.into());
            }
        }
        if s.starts_with("[data-testid") || s.starts_with("[data-test") {
            if let Some(val) = parse_data_testid(s) {
                sel.insert("data_testid".into(), val.into());
            }
        }
    }

    // Always keep the captured strategies as fallbacks so the self-healing
    // chain stays wide — even when a strong anchor led the block. When the CSS
    // path *is* the anchor we already hoisted (e.g. `#id`), it's still useful
    // as a literal fallback, so we keep emitting it either way.
    if let Some(css) = css {
        sel.insert("css".into(), css.into());
    }
    if let Some(xp) = xpath {
        sel.insert("xpath".into(), xp.into());
    }
    if let Some(label) = label {
        sel.insert("aria_label".into(), label.into());
        // `text_includes` is a softer fallback — only emit when label is
        // short enough to be a button/link caption rather than free text.
        if label.len() < 32 {
            sel.insert("text_includes".into(), label.into());
        }
    }
    sel
}

/// Extract a bare id from a `#id` selector. Returns `None` for anything that
/// isn't a single id token (paths, compound selectors) so we never hoist a
/// multi-part path into the `id:` field.
fn id_from_selector(selector: &str) -> Option<String> {
    let s = selector.trim();
    let rest = s.strip_prefix('#')?;
    if rest.is_empty() || rest.contains([' ', '>', '.', '[', ':', '#']) {
        return None;
    }
    Some(rest.to_string())
}

/// Extract the value from a `[data-testid="x"]` / `[data-test='x']` selector.
fn parse_data_testid(selector: &str) -> Option<String> {
    let inner = selector.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (_attr, val) = inner.split_once('=')?;
    let val = val.trim().trim_matches(['"', '\'']);
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

/// Convert a captured event log into a LumoFlow YAML fragment that the user
/// can splice into `spec.steps`. The output is best-effort: selectors are
/// computed from heuristics, so users will still want to review them — but
/// the structure is good enough that "record → run" is one click away for
/// straightforward flows.
///
/// Returns a YAML string. Empty captures return a single comment line so the
/// caller can still concatenate it safely.
pub fn events_to_yaml_patch(events: &[RawEvent]) -> String {
    let condensed = condense_events(events);
    let mut steps: Vec<serde_yaml::Value> = Vec::new();
    let mut idx = 0usize;
    let mut id_for = |prefix: &str| {
        idx += 1;
        format!("{prefix}_{idx}")
    };
    let mut last_url: Option<String> = None;

    for evt in &condensed {
        match (evt.source.as_str(), evt.kind.as_str()) {
            ("browser", "navigate") => {
                let url = evt
                    .payload
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if url.is_empty() || url == "about:blank" {
                    continue;
                }
                if last_url.as_deref() == Some(url) {
                    continue;
                }
                let mut step = serde_yaml::Mapping::new();
                step.insert("id".into(), id_for("open").into());
                step.insert("action".into(), "browser.open".into());
                let mut with = serde_yaml::Mapping::new();
                with.insert("url".into(), url.into());
                step.insert("with".into(), with.into());
                steps.push(step.into());
                last_url = Some(url.to_string());
            }
            ("dom", "click") => {
                let sel = selectors_block(&evt.payload);
                if sel.is_empty() {
                    continue;
                }
                let mut step = serde_yaml::Mapping::new();
                step.insert("id".into(), id_for("click").into());
                step.insert("action".into(), "browser.click".into());
                let mut with = serde_yaml::Mapping::new();
                with.insert("selectors".into(), sel.into());
                step.insert("with".into(), with.into());
                steps.push(step.into());
            }
            ("dom", "input") | ("dom", "change") => {
                let sel = selectors_block(&evt.payload);
                if sel.is_empty() {
                    continue;
                }
                let value = evt
                    .payload
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if value.is_empty() {
                    continue;
                }
                let mut step = serde_yaml::Mapping::new();
                step.insert("id".into(), id_for("type").into());
                step.insert("action".into(), "browser.type".into());
                let mut with = serde_yaml::Mapping::new();
                with.insert("selectors".into(), sel.into());
                with.insert("text".into(), value.into());
                with.insert("clear".into(), true.into());
                step.insert("with".into(), with.into());
                steps.push(step.into());
            }
            ("dom", "keydown") => {
                let key = evt
                    .payload
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if key == "Enter" {
                    let mut step = serde_yaml::Mapping::new();
                    step.insert("id".into(), id_for("submit").into());
                    step.insert("action".into(), "browser.click".into());
                    let mut with = serde_yaml::Mapping::new();
                    let mut sel = serde_yaml::Mapping::new();
                    sel.insert(
                        "css".into(),
                        "button[type=submit], input[type=submit]".into(),
                    );
                    with.insert("selectors".into(), sel.into());
                    step.insert("with".into(), with.into());
                    steps.push(step.into());
                }
            }
            ("dom", "similar_grab") => {
                // R-09: alt-click on a card-like element. We emit a single
                // browser.extract step with all: true so one capture turns into
                // a batch — the YingDao "similar elements" superpower.
                let general = evt
                    .payload
                    .get("generalized_selector")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if general.is_empty() {
                    continue;
                }
                let sibling_count = evt
                    .payload
                    .get("sibling_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let mut step = serde_yaml::Mapping::new();
                step.insert("id".into(), id_for("extract").into());
                step.insert("action".into(), "browser.extract".into());
                let mut with = serde_yaml::Mapping::new();
                with.insert("selector".into(), general.into());
                with.insert("all".into(), true.into());
                step.insert("with".into(), with.into());
                // Attach a sibling-count comment via a `_note` field — the user
                // sees how many items were spotted, schema layer rejects it so
                // they must strip before running. That's fine; reviewers should
                // skim recorder output anyway.
                if sibling_count > 0 {
                    step.insert(
                        "# note".into(),
                        format!("recorder spotted {sibling_count} similar items").into(),
                    );
                }
                steps.push(step.into());
            }
            _ => {}
        }
    }

    let mut yaml = String::new();
    yaml.push_str("# Recorder YAML patch — review selectors before merging.\n");
    yaml.push_str("# Each step ships a 4-fingerprint `selectors:` block so the\n");
    yaml.push_str("# runtime self-heals through CSS → XPath → aria-label → text.\n");
    if steps.is_empty() {
        yaml.push_str("# (no actionable events were captured)\n");
        return yaml;
    }
    match serde_yaml::to_string(&serde_yaml::Value::Sequence(steps)) {
        Ok(s) => yaml.push_str(&s),
        Err(e) => yaml.push_str(&format!("# yaml error: {e}\n")),
    }
    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evt(kind: &str, payload: serde_json::Value, at_ms: i64) -> RawEvent {
        RawEvent {
            source: if kind == "navigate" { "browser" } else { "dom" }.into(),
            kind: kind.into(),
            at_ms,
            payload,
        }
    }

    #[test]
    fn condense_merges_input_within_window() {
        let events = vec![
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "h"}),
                1000,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "he"}),
                1050,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "hell"}),
                1100,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "hello"}),
                1150,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1, "should merge into one input");
        assert_eq!(out[0].payload.get("value").unwrap().as_str(), Some("hello"));
        assert_eq!(out[0].at_ms, 1150);
    }

    #[test]
    fn condense_does_not_merge_across_window() {
        let events = vec![
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "h"}),
                1000,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "hello"}),
                1500,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 2, "200ms apart → not merged");
    }

    #[test]
    fn condense_does_not_merge_across_selectors() {
        let events = vec![
            evt(
                "input",
                serde_json::json!({"selector": "#a", "value": "1"}),
                1000,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#b", "value": "2"}),
                1050,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn condense_drops_heartbeat_and_bind_diag() {
        let events = vec![
            evt("heartbeat", serde_json::json!({"n": 1}), 1000),
            evt("binding_ready", serde_json::json!({}), 1100),
            evt("click", serde_json::json!({"selector": "#x"}), 1200),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "click");
    }

    #[test]
    fn condense_drops_click_immediately_before_input_on_same_selector() {
        // Click into field then type → the click is just focus, drop it.
        let events = vec![
            evt("click", serde_json::json!({"selector": "#q"}), 1000),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "hi"}),
                1100,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "input");
    }

    #[test]
    fn condense_keeps_click_when_input_is_on_different_selector() {
        // Click button A then type into field B → both meaningful.
        let events = vec![
            evt("click", serde_json::json!({"selector": "#btn"}), 1000),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "x"}),
                1100,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, "click");
        assert_eq!(out[1].kind, "input");
    }

    #[test]
    fn condense_keeps_click_when_input_is_too_late() {
        // Click then wait 2s then type → no longer a focus-then-type pair.
        let events = vec![
            evt("click", serde_json::json!({"selector": "#q"}), 1000),
            evt(
                "input",
                serde_json::json!({"selector": "#q", "value": "hi"}),
                4000,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn condense_drops_change_following_input_on_same_selector() {
        // Browsers fire `change` on blur with the typed value → redundant.
        let events = vec![
            evt(
                "input",
                serde_json::json!({"selector": "#email", "value": "a@b"}),
                1000,
            ),
            evt(
                "change",
                serde_json::json!({"selector": "#email", "value": "a@b"}),
                1200,
            ),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "input");
    }

    #[test]
    fn condense_keeps_change_when_no_recent_input() {
        // A bare `change` (e.g. <select> menu pick) survives.
        let events = vec![evt(
            "change",
            serde_json::json!({"selector": "#country", "value": "JP"}),
            1000,
        )];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, "change");
    }

    #[test]
    fn condense_collapses_dblclick_into_single_click() {
        let events = vec![
            evt("click", serde_json::json!({"selector": "#row"}), 1000),
            evt("click", serde_json::json!({"selector": "#row"}), 1030),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at_ms, 1030, "keep the later timestamp");
    }

    #[test]
    fn condense_keeps_two_clicks_far_apart() {
        let events = vec![
            evt("click", serde_json::json!({"selector": "#row"}), 1000),
            evt("click", serde_json::json!({"selector": "#row"}), 2000),
        ];
        let out = condense_events(&events);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn yaml_patch_renders_open_click_type_submit() {
        let mut e = vec![
            evt(
                "navigate",
                serde_json::json!({"url": "https://example.com/login"}),
                1000,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#user", "value": "alice"}),
                1100,
            ),
            evt(
                "input",
                serde_json::json!({"selector": "#user", "value": "alice@x"}),
                1180,
            ),
            evt(
                "click",
                serde_json::json!({"selector": "button.login"}),
                1300,
            ),
            evt("keydown", serde_json::json!({"key": "Enter"}), 1350),
        ];
        // First event's source needs to be browser (helper sets it based on kind).
        e[0].source = "browser".into();
        let yaml = events_to_yaml_patch(&e);
        assert!(yaml.contains("browser.open"));
        assert!(yaml.contains("https://example.com/login"));
        assert!(yaml.contains("browser.click"));
        assert!(yaml.contains("browser.type"));
        assert!(yaml.contains("alice@x"));
        assert!(yaml.contains("type[submit]") || yaml.contains("input[type=submit]"));
    }

    #[test]
    fn yaml_patch_empty_returns_comment() {
        let yaml = events_to_yaml_patch(&[]);
        assert!(yaml.contains("no actionable events were captured"));
    }

    #[test]
    fn yaml_patch_emits_multi_strategy_selectors() {
        let mut click_evt = evt(
            "click",
            serde_json::json!({
                "selector": "button.primary",
                "xpath": "//button[1]",
                "label": "登录",
            }),
            1000,
        );
        click_evt.source = "dom".into();
        let yaml = events_to_yaml_patch(&[click_evt]);
        assert!(yaml.contains("selectors:"), "should emit selectors block");
        assert!(yaml.contains("css:"), "css strategy present");
        assert!(yaml.contains("button.primary"));
        assert!(yaml.contains("xpath:"), "xpath strategy present");
        assert!(yaml.contains("//button[1]"));
        assert!(yaml.contains("aria_label:"), "aria_label strategy present");
        assert!(yaml.contains("登录"));
    }

    #[test]
    fn yaml_patch_skips_empty_label() {
        let mut click_evt = evt(
            "click",
            serde_json::json!({
                "selector": "button.primary",
                "label": "",
            }),
            1000,
        );
        click_evt.source = "dom".into();
        let yaml = events_to_yaml_patch(&[click_evt]);
        assert!(yaml.contains("css:"));
        assert!(!yaml.contains("aria_label:"));
    }

    #[test]
    fn yaml_patch_renders_similar_grab_as_extract_all() {
        let mut grab = evt(
            "similar_grab",
            serde_json::json!({
                "generalized_selector": "ul.cards > li.card",
                "sibling_count": 12,
                "sample_values": ["Card A", "Card B", "Card C"],
            }),
            1000,
        );
        grab.source = "dom".into();
        let yaml = events_to_yaml_patch(&[grab]);
        assert!(yaml.contains("browser.extract"));
        assert!(yaml.contains("ul.cards > li.card"));
        assert!(yaml.contains("all: true"));
        assert!(yaml.contains("12 similar items"));
    }

    #[test]
    fn yaml_patch_skips_similar_grab_without_selector() {
        let mut grab = evt(
            "similar_grab",
            serde_json::json!({
                "generalized_selector": "",
                "sibling_count": 0,
            }),
            1000,
        );
        grab.source = "dom".into();
        let yaml = events_to_yaml_patch(&[grab]);
        assert!(!yaml.contains("browser.extract"));
    }

    // ─── F-22: selector scoring in the YAML patch ─────────────────────────

    #[test]
    fn yaml_patch_hoists_id_path_into_id_field() {
        // A DOM-walk that resolved to `#id` should lead with an `id:` anchor,
        // not bury it in the `css` string.
        let mut click = evt(
            "click",
            serde_json::json!({
                "selector": "#login-btn",
                "xpath": "//button[1]",
            }),
            1000,
        );
        click.source = "dom".into();
        let yaml = events_to_yaml_patch(&[click]);
        assert!(yaml.contains("id: login-btn"), "should hoist #id → id:\n{yaml}");
        // CSS path still kept as a fallback.
        assert!(yaml.contains("css:"));
        assert!(yaml.contains("xpath:"));
    }

    #[test]
    fn yaml_patch_hoists_data_testid_path() {
        let mut click = evt(
            "click",
            serde_json::json!({ "selector": "[data-testid=\"submit\"]" }),
            1000,
        );
        click.source = "dom".into();
        let yaml = events_to_yaml_patch(&[click]);
        assert!(
            yaml.contains("data_testid: submit"),
            "should hoist [data-testid] → data_testid:\n{yaml}"
        );
    }

    #[test]
    fn yaml_patch_leaves_plain_class_path_in_css() {
        // No strong anchor → no id/data_testid selector field, css leads. (Note
        // the step itself has an `id:` key — we check the selectors block, not
        // the step header.)
        let mut click = evt(
            "click",
            serde_json::json!({ "selector": "form > button.login-button" }),
            1000,
        );
        click.source = "dom".into();
        let yaml = events_to_yaml_patch(&[click]);
        // The hoisted-anchor fields must be absent from the selectors block.
        assert!(!yaml.contains("data_testid:"));
        assert!(!yaml.contains("aria_label:"));
        assert!(yaml.contains("button.login-button"));
    }
}

