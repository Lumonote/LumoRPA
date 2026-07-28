//! 指令集缺口 P0:原生截屏 + 窗口管理,仅在 feature `desktop` 下编译。
//!
//! 补齐纯桌面场景的图像驱动闭环:`desktop.screenshot → image.locate →
//! desktop.click`(此前 haystack 只能来自 browser.screenshot),以及坐标点击前的
//! 窗口前置/对位(窗口不在前台或位置漂移时,绝对坐标全错)。
//!
//! 动作(能力闸门沿用 `desktop`,新增类别 `screen` / `window`,与 mouse/keyboard 同构):
//! - `desktop.screenshot` —— 整屏或区域截图写 PNG(路径另过 `fs.write` 闸门,与
//!   browser.screenshot 落盘语义对齐);`display` 选显示器(0=主屏)。
//! - `window.list`     —— 枚举可见窗口(id/title/app/几何/minimized)。
//! - `window.activate` —— 按 `id` 或 `title_contains` 把窗口提到前台。
//! - `window.bounds`   —— 读窗口几何;可选 `set` 改位置/尺寸(平台能力见下表)。
//!
//! 选型:截屏/枚举用 xcap(一库覆盖三平台;**钉 =0.5.1**,0.6.0 起 edition 2024 需
//! rustc ≥1.85 > 本仓 MSRV 1.83,见 Cargo.toml 注释)。激活/改几何 xcap 不提供,按
//! 平台落地 —— macOS 走 osascript/System Events(同 lumo-recorder 的 AX 先例,免
//! objc 桥接);Windows 走 windows-sys 的 SetForegroundWindow/MoveWindow;Linux 仅
//! X11 best-effort(外呼 `wmctrl`,Wayland 无全局窗口协议)。
//!
//! 三平台支持矩阵:
//!
//! | 能力                | macOS                  | Windows                 | Linux                       |
//! |---------------------|------------------------|-------------------------|-----------------------------|
//! | desktop.screenshot  | ✅ CoreGraphics        | ✅ GDI                  | ✅ X11 / Wayland portal     |
//! | window.list         | ✅ CGWindowList        | ✅ EnumWindows          | ✅ X11(Wayland 受限)      |
//! | window.activate     | ✅ osascript(AX 授权)| ✅ SetForegroundWindow  | ⚠️ 需 `wmctrl`(仅 X11)    |
//! | window.bounds(读) | ✅                     | ✅                      | ✅ X11                      |
//! | window.bounds(set)| ✅ osascript(AX 授权)| ✅ MoveWindow           | ⚠️ 需 `wmctrl`(仅 X11)    |
//!
//! 做不到的平台显式报 "not supported on this platform"(或缺 wmctrl 的明确指引),
//! 绝不静默吞掉。macOS 首次截屏/读窗口标题需「屏幕录制」授权,osascript 操作窗口需
//! 「辅助功能」授权 —— 与 desktop.* 输入模拟、recorder 的授权提示一致。
//! 所有 xcap / 进程外呼都是同步阻塞,统一挪 `spawn_blocking`;CI/headless 无显示器,
//! 真实截屏/枚举用例标 `#[ignore]`,CI 只跑闸门 + 入参校验(在触 xcap 前即返回)。

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use xcap::{Monitor, Window};

pub fn register(r: &mut ActionRegistry) {
    r.register(ScreenshotAction);
    r.register(WindowListAction);
    r.register(WindowActivateAction);
    r.register(WindowBoundsAction);
    r.register(WindowCloseAction);
    r.register(WindowMinimizeAction);
    r.register(WindowMaximizeAction);
}

// ---------------------------------------------------------------------------
// desktop.screenshot
// ---------------------------------------------------------------------------

pub struct ScreenshotAction;

/// 截图区域(相对所选显示器图像的像素坐标,原点左上)。
/// `pub(crate)`:desktop_text(desktop.click_text)的 `region` 入参共用同一形状。
#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub(crate) struct Region {
    /// 区域左上角 X。
    pub(crate) x: u32,
    /// 区域左上角 Y。
    pub(crate) y: u32,
    /// 区域宽(像素,>0)。
    pub(crate) width: u32,
    /// 区域高(像素,>0)。
    pub(crate) height: u32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScreenshotIn {
    /// 目标 PNG 路径(过 fs.write 能力闸门;无论扩展名,始终写 PNG 编码)。
    path: String,
    /// 可选截取区域;省略=整屏。
    #[serde(default)]
    region: Option<Region>,
    /// 显示器索引,0(默认)=主屏,其余按系统枚举顺序。
    #[serde(default)]
    display: usize,
}

#[async_trait]
impl Action for ScreenshotAction {
    fn id(&self) -> &'static str {
        "desktop.screenshot"
    }
    fn summary(&self) -> &'static str {
        "Capture the screen (or a region) of a display to a PNG file"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<ScreenshotIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let ScreenshotIn {
            path,
            region,
            display,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("desktop.screenshot input invalid: {e}")))?;
        // 闸门先行(screen 类别 + 落盘路径),未授权时在触 xcap/显示器前快速失败 ——
        // 与 browser.screenshot 的 "gate before work" 顺序一致,也让 CI 无显示器可测。
        ctx.ensure_desktop("screen")?;
        let dest = PathBuf::from(&path);
        ctx.ensure_fs_write(&dest)?;
        if let Some(r) = region {
            if r.width == 0 || r.height == 0 {
                return Err(StepError::msg(
                    "desktop.screenshot: region width/height must be > 0".to_string(),
                ));
            }
        }
        let (width, height, png) = tokio::task::spawn_blocking(move || {
            let img = capture_display(display)?;
            let img = match region {
                Some(r) => {
                    if r.x.saturating_add(r.width) > img.width()
                        || r.y.saturating_add(r.height) > img.height()
                    {
                        return Err(StepError::msg(format!(
                            "desktop.screenshot: region exceeds display bounds ({}x{})",
                            img.width(),
                            img.height()
                        )));
                    }
                    image::imageops::crop_imm(&img, r.x, r.y, r.width, r.height).to_image()
                }
                None => img,
            };
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // 先编码进内存再落盘:同一份 PNG 字节既写用户目标路径,又供下方 X-07
            // artifact 归档复用,避免「写完再回读」的双重 IO 与竞态。
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| StepError::msg(format!("desktop.screenshot encode: {e}")))?;
            std::fs::write(&dest, &png).map_err(|e| {
                StepError::msg(format!("desktop.screenshot write {}: {e}", dest.display()))
            })?;
            Ok::<(u32, u32, Vec<u8>), StepError>((img.width(), img.height(), png))
        })
        .await
        .map_err(|e| StepError::msg(format!("desktop.screenshot join: {e}")))??;
        // X-07 时光回溯:与 browser.screenshot 同语义 —— 截图归档为 run 级
        // artifact(kind=screenshot / mime=image/png);no-op / 失败 → null。
        let artifact_id = crate::attach_artifact_lenient(
            ctx,
            "desktop.screenshot",
            "screenshot",
            "image/png",
            &png,
        );
        Ok(ActionResult::from(json!({
            "path": path,
            "width": width,
            "height": height,
            "artifact_id": artifact_id,
        })))
    }
}

/// 抓取第 `display` 个显示器的整屏图像。排序保证主屏恒为索引 0(系统枚举顺序在
/// 多屏热插拔下不稳定,主屏置前让默认值语义可预期)。
fn capture_display(display: usize) -> Result<image::RgbaImage, StepError> {
    let monitor = select_monitor("desktop.screenshot", display)?;
    monitor
        .capture_image()
        .map_err(|e| StepError::msg(format!("desktop.screenshot: capture failed: {e}")))
}

/// 选第 `display` 个显示器(主屏恒为索引 0)。`pub(crate)`:desktop_text
/// (desktop.click_text)需要显示器几何(逻辑坐标原点 / 尺寸)做像素 → 屏幕坐标换算,
/// 不能只拿图像。
pub(crate) fn select_monitor(action: &str, display: usize) -> Result<Monitor, StepError> {
    let mut monitors =
        Monitor::all().map_err(|e| StepError::msg(format!("{action}: enumerate displays: {e}")))?;
    // 稳定排序:主屏排前,其余保持枚举序。
    monitors.sort_by_key(|m| !m.is_primary().unwrap_or(false));
    let count = monitors.len();
    if count == 0 {
        // 实测:无窗口服务器连接(headless 会话)或宿主进程未获「屏幕录制」授权时,
        // 枚举为空 —— 给出可行动的提示而不是裸的 "index out of range"。
        return Err(StepError::msg(format!(
            "{action}: no displays detected (headless session, or missing \
             Screen Recording permission on macOS)"
        )));
    }
    monitors.into_iter().nth(display).ok_or_else(|| {
        StepError::msg(format!(
            "{action}: display index {display} out of range ({count} display(s))"
        ))
    })
}

// ---------------------------------------------------------------------------
// 窗口选取(window.activate / window.bounds 共用)
// ---------------------------------------------------------------------------

/// 选中的窗口快照(xcap 的 getter 全是 Result,统一在此一次性收敛成普通值;个别
/// 字段读取失败按空/0 兜底 —— 枚举型语义下单字段失败不应整步失败)。
struct WinInfo {
    id: u32,
    pid: u32,
    title: String,
    app: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    minimized: bool,
}

fn win_info(w: &Window) -> WinInfo {
    WinInfo {
        id: w.id().unwrap_or(0),
        pid: w.pid().unwrap_or(0),
        title: w.title().unwrap_or_default(),
        app: w.app_name().unwrap_or_default(),
        x: w.x().unwrap_or(0),
        y: w.y().unwrap_or(0),
        width: w.width().unwrap_or(0),
        height: w.height().unwrap_or(0),
        minimized: w.is_minimized().unwrap_or(false),
    }
}

/// 按 `id` 或 `title_contains`(忽略大小写的子串)选窗口。返回 (第一个命中, 命中数);
/// 零命中报错。调用方负责保证两个条件恰好给了一个(入参校验先于本函数,CI 可测)。
fn pick_window(
    action: &str,
    id: Option<u32>,
    title_contains: Option<&str>,
) -> Result<(WinInfo, usize), StepError> {
    let windows =
        Window::all().map_err(|e| StepError::io(format!("{action}: enumerate windows: {e}")))?;
    let infos: Vec<WinInfo> = windows.iter().map(win_info).collect();
    let matched: Vec<WinInfo> = match (id, title_contains) {
        (Some(id), _) => infos.into_iter().filter(|w| w.id == id).collect(),
        (None, Some(needle)) => {
            let needle = needle.to_lowercase();
            infos
                .into_iter()
                .filter(|w| w.title.to_lowercase().contains(&needle))
                .collect()
        }
        (None, None) => unreachable!("validated by caller"),
    };
    let count = matched.len();
    let first = matched.into_iter().next().ok_or_else(|| {
        let what = match (id, title_contains) {
            (Some(id), _) => format!("id {id}"),
            (_, Some(t)) => format!("title containing `{t}`"),
            _ => unreachable!(),
        };
        // 归类为 selector_not_found:与 browser/desktop_text 的「目标未命中」同款
        // typed 错误,`retry.on: [selector_not_found]` 开箱可用。
        StepError::SelectorNotFound(format!("{action}: no window matched {what}"))
    })?;
    Ok((first, count))
}

/// `id` 与 `title_contains` 必须恰好给一个(都给以谁为准会歧义,都不给无从选)。
fn ensure_one_selector(
    action: &str,
    id: Option<u32>,
    title_contains: Option<&String>,
) -> Result<(), StepError> {
    match (id, title_contains) {
        (Some(_), None) | (None, Some(_)) => Ok(()),
        _ => Err(StepError::msg(format!(
            "{action}: provide exactly one of `id` or `title_contains`"
        ))),
    }
}

// ---------------------------------------------------------------------------
// window.list
// ---------------------------------------------------------------------------

pub struct WindowListAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WindowListIn {}

#[async_trait]
impl Action for WindowListAction {
    fn id(&self) -> &'static str {
        "window.list"
    }
    fn summary(&self) -> &'static str {
        "List visible desktop windows with id, title, app and geometry"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WindowListIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WindowListIn {} = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("window.list input invalid: {e}")))?;
        ctx.ensure_desktop("window")?;
        let windows = tokio::task::spawn_blocking(|| {
            let windows = Window::all()
                .map_err(|e| StepError::io(format!("window.list: enumerate windows: {e}")))?;
            Ok::<Vec<Value>, StepError>(windows.iter().map(|w| win_json(&win_info(w))).collect())
        })
        .await
        .map_err(|e| StepError::msg(format!("window.list join: {e}")))??;
        Ok(ActionResult::from(json!({
            "count": windows.len(),
            "windows": windows,
        })))
    }
}

fn win_json(w: &WinInfo) -> Value {
    json!({
        "id": w.id,
        "title": w.title,
        "app": w.app,
        "x": w.x,
        "y": w.y,
        "width": w.width,
        "height": w.height,
        "minimized": w.minimized,
    })
}

// ---------------------------------------------------------------------------
// window.activate
// ---------------------------------------------------------------------------

pub struct WindowActivateAction;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WindowActivateIn {
    /// 目标窗口 id(来自 window.list);与 `title_contains` 二选一。
    #[serde(default)]
    id: Option<u32>,
    /// 标题子串匹配(忽略大小写);命中多个取第一个,输出回报 `matched` 数。
    #[serde(default)]
    title_contains: Option<String>,
}

#[async_trait]
impl Action for WindowActivateAction {
    fn id(&self) -> &'static str {
        "window.activate"
    }
    fn summary(&self) -> &'static str {
        "Bring a window to the foreground by id or title substring"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WindowActivateIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WindowActivateIn { id, title_contains } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("window.activate input invalid: {e}")))?;
        ctx.ensure_desktop("window")?;
        ensure_one_selector("window.activate", id, title_contains.as_ref())?;
        let out = tokio::task::spawn_blocking(move || {
            let (win, matched) = pick_window("window.activate", id, title_contains.as_deref())?;
            platform::activate(&win)?;
            Ok::<Value, StepError>(json!({
                "activated": true,
                "id": win.id,
                "title": win.title,
                "app": win.app,
                "matched": matched,
            }))
        })
        .await
        .map_err(|e| StepError::msg(format!("window.activate join: {e}")))??;
        Ok(ActionResult::from(out))
    }
}

// ---------------------------------------------------------------------------
// window.bounds
// ---------------------------------------------------------------------------

pub struct WindowBoundsAction;

/// 期望几何(屏幕绝对坐标)。
#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(deny_unknown_fields)]
struct BoundsSet {
    /// 窗口左上角 X。
    x: i32,
    /// 窗口左上角 Y。
    y: i32,
    /// 窗口宽(像素,>0)。
    width: u32,
    /// 窗口高(像素,>0)。
    height: u32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WindowBoundsIn {
    /// 目标窗口 id(来自 window.list);与 `title_contains` 二选一。
    #[serde(default)]
    id: Option<u32>,
    /// 标题子串匹配(忽略大小写);命中多个取第一个,输出回报 `matched` 数。
    #[serde(default)]
    title_contains: Option<String>,
    /// 可选:把窗口移动/缩放到该几何。平台能力见模块头矩阵,做不到的平台显式报错。
    #[serde(default)]
    set: Option<BoundsSet>,
}

#[async_trait]
impl Action for WindowBoundsAction {
    fn id(&self) -> &'static str {
        "window.bounds"
    }
    fn summary(&self) -> &'static str {
        "Read a window's geometry, optionally moving/resizing it first"
    }
    fn schema(&self) -> &'static serde_json::Value {
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<WindowBoundsIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let WindowBoundsIn {
            id,
            title_contains,
            set,
        } = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("window.bounds input invalid: {e}")))?;
        ctx.ensure_desktop("window")?;
        ensure_one_selector("window.bounds", id, title_contains.as_ref())?;
        if let Some(s) = set {
            if s.width == 0 || s.height == 0 {
                return Err(StepError::msg(
                    "window.bounds: set.width/set.height must be > 0".to_string(),
                ));
            }
        }
        let out = tokio::task::spawn_blocking(move || {
            let (mut win, matched) = pick_window("window.bounds", id, title_contains.as_deref())?;
            if let Some(s) = set {
                platform::set_bounds(&win, s)?;
                // set 之后重读几何(WM 可能吸附/夹取实际值);窗口若在间隙中消失,
                // 保守回报 set 请求值而不是报错 —— set 本身已成功。
                if let Ok((requeried, _)) = pick_window("window.bounds", Some(win.id), None) {
                    win = requeried;
                } else {
                    win.x = s.x;
                    win.y = s.y;
                    win.width = s.width;
                    win.height = s.height;
                }
            }
            Ok::<Value, StepError>(json!({
                "id": win.id,
                "title": win.title,
                "app": win.app,
                "x": win.x,
                "y": win.y,
                "width": win.width,
                "height": win.height,
                "matched": matched,
                "set": set.is_some(),
            }))
        })
        .await
        .map_err(|e| StepError::msg(format!("window.bounds join: {e}")))??;
        Ok(ActionResult::from(out))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WindowControlIn {
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    title_contains: Option<String>,
}

#[derive(Clone, Copy)]
enum WindowControlKind {
    Close,
    Minimize,
    Maximize,
}

async fn execute_window_control(
    ctx: &mut StepCtx,
    input: Value,
    action: &'static str,
    kind: WindowControlKind,
) -> Result<ActionResult, StepError> {
    let WindowControlIn { id, title_contains } = serde_json::from_value(input)
        .map_err(|e| StepError::msg(format!("{action} input invalid: {e}")))?;
    ctx.ensure_desktop("window")?;
    ensure_one_selector(action, id, title_contains.as_ref())?;
    let out = tokio::task::spawn_blocking(move || {
        let (win, matched) = pick_window(action, id, title_contains.as_deref())?;
        match kind {
            WindowControlKind::Close => platform::close(&win)?,
            WindowControlKind::Minimize => platform::minimize(&win)?,
            WindowControlKind::Maximize => platform::maximize(&win)?,
        }
        Ok::<Value, StepError>(json!({
            "ok": true,
            "id": win.id,
            "title": win.title,
            "app": win.app,
            "matched": matched,
        }))
    })
    .await
    .map_err(|e| StepError::msg(format!("{action} join: {e}")))??;
    Ok(ActionResult::from(out))
}

macro_rules! window_control_action {
    ($name:ident, $id:literal, $summary:literal, $kind:ident) => {
        pub struct $name;
        #[async_trait]
        impl Action for $name {
            fn id(&self) -> &'static str {
                $id
            }
            fn summary(&self) -> &'static str {
                $summary
            }
            fn schema(&self) -> &'static Value {
                static S: Lazy<Value> = Lazy::new(crate::schema::derive::<WindowControlIn>);
                &S
            }
            async fn execute(
                &self,
                ctx: &mut StepCtx,
                input: Value,
            ) -> Result<ActionResult, StepError> {
                execute_window_control(ctx, input, self.id(), WindowControlKind::$kind).await
            }
        }
    };
}

window_control_action!(WindowCloseAction, "window.close", "Close a window", Close);
window_control_action!(
    WindowMinimizeAction,
    "window.minimize",
    "Minimize a window",
    Minimize
);
window_control_action!(
    WindowMaximizeAction,
    "window.maximize",
    "Maximize a window",
    Maximize
);

// ---------------------------------------------------------------------------
// 平台实现:激活 / 改几何(xcap 只读不写,这两件事必须各平台自己来)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform {
    //! macOS:osascript + System Events(AX)。沿用 lumo-recorder 的先例 —— 元数据
    //! 级窗口操作用一次性 osascript 即可,不值得为此引 objc2 桥接;需要用户授予
    //! 「辅助功能」权限(OS 自己会弹引导,失败时把 stderr 透传给用户)。

    use super::{BoundsSet, WinInfo};
    use lumo_core::error::StepError;

    pub fn activate(win: &WinInfo) -> Result<(), StepError> {
        // 按 pid 把整个进程置前(CGWindowID 无法直接喂给 System Events;对 RPA
        // 「把目标应用拉到前台再点」的诉求,进程级 frontmost 已是正确语义)。
        let script = format!(
            "tell application \"System Events\" to set frontmost of \
             (first application process whose unix id is {}) to true",
            win.pid
        );
        run_osascript("window.activate", &script).map(|_| ())
    }

    pub fn set_bounds(win: &WinInfo, s: BoundsSet) -> Result<(), StepError> {
        // 进程内按标题精确选窗,标题空/未命中时退到前窗(单窗应用的常态)。
        let title = escape_applescript(&win.title);
        let script = format!(
            r#"tell application "System Events"
  set p to first application process whose unix id is {pid}
  try
    set w to first window of p whose name is "{title}"
  on error
    set w to front window of p
  end try
  set position of w to {{{x}, {y}}}
  set size of w to {{{width}, {height}}}
end tell"#,
            pid = win.pid,
            title = title,
            x = s.x,
            y = s.y,
            width = s.width,
            height = s.height,
        );
        run_osascript("window.bounds", &script).map(|_| ())
    }

    pub fn close(win: &WinInfo) -> Result<(), StepError> {
        run_window_action("window.close", win, "perform action \"AXClose\" of w")
    }

    pub fn minimize(win: &WinInfo) -> Result<(), StepError> {
        run_window_action(
            "window.minimize",
            win,
            "set value of attribute \"AXMinimized\" of w to true",
        )
    }

    pub fn maximize(win: &WinInfo) -> Result<(), StepError> {
        run_window_action(
            "window.maximize",
            win,
            "perform action \"AXPress\" of (first button of w whose subrole is \"AXZoomButton\")",
        )
    }

    fn run_window_action(action: &str, win: &WinInfo, operation: &str) -> Result<(), StepError> {
        let title = escape_applescript(&win.title);
        let script = format!(
            r#"tell application "System Events"
  set p to first application process whose unix id is {pid}
  try
    set w to first window of p whose name is "{title}"
  on error
    set w to front window of p
  end try
  {operation}
end tell"#,
            pid = win.pid,
        );
        run_osascript(action, &script).map(|_| ())
    }

    /// AppleScript 字符串字面量转义(反斜杠与双引号)。标题来自任意应用,不可信。
    fn escape_applescript(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn run_osascript(action: &str, script: &str) -> Result<String, StepError> {
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| StepError::io(format!("{action}: spawn osascript: {e}")))?;
        if !out.status.success() {
            // 未授予「辅助功能」时 osascript 以非零退出 + stderr 说明,原样透传。
            // 归类为 io:本机进程/权限层失败(permission),`retry.on: [io]` 可控。
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(StepError::io(format!(
                "{action}: osascript failed: {}",
                err.trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    //! Windows:windows-sys 直调 Win32(xcap 在 Windows 上的窗口 id 即 HWND 值)。

    use super::{BoundsSet, WinInfo};
    use lumo_core::error::StepError;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MoveWindow, PostMessageW, SetForegroundWindow, ShowWindow, SW_MAXIMIZE, SW_MINIMIZE,
        SW_RESTORE, WM_CLOSE,
    };

    fn hwnd(win: &WinInfo) -> HWND {
        win.id as usize as HWND
    }

    pub fn activate(win: &WinInfo) -> Result<(), StepError> {
        // 先还原(最小化窗口 SetForegroundWindow 不会自己展开),再置前。
        // SetForegroundWindow 受前台锁定策略限制(用户正在交互时 OS 仅闪任务栏),
        // 返回 0 时如实报错而不是假装成功。
        unsafe {
            ShowWindow(hwnd(win), SW_RESTORE);
            if SetForegroundWindow(hwnd(win)) == 0 {
                return Err(StepError::io(
                    "window.activate: SetForegroundWindow refused (foreground lock)".to_string(),
                ));
            }
        }
        Ok(())
    }

    pub fn set_bounds(win: &WinInfo, s: BoundsSet) -> Result<(), StepError> {
        unsafe {
            if MoveWindow(hwnd(win), s.x, s.y, s.width as i32, s.height as i32, 1) == 0 {
                return Err(StepError::io(format!(
                    "window.bounds: MoveWindow failed (os error {})",
                    std::io::Error::last_os_error()
                )));
            }
        }
        Ok(())
    }

    pub fn close(win: &WinInfo) -> Result<(), StepError> {
        unsafe {
            if PostMessageW(hwnd(win), WM_CLOSE, 0, 0) == 0 {
                return Err(StepError::io(format!(
                    "window.close: PostMessageW failed (os error {})",
                    std::io::Error::last_os_error()
                )));
            }
        }
        Ok(())
    }

    pub fn minimize(win: &WinInfo) -> Result<(), StepError> {
        unsafe {
            ShowWindow(hwnd(win), SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn maximize(win: &WinInfo) -> Result<(), StepError> {
        unsafe {
            ShowWindow(hwnd(win), SW_MAXIMIZE);
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! Linux:仅 X11 best-effort,外呼 `wmctrl`(EWMH 客户端,主流发行版均有包)。
    //! 不直连 X 协议 —— 仅为激活/改几何引 xcb 窗口管理子协议不值得;Wayland 没有
    //! 跨合成器的全局窗口控制协议,统一以「缺 wmctrl / 非 X11」的明确报错呈现。

    use super::{BoundsSet, WinInfo};
    use lumo_core::error::StepError;

    pub fn activate(win: &WinInfo) -> Result<(), StepError> {
        run_wmctrl("window.activate", &["-i", "-a", &format!("0x{:x}", win.id)])
    }

    pub fn set_bounds(win: &WinInfo, s: BoundsSet) -> Result<(), StepError> {
        run_wmctrl(
            "window.bounds",
            &[
                "-i",
                "-r",
                &format!("0x{:x}", win.id),
                "-e",
                // gravity 0 = 交给 WM 默认;其后依次 x,y,width,height。
                &format!("0,{},{},{},{}", s.x, s.y, s.width, s.height),
            ],
        )
    }

    pub fn close(win: &WinInfo) -> Result<(), StepError> {
        run_wmctrl("window.close", &["-i", "-c", &format!("0x{:x}", win.id)])
    }

    pub fn minimize(win: &WinInfo) -> Result<(), StepError> {
        run_wmctrl(
            "window.minimize",
            &["-i", "-r", &format!("0x{:x}", win.id), "-b", "add,hidden"],
        )
    }

    pub fn maximize(win: &WinInfo) -> Result<(), StepError> {
        run_wmctrl(
            "window.maximize",
            &[
                "-i",
                "-r",
                &format!("0x{:x}", win.id),
                "-b",
                "add,maximized_vert,maximized_horz",
            ],
        )
    }

    fn run_wmctrl(action: &str, args: &[&str]) -> Result<(), StepError> {
        let out = std::process::Command::new("wmctrl")
            .args(args)
            .output()
            .map_err(|e| {
                StepError::io(format!(
                    "{action}: not supported on this platform without `wmctrl` (X11 only): {e}"
                ))
            })?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(StepError::io(format!(
                "{action}: wmctrl failed: {}",
                err.trim()
            )));
        }
        Ok(())
    }
}

/// 其余平台(理论上 desktop feature 不会在此类目标启用;留显式报错兜底)。
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::{BoundsSet, WinInfo};
    use lumo_core::error::StepError;

    pub fn activate(_win: &WinInfo) -> Result<(), StepError> {
        Err(StepError::msg(
            "window.activate: not supported on this platform".to_string(),
        ))
    }

    pub fn set_bounds(_win: &WinInfo, _s: BoundsSet) -> Result<(), StepError> {
        Err(StepError::msg(
            "window.bounds: set is not supported on this platform".to_string(),
        ))
    }

    pub fn close(_win: &WinInfo) -> Result<(), StepError> {
        Err(StepError::msg(
            "window.close: not supported on this platform",
        ))
    }
    pub fn minimize(_win: &WinInfo) -> Result<(), StepError> {
        Err(StepError::msg(
            "window.minimize: not supported on this platform",
        ))
    }
    pub fn maximize(_win: &WinInfo) -> Result<(), StepError> {
        Err(StepError::msg(
            "window.maximize: not supported on this platform",
        ))
    }
}
