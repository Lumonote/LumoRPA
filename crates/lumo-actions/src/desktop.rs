//! F-1 桌面输入自动化(坐标 + 图像驱动),仅在 feature `desktop` 下编译。
//!
//! 经 `rdev` 模拟 OS 级鼠标 / 键盘事件。桌面输入没有纯 Rust 实现(必走 macOS
//! CGEvent / Linux XTest / Windows SendInput 的 C API),故整族放在**默认关闭**的
//! `desktop` feature 后,不污染信创核心 / headless 交叉编译;实际桌面端(Tauri /
//! CLI)按需 `--features desktop` 开启。
//!
//! 动作(均经 `desktop` 能力闸门按粗粒度类别授权:`mouse` / `keyboard` / `*`):
//! - `desktop.move`   —— 移动指针到绝对屏幕坐标 (x, y)。
//! - `desktop.click`  —— 在 (x, y)(缺省=当前位置)按 left/right/middle,可双击。
//! - `desktop.scroll` —— 滚轮 (dx, dy)。
//! - `desktop.key`    —— 组合键,如 `ctrl+c`、`enter`、`cmd+shift+t`。
//! - `desktop.type`   —— 输入文本:走**剪贴板粘贴**(arboard 写入 + 模拟 Cmd/Ctrl+V),
//!   对任意 Unicode / 中文安全(rdev 逐键模拟产不出中文,只能 ASCII)。
//!
//! 「图像驱动点击」由 `image.locate → desktop.click` 组合达成:locate 回传
//! center_x/center_y 直接喂 click 的 x/y(见 examples/desktop-click.lumoflow.yaml)。
//! 原生整屏截图暂缓(xcap 需 LLVM/pipewire/libxcb 破坏信创),haystack 由既有
//! `browser.screenshot` 或外部提供。
//!
//! rdev::simulate 同步、且合成事件间须留间隔(过快会被 OS 合并/丢弃),故所有触发
//! 挪 `spawn_blocking`。macOS 首次运行需「辅助功能」授权;CI/headless 无显示,触发
//! 类用例标 `#[ignore]`,仅能力 / 入参校验在 CI 跑(在闸门/解析阶段即返回,不触 rdev)。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx, StepInterrupt};
use once_cell::sync::Lazy;
use rdev::{simulate, Button, EventType, Key};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub fn register(r: &mut ActionRegistry) {
    r.register(MoveAction);
    r.register(ClickAction);
    r.register(DragAction);
    r.register(ScrollAction);
    r.register(KeyAction);
    r.register(TypeAction);
}

// ---------------------------------------------------------------------------
// desktop.drag
// ---------------------------------------------------------------------------

pub struct DragAction;

fn default_drag_duration_ms() -> u64 {
    500
}

fn default_drag_steps() -> u32 {
    20
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DragIn {
    from_x: f64,
    from_y: f64,
    to_x: f64,
    to_y: f64,
    #[serde(default)]
    button: ClickButton,
    #[serde(default = "default_drag_duration_ms")]
    duration_ms: u64,
    #[serde(default = "default_drag_steps")]
    steps: u32,
}

#[async_trait]
impl Action for DragAction {
    fn id(&self) -> &'static str {
        "desktop.drag"
    }
    fn summary(&self) -> &'static str {
        "Drag from one absolute screen coordinate to another"
    }
    fn schema(&self) -> &'static Value {
        static S: Lazy<Value> = Lazy::new(crate::schema::derive::<DragIn>);
        &S
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let DragIn {
            from_x,
            from_y,
            to_x,
            to_y,
            button,
            duration_ms,
            steps,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.drag input invalid: {e}")))?;
        ctx.ensure_desktop("mouse")?;
        if duration_ms == 0 {
            return Err(StepError::msg("desktop.drag: `duration_ms` must be >= 1"));
        }
        if steps == 0 {
            return Err(StepError::msg("desktop.drag: `steps` must be >= 1"));
        }
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || {
            let mut input = NativeInput::new(interrupt, send);
            input.event(EventType::MouseMove {
                x: from_x,
                y: from_y,
            })?;
            input.event(EventType::ButtonPress(button.to_rdev()))?;
            let pause = Duration::from_millis(duration_ms / u64::from(steps));
            for step in 1..=steps {
                let t = f64::from(step) / f64::from(steps);
                input.event(EventType::MouseMove {
                    x: from_x + (to_x - from_x) * t,
                    y: from_y + (to_y - from_y) * t,
                })?;
                if !pause.is_zero() {
                    input.wait(pause)?;
                }
            }
            input.event(EventType::ButtonRelease(button.to_rdev()))?;
            input.check()
        })
        .await
        .map_err(|e| StepError::msg(format!("desktop.drag join: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({
            "dragged": true,
            "from": {"x": from_x, "y": from_y},
            "to": {"x": to_x, "y": to_y},
            "duration_ms": duration_ms,
            "steps": steps,
        })))
    }
}

/// 合成事件间的节流:OS 事件队列需要一点时间,过快会被合并 / 丢弃。
const TICK: Duration = Duration::from_millis(20);

/// 发一个事件并停顿一拍。仅在 `spawn_blocking` 内调用,sleep 不阻塞 async 执行器。
/// All callers must go through NativeInput so interruption releases held input.
fn send(ev: EventType) -> Result<(), StepError> {
    simulate(&ev).map_err(|e| StepError::msg(format!("desktop simulate failed: {e:?}")))?;
    std::thread::sleep(TICK);
    Ok(())
}

/// One native input transaction. Cancellation blocks every new event, while
/// Drop releases keys/buttons already pressed even if a send failed or the
/// async caller dropped its spawn_blocking handle. Cleanup must ignore the
/// cancellation bit so modifiers cannot remain stuck after a timeout.
struct NativeInput<S: FnMut(EventType) -> Result<(), StepError>> {
    interrupt: StepInterrupt,
    sink: S,
    releases: Vec<EventType>,
}

impl<S: FnMut(EventType) -> Result<(), StepError>> NativeInput<S> {
    fn new(interrupt: StepInterrupt, sink: S) -> Self {
        Self {
            interrupt,
            sink,
            releases: Vec::new(),
        }
    }

    fn check(&self) -> Result<(), StepError> {
        if self.interrupt.is_interrupted() {
            Err(StepError::msg("desktop input cancelled"))
        } else {
            Ok(())
        }
    }

    fn event(&mut self, event: EventType) -> Result<(), StepError> {
        self.check()?;
        match event {
            // Track before sending: an OS API can partially deliver an event
            // before reporting failure, so a best-effort release is required.
            EventType::KeyPress(key) => self.releases.push(EventType::KeyRelease(key)),
            EventType::ButtonPress(button) => self.releases.push(EventType::ButtonRelease(button)),
            _ => {}
        }
        (self.sink)(event)?;
        if matches!(
            event,
            EventType::KeyRelease(_) | EventType::ButtonRelease(_)
        ) {
            if let Some(index) = self.releases.iter().rposition(|release| *release == event) {
                self.releases.remove(index);
            }
        }
        self.check()
    }

    fn click(
        &mut self,
        position: Option<(f64, f64)>,
        button: ClickButton,
        double: bool,
    ) -> Result<(), StepError> {
        if let Some((x, y)) = position {
            self.event(EventType::MouseMove { x, y })?;
        }
        for _ in 0..if double { 2 } else { 1 } {
            self.event(EventType::ButtonPress(button.to_rdev()))?;
            self.event(EventType::ButtonRelease(button.to_rdev()))?;
        }
        self.check()
    }

    fn combo(&mut self, modifiers: &[Key], main: Key) -> Result<(), StepError> {
        for modifier in modifiers {
            self.event(EventType::KeyPress(*modifier))?;
        }
        self.event(EventType::KeyPress(main))?;
        self.event(EventType::KeyRelease(main))?;
        for modifier in modifiers.iter().rev() {
            self.event(EventType::KeyRelease(*modifier))?;
        }
        self.check()
    }

    fn wait(&self, duration: Duration) -> Result<(), StepError> {
        let start = std::time::Instant::now();
        while start.elapsed() < duration {
            self.check()?;
            std::thread::sleep(
                duration
                    .saturating_sub(start.elapsed())
                    .min(Duration::from_millis(10)),
            );
        }
        self.check()
    }
}

impl<S: FnMut(EventType) -> Result<(), StepError>> Drop for NativeInput<S> {
    fn drop(&mut self) {
        for release in self.releases.drain(..).rev() {
            if let Err(error) = (self.sink)(release) {
                tracing::warn!("desktop input cleanup could not release held input: {error}");
            }
        }
    }
}

/// Shared guarded native click used by coordinate and OCR targeting actions.
/// Call only inside spawn_blocking; the OS sender includes short event pacing.
pub(crate) fn guarded_click(
    interrupt: StepInterrupt,
    position: Option<(f64, f64)>,
    button: ClickButton,
    double: bool,
) -> Result<(), StepError> {
    NativeInput::new(interrupt, send).click(position, button, double)
}

// ---------------------------------------------------------------------------
// desktop.move
// ---------------------------------------------------------------------------

pub struct MoveAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveIn {
    /// 目标 X(屏幕像素)。
    x: f64,
    /// 目标 Y(屏幕像素)。
    y: f64,
}

#[async_trait]
impl Action for MoveAction {
    fn id(&self) -> &'static str {
        "desktop.move"
    }
    fn summary(&self) -> &'static str {
        "Move the mouse pointer to absolute screen coordinates"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<MoveIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let MoveIn { x, y } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.move input invalid: {e}")))?;
        ctx.ensure_desktop("mouse")?;
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || {
            NativeInput::new(interrupt, send).event(EventType::MouseMove { x, y })
        })
        .await
        .map_err(|e| StepError::msg(format!("desktop.move join: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({ "x": x, "y": y })))
    }
}

// ---------------------------------------------------------------------------
// desktop.click
// ---------------------------------------------------------------------------

pub struct ClickAction;

/// 鼠标键(派生枚举 → schema 内联出 `["left","right","middle"]` 约束)。
/// `pub(crate)`:desktop_text(desktop.click_text)的 `button` 入参共用同一约束。
#[derive(Deserialize, JsonSchema, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ClickButton {
    #[default]
    Left,
    Right,
    Middle,
}

impl ClickButton {
    pub(crate) fn to_rdev(self) -> Button {
        match self {
            ClickButton::Left => Button::Left,
            ClickButton::Right => Button::Right,
            ClickButton::Middle => Button::Middle,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClickIn {
    /// 点击前移动到的 X;须与 `y` 同时给或同时省略(省略=当前位置)。
    #[serde(default)]
    x: Option<f64>,
    /// 点击前移动到的 Y。
    #[serde(default)]
    y: Option<f64>,
    /// 鼠标键:`left`(默认)/ `right` / `middle`。
    #[serde(default)]
    button: ClickButton,
    /// 是否双击。
    #[serde(default)]
    double: bool,
}

#[async_trait]
impl Action for ClickAction {
    fn id(&self) -> &'static str {
        "desktop.click"
    }
    fn summary(&self) -> &'static str {
        "Click a mouse button, optionally moving to coordinates first"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ClickIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ClickIn {
            x,
            y,
            button,
            double,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.click input invalid: {e}")))?;
        ctx.ensure_desktop("mouse")?;
        let pos = match (x, y) {
            (Some(x), Some(y)) => Some((x, y)),
            (None, None) => None,
            _ => {
                return Err(StepError::msg(
                    "desktop.click: provide both x and y, or neither".to_string(),
                ))
            }
        };
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || guarded_click(interrupt, pos, button, double))
            .await
            .map_err(|e| StepError::msg(format!("desktop.click join: {e}")))??;
        Ok(ActionResult::from(
            serde_json::json!({ "clicked": true, "double": double }),
        ))
    }
}

// ---------------------------------------------------------------------------
// desktop.scroll
// ---------------------------------------------------------------------------

pub struct ScrollAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScrollIn {
    /// 水平滚动量(正右负左)。
    #[serde(default)]
    dx: i64,
    /// 垂直滚动量(正下负上)。注:Linux 下 rdev 仅取符号,不看绝对值。
    #[serde(default)]
    dy: i64,
}

#[async_trait]
impl Action for ScrollAction {
    fn id(&self) -> &'static str {
        "desktop.scroll"
    }
    fn summary(&self) -> &'static str {
        "Scroll the mouse wheel by (dx, dy)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ScrollIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ScrollIn { dx, dy } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.scroll input invalid: {e}")))?;
        ctx.ensure_desktop("mouse")?;
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || {
            NativeInput::new(interrupt, send).event(EventType::Wheel {
                delta_x: dx,
                delta_y: dy,
            })
        })
        .await
        .map_err(|e| StepError::msg(format!("desktop.scroll join: {e}")))??;
        Ok(ActionResult::from(
            serde_json::json!({ "dx": dx, "dy": dy }),
        ))
    }
}

// ---------------------------------------------------------------------------
// desktop.key
// ---------------------------------------------------------------------------

pub struct KeyAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct KeyIn {
    /// 组合键,`+` 分隔。修饰键:`ctrl`/`control`、`shift`、`alt`/`option`、
    /// `cmd`/`command`/`meta`/`super`/`win`;主键:`a`-`z`、`0`-`9`、`f1`-`f12`、
    /// `enter`、`esc`、`tab`、`space`、`backspace`、`delete`、`up`/`down`/`left`/`right`
    /// 等。例:`ctrl+c`、`cmd+shift+t`、`enter`。
    keys: String,
}

#[async_trait]
impl Action for KeyAction {
    fn id(&self) -> &'static str {
        "desktop.key"
    }
    fn summary(&self) -> &'static str {
        "Press a key combination (e.g. ctrl+c, enter, cmd+shift+t)"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<KeyIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let KeyIn { keys } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.key input invalid: {e}")))?;
        ctx.ensure_desktop("keyboard")?;
        // 解析在闸门后、触发前 —— 非法组合键快速失败,不触 rdev。
        let (mods, main) = parse_combo(&keys)?;
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || NativeInput::new(interrupt, send).combo(&mods, main))
            .await
            .map_err(|e| StepError::msg(format!("desktop.key join: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({ "keys": keys })))
    }
}

/// 把 `"ctrl+shift+t"` 解析成(修饰键序列, 主键)。多个非修饰键或无主键都报错。
fn parse_combo(spec: &str) -> Result<(Vec<Key>, Key), StepError> {
    let mut mods = Vec::new();
    let mut main: Option<Key> = None;
    for raw in spec.split('+') {
        let tok = raw.trim().to_lowercase();
        if tok.is_empty() {
            continue;
        }
        if let Some(m) = parse_modifier(&tok) {
            mods.push(m);
        } else if let Some(k) = parse_key(&tok) {
            if main.replace(k).is_some() {
                return Err(StepError::msg(format!(
                    "desktop.key: more than one non-modifier key in `{spec}`"
                )));
            }
        } else {
            return Err(StepError::msg(format!(
                "desktop.key: unknown key token `{tok}` in `{spec}`"
            )));
        }
    }
    let main = main
        .ok_or_else(|| StepError::msg(format!("desktop.key: no non-modifier key in `{spec}`")))?;
    Ok((mods, main))
}

/// 修饰键 token → rdev Key(统一取左侧物理键)。
fn parse_modifier(tok: &str) -> Option<Key> {
    Some(match tok {
        "ctrl" | "control" => Key::ControlLeft,
        "shift" => Key::ShiftLeft,
        "alt" | "option" => Key::Alt,
        "cmd" | "command" | "meta" | "super" | "win" => Key::MetaLeft,
        _ => return None,
    })
}

/// 主键 token(已 lowercase)→ rdev Key。覆盖字母 / 数字 / 功能键 / 常用具名键。
fn parse_key(tok: &str) -> Option<Key> {
    use Key::*;
    Some(match tok {
        "a" => KeyA,
        "b" => KeyB,
        "c" => KeyC,
        "d" => KeyD,
        "e" => KeyE,
        "f" => KeyF,
        "g" => KeyG,
        "h" => KeyH,
        "i" => KeyI,
        "j" => KeyJ,
        "k" => KeyK,
        "l" => KeyL,
        "m" => KeyM,
        "n" => KeyN,
        "o" => KeyO,
        "p" => KeyP,
        "q" => KeyQ,
        "r" => KeyR,
        "s" => KeyS,
        "t" => KeyT,
        "u" => KeyU,
        "v" => KeyV,
        "w" => KeyW,
        "x" => KeyX,
        "y" => KeyY,
        "z" => KeyZ,
        "0" => Num0,
        "1" => Num1,
        "2" => Num2,
        "3" => Num3,
        "4" => Num4,
        "5" => Num5,
        "6" => Num6,
        "7" => Num7,
        "8" => Num8,
        "9" => Num9,
        "f1" => F1,
        "f2" => F2,
        "f3" => F3,
        "f4" => F4,
        "f5" => F5,
        "f6" => F6,
        "f7" => F7,
        "f8" => F8,
        "f9" => F9,
        "f10" => F10,
        "f11" => F11,
        "f12" => F12,
        "enter" | "return" => Return,
        "esc" | "escape" => Escape,
        "tab" => Tab,
        "space" => Space,
        "backspace" => Backspace,
        "delete" | "del" => Delete,
        "insert" | "ins" => Insert,
        "up" => UpArrow,
        "down" => DownArrow,
        "left" => LeftArrow,
        "right" => RightArrow,
        "home" => Home,
        "end" => End,
        "pageup" | "pgup" => PageUp,
        "pagedown" | "pgdn" => PageDown,
        "minus" | "-" => Minus,
        "comma" | "," => Comma,
        "dot" | "period" | "." => Dot,
        "slash" | "/" => Slash,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// desktop.type
// ---------------------------------------------------------------------------

pub struct TypeAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TypeIn {
    /// 要输入的文本(任意 Unicode / 中文);经剪贴板粘贴写入当前焦点控件。
    text: String,
}

#[async_trait]
impl Action for TypeAction {
    fn id(&self) -> &'static str {
        "desktop.type"
    }
    fn summary(&self) -> &'static str {
        "Type Unicode text into the focused field via clipboard paste"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<TypeIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let TypeIn { text } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.type input invalid: {e}")))?;
        ctx.ensure_desktop("keyboard")?;
        let len = text.chars().count();
        let interrupt = ctx.step_interrupt();
        tokio::task::spawn_blocking(move || paste_text(text, interrupt))
            .await
            .map_err(|e| StepError::msg(format!("desktop.type join: {e}")))??;
        Ok(ActionResult::from(serde_json::json!({ "typed": len })))
    }
}

/// 把文本写入剪贴板再模拟粘贴热键(macOS = Cmd+V,其余 = Ctrl+V)。中文/Unicode
/// 安全 —— rdev 逐键模拟无法产出非 ASCII,粘贴是唯一可靠的跨平台路径。
fn paste_text(text: String, interrupt: StepInterrupt) -> Result<(), StepError> {
    let mut input = NativeInput::new(interrupt, send);
    input.check()?;
    let mut cb = arboard::Clipboard::new()
        .map_err(|e| StepError::msg(format!("desktop.type: clipboard unavailable: {e}")))?;
    input.check()?;
    cb.set_text(text)
        .map_err(|e| StepError::msg(format!("desktop.type: clipboard write: {e}")))?;
    let modifier = if cfg!(target_os = "macos") {
        Key::MetaLeft
    } else {
        Key::ControlLeft
    };
    input.combo(&[modifier], Key::KeyV)
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn cancelled_before_start_never_sends_input_or_pastes() {
        let interrupt = StepInterrupt::default();
        interrupt.interrupt();
        let sent = RefCell::new(Vec::new());
        let mut input = NativeInput::new(interrupt.clone(), |event| {
            sent.borrow_mut().push(event);
            Ok(())
        });
        assert!(input.combo(&[Key::ControlLeft], Key::KeyV).is_err());
        assert!(input
            .event(EventType::Wheel {
                delta_x: 0,
                delta_y: 1
            })
            .is_err());
        assert!(paste_text("must never reach clipboard".into(), interrupt).is_err());
        drop(input);
        assert!(sent.borrow().is_empty());
    }

    #[test]
    fn cancelled_mid_combo_releases_modifier_without_pressing_main_key() {
        let interrupt = StepInterrupt::default();
        let sent = RefCell::new(Vec::new());
        {
            let mut input = NativeInput::new(interrupt.clone(), |event| {
                sent.borrow_mut().push(event);
                if event == EventType::KeyPress(Key::ControlLeft) {
                    interrupt.interrupt();
                }
                Ok(())
            });
            assert!(input.combo(&[Key::ControlLeft], Key::KeyV).is_err());
        }
        assert_eq!(
            *sent.borrow(),
            vec![
                EventType::KeyPress(Key::ControlLeft),
                EventType::KeyRelease(Key::ControlLeft)
            ]
        );
    }

    #[test]
    fn failed_press_releases_main_and_modifiers_in_reverse_order() {
        let sent = RefCell::new(Vec::new());
        {
            let mut input = NativeInput::new(StepInterrupt::default(), |event| {
                sent.borrow_mut().push(event);
                if event == EventType::KeyPress(Key::KeyA) {
                    Err(StepError::msg("OS reported partial input failure"))
                } else {
                    Ok(())
                }
            });
            assert!(input
                .combo(&[Key::ControlLeft, Key::ShiftLeft], Key::KeyA)
                .is_err());
        }
        assert_eq!(
            *sent.borrow(),
            vec![
                EventType::KeyPress(Key::ControlLeft),
                EventType::KeyPress(Key::ShiftLeft),
                EventType::KeyPress(Key::KeyA),
                EventType::KeyRelease(Key::KeyA),
                EventType::KeyRelease(Key::ShiftLeft),
                EventType::KeyRelease(Key::ControlLeft)
            ]
        );
    }

    #[test]
    fn cancelled_drag_releases_mouse_without_sending_more_movement() {
        let interrupt = StepInterrupt::default();
        let sent = RefCell::new(Vec::new());
        {
            let mut input = NativeInput::new(interrupt.clone(), |event| {
                sent.borrow_mut().push(event);
                if event == EventType::ButtonPress(Button::Left) {
                    interrupt.interrupt();
                }
                Ok(())
            });
            assert!(input.event(EventType::ButtonPress(Button::Left)).is_err());
            assert!(input
                .event(EventType::MouseMove { x: 100.0, y: 100.0 })
                .is_err());
        }
        assert_eq!(
            *sent.borrow(),
            vec![
                EventType::ButtonPress(Button::Left),
                EventType::ButtonRelease(Button::Left)
            ]
        );
    }

    #[test]
    fn cancelled_ocr_coordinate_click_never_presses_mouse() {
        let interrupt = StepInterrupt::default();
        let sent = RefCell::new(Vec::new());
        {
            let mut input = NativeInput::new(interrupt.clone(), |event| {
                sent.borrow_mut().push(event);
                if matches!(event, EventType::MouseMove { .. }) {
                    interrupt.interrupt();
                }
                Ok(())
            });
            assert!(input
                .click(Some((12.0, 34.0)), ClickButton::Left, true)
                .is_err());
        }
        assert_eq!(
            *sent.borrow(),
            vec![EventType::MouseMove { x: 12.0, y: 34.0 }]
        );
    }

    #[test]
    fn successful_combo_does_not_release_twice_on_drop() {
        let sent = RefCell::new(Vec::new());
        {
            let mut input = NativeInput::new(StepInterrupt::default(), |event| {
                sent.borrow_mut().push(event);
                Ok(())
            });
            input.combo(&[Key::MetaLeft], Key::KeyV).unwrap();
        }
        assert_eq!(
            *sent.borrow(),
            vec![
                EventType::KeyPress(Key::MetaLeft),
                EventType::KeyPress(Key::KeyV),
                EventType::KeyRelease(Key::KeyV),
                EventType::KeyRelease(Key::MetaLeft)
            ]
        );
    }
}
