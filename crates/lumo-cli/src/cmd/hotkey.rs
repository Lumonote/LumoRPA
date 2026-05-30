//! T-05: global hotkey trigger.
//!
//! Scans `--flows` for declarations like
//!
//! ```yaml
//! triggers:
//!   - { kind: hotkey, with: { keys: "Ctrl+Shift+L" } }
//! ```
//!
//! and spawns one platform listener task per flow. When the combination is
//! pressed, the flow runs with `inputs: { trigger: { keys, at } }` so the
//! flow can tell whether it was kicked off manually, on cron, or via a
//! hotkey.
//!
//! Platform integration:
//! - macOS / Linux: `evdev`-style global key capture requires Accessibility
//!   (macOS) or `uinput` (Linux) — we surface that via a one-shot
//!   permission probe at startup and emit a clear error if access is
//!   denied, rather than silently failing.
//! - Windows: `RegisterHotKey` requires only standard user perms.
//!
//! The actual platform input plumbing is intentionally pluggable: the
//! [`Listener`] trait abstracts "wait for the next match" so tests can
//! drive the dispatcher through a scripted listener without depending on
//! `rdev` / `windows`.

use async_trait::async_trait;
use lumo_core::{FlowVm, RunOptions};
use lumo_storage::Repo;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::build_action_registry;

/// Parsed hotkey trigger pulled from a flow's `spec.triggers`.
#[derive(Debug, Clone)]
pub struct HotkeyFlow {
    pub name: String,
    pub flow_path: PathBuf,
    pub keys: HotkeyCombo,
}

/// Modifier set + final key. We normalize on parse so `"Ctrl+Shift+L"` and
/// `"shift + control + l"` collapse to the same value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotkeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    /// Final non-modifier key, lowercased. E.g. `"l"`, `"f5"`, `"enter"`.
    pub key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("empty hotkey expression")]
    Empty,
    #[error("hotkey `{0}` does not contain a non-modifier key (e.g. add `+L`)")]
    OnlyModifiers(String),
    #[error("hotkey `{0}` declares no modifier — refuse to grab a bare keystroke")]
    OnlyKey(String),
    #[error("hotkey contains unknown token `{0}`")]
    UnknownToken(String),
}

impl HotkeyCombo {
    /// Canonical "Ctrl+Shift+L"-style label, useful for log lines so the
    /// startup banner echoes what the user wrote in YAML.
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.meta {
            parts.push("Cmd");
        }
        let key = self.key.to_uppercase();
        parts.push(&key);
        parts.join("+")
    }
}

/// Parse a textual hotkey like `"Ctrl+Shift+L"` into a [`HotkeyCombo`].
///
/// Recognised modifier names (case-insensitive): `ctrl`, `control`, `shift`,
/// `alt`, `option`, `meta`, `cmd`, `super`, `win`. Anything else is treated
/// as the final key (one token only).
pub fn parse_hotkey(spec: &str) -> Result<HotkeyCombo, HotkeyError> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(HotkeyError::Empty);
    }
    let mut combo = HotkeyCombo {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        key: String::new(),
    };
    let mut last_key: Option<String> = None;
    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => combo.ctrl = true,
            "shift" => combo.shift = true,
            "alt" | "option" => combo.alt = true,
            "meta" | "cmd" | "super" | "win" => combo.meta = true,
            _ => {
                if last_key.is_some() {
                    return Err(HotkeyError::UnknownToken(token.to_string()));
                }
                last_key = Some(lower);
            }
        }
    }
    let Some(key) = last_key else {
        return Err(HotkeyError::OnlyModifiers(trimmed.to_string()));
    };
    if !(combo.ctrl || combo.shift || combo.alt || combo.meta) {
        return Err(HotkeyError::OnlyKey(trimmed.to_string()));
    }
    combo.key = key;
    Ok(combo)
}

/// Walk `flows_dir`, returning one entry per (`flow` × `hotkey trigger`).
/// Bad combos are logged and skipped, matching the cron/file scanners.
pub fn scan_hotkey_triggers(flows_dir: &Path) -> Vec<HotkeyFlow> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(flows_dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_flow_path(&path) {
            continue;
        }
        let flow = match lumo_dsl::parse_file(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  ! hotkey skip: {} parse: {e}", path.display());
                continue;
            }
        };
        if let Err(e) = lumo_dsl::validate(&flow) {
            eprintln!("  ! hotkey skip: {} validate: {e}", path.display());
            continue;
        }
        for trigger in &flow.spec.triggers {
            if trigger.kind != "hotkey" {
                continue;
            }
            let raw = trigger
                .with
                .get("keys")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let Some(raw) = raw else {
                eprintln!(
                    "  ! hotkey skip: {} trigger missing `keys` string",
                    path.display()
                );
                continue;
            };
            let combo = match parse_hotkey(&raw) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("  ! hotkey skip: {} `{raw}`: {e}", path.display());
                    continue;
                }
            };
            out.push(HotkeyFlow {
                name: flow_display_name(&path),
                flow_path: path.clone(),
                keys: combo,
            });
        }
    }
    out
}

fn is_flow_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".lumoflow.yaml") || n.ends_with(".lumoflow.yml"))
        .unwrap_or(false)
}

fn flow_display_name(path: &Path) -> String {
    let raw = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
    raw.strip_suffix(".lumoflow.yaml")
        .or_else(|| raw.strip_suffix(".lumoflow.yml"))
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"))
        .to_string()
}

/// Abstract "give me the next hotkey hit" so the dispatch loop can be
/// driven by a real OS listener in production and by a fake one in tests.
#[async_trait]
pub trait Listener: Send + Sync {
    /// Block until the configured combo fires. Returns `Ok(())` for one hit.
    async fn wait(&self) -> anyhow::Result<()>;
}

/// Singleton platform side: takes care of one global `rdev::listen`
/// thread (real backend) or holds no state (stub). Each registered flow
/// is given its own [`Listener`] handle so the dispatcher only sees per-
/// flow notifications, even though the underlying OS hook is shared.
pub trait Hub: Send + Sync {
    /// Surface OS permission status so the startup banner can warn the
    /// user when global hotkeys can't actually fire (macOS Accessibility,
    /// Linux uinput, etc.).
    fn permission_status(&self) -> PermissionStatus;
    /// Register a combo and return a listener that fires only when that
    /// combo is pressed.
    fn register(&self, combo: HotkeyCombo) -> Arc<dyn Listener>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// All set — the listener should fire when the combo is pressed.
    Ready,
    /// The OS permission has not been granted yet. The CLI surfaces this so
    /// the user can open the right Settings pane.
    #[allow(dead_code)]
    NeedsAccessibility,
    /// The platform doesn't support global hotkeys (Linux without uinput,
    /// containers, etc.). Hotkey triggers are skipped gracefully.
    #[allow(dead_code)]
    Unsupported,
}

/// Run one flow when its hotkey fires. Mirrors `run_cron_flow` / `run_file_flow`
/// in `serve.rs` so all trigger lanes record runs through the same `Repo`.
pub async fn run_hotkey_flow(flow_path: &Path, home: &Path, label: &str) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(flow_path)?;
    lumo_dsl::validate(&flow)?;
    let registry = build_action_registry(home, Some(flow_path));
    let repo = Some(Repo::open(home.join("lumo.db"))?);
    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), home, &flow)
        .with_vault(super::load_vault_identity(home));
    let mut inputs = serde_json::Map::new();
    inputs.insert(
        "trigger".into(),
        serde_json::json!({
            "kind": "hotkey",
            "keys": label,
            "at": chrono::Utc::now().to_rfc3339(),
        }),
    );
    vm.run(
        &flow,
        RunOptions {
            inputs: Value::Object(inputs),
            trigger_kind: "hotkey".into(),
        },
    )
    .await?;
    Ok(())
}

/// Wrap a listener loop so the caller just spawns a Tokio task per flow.
/// On each successful `wait` the flow is dispatched; errors are logged but
/// never tear the listener down — a transient X11 / WinAPI hiccup
/// shouldn't disable the hotkey for the rest of the session.
pub async fn dispatch_loop(flow: HotkeyFlow, home: PathBuf, listener: Arc<dyn Listener>) {
    loop {
        if let Err(e) = listener.wait().await {
            tracing::warn!("hotkey listener {}: {e}", flow.name);
            // Sleep briefly so a hot-looping error doesn't burn CPU.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }
        let label = flow.keys.label();
        if let Err(e) = run_hotkey_flow(&flow.flow_path, &home, &label).await {
            tracing::error!("hotkey run {}: {e}", flow.name);
        }
    }
}

/// Returns the platform-default hub. With the `hotkey-rdev` feature
/// (default-on) we install a global `rdev::listen` thread and dispatch
/// matches into per-flow tokio channels. With the feature disabled the
/// hub returns permissive stub listeners that never fire — useful for
/// headless CI and environments where the rdev native deps are missing.
///
/// On macOS, the first hotkey press will trigger the system Accessibility
/// permission prompt; without that grant `rdev::listen` returns
/// immediately and `permission_status()` reports `NeedsAccessibility`.
/// We surface that status from `lumo serve`'s startup banner so the user
/// can open the right Settings pane without grepping logs.
pub fn default_hub() -> Arc<dyn Hub> {
    #[cfg(feature = "hotkey-rdev")]
    {
        Arc::new(rdev_backend::RdevHub::start())
    }
    #[cfg(not(feature = "hotkey-rdev"))]
    {
        Arc::new(stub::StubHub)
    }
}

#[cfg(feature = "hotkey-rdev")]
mod rdev_backend {
    //! Real global hotkey backend driven by `rdev::listen`.
    //!
    //! `rdev::listen` blocks the calling thread for the whole process
    //! lifetime, so we run it on a dedicated OS thread spawned at the
    //! first `default_hub()` call. All registered hotkeys live in a
    //! shared list and the rdev callback notifies any matching tokio
    //! channel waiter.
    //!
    //! When `rdev::listen` returns an error (typically macOS Accessibility
    //! not granted, or `EPERM` reading `/dev/input/event*` on Linux), the
    //! hub records `NeedsAccessibility`. Existing listeners then park
    //! forever — the user grants the permission and restarts `lumo serve`
    //! to pick up the new state.

    use super::*;
    use parking_lot::Mutex;
    use tokio::sync::Notify;

    type Combos = Vec<(HotkeyCombo, Arc<Notify>)>;

    pub struct RdevHub {
        combos: Arc<Mutex<Combos>>,
        status: Arc<Mutex<PermissionStatus>>,
    }

    impl RdevHub {
        pub fn start() -> Self {
            let combos: Arc<Mutex<Combos>> = Arc::new(Mutex::new(Vec::new()));
            let status = Arc::new(Mutex::new(PermissionStatus::Ready));
            spawn_rdev_thread(combos.clone(), status.clone());
            Self { combos, status }
        }
    }

    fn spawn_rdev_thread(combos: Arc<Mutex<Combos>>, status: Arc<Mutex<PermissionStatus>>) {
        std::thread::Builder::new()
            .name("lumo-hotkey".into())
            .spawn(move || {
                use rdev::{listen, EventType};
                let mut ctrl = false;
                let mut shift = false;
                let mut alt = false;
                let mut meta = false;
                let combos = combos.clone();
                let status_clone = status.clone();
                let res = listen(move |evt| match evt.event_type {
                    EventType::KeyPress(key) => {
                        if matches_modifier(key, Modifier::Ctrl) {
                            ctrl = true;
                        } else if matches_modifier(key, Modifier::Shift) {
                            shift = true;
                        } else if matches_modifier(key, Modifier::Alt) {
                            alt = true;
                        } else if matches_modifier(key, Modifier::Meta) {
                            meta = true;
                        } else if let Some(name) = key_to_name(&key) {
                            let g = combos.lock();
                            for (combo, notify) in g.iter() {
                                if combo.ctrl == ctrl
                                    && combo.shift == shift
                                    && combo.alt == alt
                                    && combo.meta == meta
                                    && combo.key == name
                                {
                                    notify.notify_one();
                                }
                            }
                        }
                    }
                    EventType::KeyRelease(key) => {
                        if matches_modifier(key, Modifier::Ctrl) {
                            ctrl = false;
                        } else if matches_modifier(key, Modifier::Shift) {
                            shift = false;
                        } else if matches_modifier(key, Modifier::Alt) {
                            alt = false;
                        } else if matches_modifier(key, Modifier::Meta) {
                            meta = false;
                        }
                    }
                    _ => {}
                });
                if let Err(e) = res {
                    tracing::warn!("rdev::listen exited: {e:?}");
                    *status_clone.lock() = PermissionStatus::NeedsAccessibility;
                }
            })
            .expect("spawn lumo-hotkey thread");
    }

    enum Modifier {
        Ctrl,
        Shift,
        Alt,
        Meta,
    }

    fn matches_modifier(key: rdev::Key, m: Modifier) -> bool {
        use rdev::Key::*;
        match m {
            Modifier::Ctrl => matches!(key, ControlLeft | ControlRight),
            Modifier::Shift => matches!(key, ShiftLeft | ShiftRight),
            Modifier::Alt => matches!(key, Alt | AltGr),
            Modifier::Meta => matches!(key, MetaLeft | MetaRight),
        }
    }

    /// Normalise an rdev `Key` into the lowercased token that
    /// `parse_hotkey` emits. Only the keys the parser actually accepts
    /// are mapped — anything else returns `None` so unknown keystrokes
    /// can't accidentally fire a registered combo.
    fn key_to_name(key: &rdev::Key) -> Option<String> {
        use rdev::Key::*;
        Some(match key {
            KeyA => "a".into(),
            KeyB => "b".into(),
            KeyC => "c".into(),
            KeyD => "d".into(),
            KeyE => "e".into(),
            KeyF => "f".into(),
            KeyG => "g".into(),
            KeyH => "h".into(),
            KeyI => "i".into(),
            KeyJ => "j".into(),
            KeyK => "k".into(),
            KeyL => "l".into(),
            KeyM => "m".into(),
            KeyN => "n".into(),
            KeyO => "o".into(),
            KeyP => "p".into(),
            KeyQ => "q".into(),
            KeyR => "r".into(),
            KeyS => "s".into(),
            KeyT => "t".into(),
            KeyU => "u".into(),
            KeyV => "v".into(),
            KeyW => "w".into(),
            KeyX => "x".into(),
            KeyY => "y".into(),
            KeyZ => "z".into(),
            Num0 => "0".into(),
            Num1 => "1".into(),
            Num2 => "2".into(),
            Num3 => "3".into(),
            Num4 => "4".into(),
            Num5 => "5".into(),
            Num6 => "6".into(),
            Num7 => "7".into(),
            Num8 => "8".into(),
            Num9 => "9".into(),
            F1 => "f1".into(),
            F2 => "f2".into(),
            F3 => "f3".into(),
            F4 => "f4".into(),
            F5 => "f5".into(),
            F6 => "f6".into(),
            F7 => "f7".into(),
            F8 => "f8".into(),
            F9 => "f9".into(),
            F10 => "f10".into(),
            F11 => "f11".into(),
            F12 => "f12".into(),
            Space => "space".into(),
            Return => "enter".into(),
            Escape => "escape".into(),
            Tab => "tab".into(),
            _ => return None,
        })
    }

    pub struct FlowListener {
        notify: Arc<Notify>,
    }

    #[async_trait]
    impl Listener for FlowListener {
        async fn wait(&self) -> anyhow::Result<()> {
            self.notify.notified().await;
            Ok(())
        }
    }

    impl Hub for RdevHub {
        fn permission_status(&self) -> PermissionStatus {
            *self.status.lock()
        }
        fn register(&self, combo: HotkeyCombo) -> Arc<dyn Listener> {
            let notify = Arc::new(Notify::new());
            self.combos.lock().push((combo, notify.clone()));
            Arc::new(FlowListener { notify })
        }
    }
}

#[cfg(not(feature = "hotkey-rdev"))]
mod stub {
    use super::*;

    /// Hub that hands out [`StubListener`]s — used when the rdev backend
    /// is feature-gated out (CI / headless / Linux without uinput).
    pub struct StubHub;

    #[derive(Debug, Default)]
    pub struct StubListener;

    #[async_trait]
    impl Listener for StubListener {
        async fn wait(&self) -> anyhow::Result<()> {
            // Parking forever yields to the runtime — the dispatch loop
            // sleeps cheaply rather than busy-polling. This is the path
            // exercised when global hotkeys are unavailable.
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    impl Hub for StubHub {
        fn permission_status(&self) -> PermissionStatus {
            PermissionStatus::Unsupported
        }
        fn register(&self, _combo: HotkeyCombo) -> Arc<dyn Listener> {
            Arc::new(StubListener)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    fn write_flow(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.lumoflow.yaml")), body.trim_start()).unwrap();
    }

    #[test]
    fn parse_ctrl_shift_l() {
        let c = parse_hotkey("Ctrl+Shift+L").unwrap();
        assert!(c.ctrl);
        assert!(c.shift);
        assert!(!c.alt);
        assert!(!c.meta);
        assert_eq!(c.key, "l");
    }

    #[test]
    fn parse_is_case_insensitive() {
        let a = parse_hotkey("ctrl+shift+L").unwrap();
        let b = parse_hotkey("CTRL+SHIFT+l").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_accepts_alt_and_meta_aliases() {
        let c = parse_hotkey("Cmd+Option+F5").unwrap();
        assert!(c.meta);
        assert!(c.alt);
        assert_eq!(c.key, "f5");
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(matches!(parse_hotkey("   "), Err(HotkeyError::Empty)));
    }

    #[test]
    fn parse_rejects_only_modifiers() {
        assert!(matches!(
            parse_hotkey("Ctrl+Shift"),
            Err(HotkeyError::OnlyModifiers(_))
        ));
    }

    #[test]
    fn parse_rejects_bare_key() {
        // No modifier → we refuse to grab a single keystroke, which would
        // otherwise eat the user's typing globally.
        assert!(matches!(parse_hotkey("L"), Err(HotkeyError::OnlyKey(_))));
    }

    #[test]
    fn parse_rejects_unknown_token() {
        // Two non-modifier tokens isn't valid — only one final key allowed.
        assert!(matches!(
            parse_hotkey("Ctrl+Foo+Bar"),
            Err(HotkeyError::UnknownToken(_))
        ));
    }

    #[test]
    fn label_round_trips_canonical_form() {
        let c = parse_hotkey("shift+ctrl+L").unwrap();
        assert_eq!(c.label(), "Ctrl+Shift+L");
    }

    fn flow_with_hotkey(id: &str, keys: &str) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  triggers:
    - {{ kind: hotkey, with: {{ keys: "{keys}" }} }}
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "from hotkey" }} }}
"#,
        )
    }

    #[test]
    fn scan_finds_flows_with_hotkey_triggers() {
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "a", &flow_with_hotkey("a", "Ctrl+Shift+L"));
        write_flow(flows.path(), "b", &flow_with_hotkey("b", "Cmd+Alt+B"));
        let scans = scan_hotkey_triggers(flows.path());
        assert_eq!(scans.len(), 2);
        let names: Vec<_> = scans.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn scan_skips_invalid_hotkey_string() {
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "bad", &flow_with_hotkey("bad", "L"));
        let scans = scan_hotkey_triggers(flows.path());
        assert!(scans.is_empty(), "bare-key hotkey must be rejected");
    }

    #[test]
    fn scan_skips_flow_without_hotkey_trigger() {
        let flows = TempDir::new().unwrap();
        let yaml = r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: { id: x }
spec:
  triggers:
    - { kind: webhook }
  steps:
    - { id: hi, action: control.log, with: { message: "" } }
"#;
        write_flow(flows.path(), "x", yaml);
        assert!(scan_hotkey_triggers(flows.path()).is_empty());
    }

    /// Test-only listener: completes `wait` `count` times then loops forever.
    /// Lets the dispatcher run a flow N times under tokio without depending
    /// on a real OS hook.
    #[derive(Debug)]
    struct ScriptedListener {
        hits: AtomicUsize,
        max: usize,
    }

    impl ScriptedListener {
        fn new(max: usize) -> Arc<Self> {
            Arc::new(Self {
                hits: AtomicUsize::new(0),
                max,
            })
        }
    }

    #[async_trait]
    impl Listener for ScriptedListener {
        async fn wait(&self) -> anyhow::Result<()> {
            let now = self.hits.fetch_add(1, Ordering::SeqCst);
            if now >= self.max {
                // Park forever once the script is exhausted.
                std::future::pending::<()>().await;
            }
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scripted_listener_records_hits() {
        let l = ScriptedListener::new(2);
        // Two real hits, then `wait` would park forever; we don't await the
        // third call, just confirm the counter advanced.
        l.wait().await.unwrap();
        l.wait().await.unwrap();
        assert_eq!(l.hits.load(Ordering::SeqCst), 2);
    }
}
