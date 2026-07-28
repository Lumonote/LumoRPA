//! F-1 `desktop.*` coverage. The CI-runnable tests exercise only the capability
//! gate + input validation — each returns at the gate or parse stage, **before**
//! `spawn_blocking`/`rdev::simulate`, so running them never moves the mouse or
//! types. Real actuation is `#[ignore]` (needs a display + macOS Accessibility
//! grant), mirroring the browser e2e convention.
#![cfg(feature = "desktop")]

mod common;
use common::{ok_with, run, run_with, Capabilities};
use serde_json::json;

/// Grant every desktop category (`*`).
fn desktop_caps() -> Capabilities {
    Capabilities {
        desktop: vec!["*".into()],
        ..Default::default()
    }
}

// ─── capability gate (CI; fail before touching rdev) ────────────────────────

#[tokio::test]
async fn move_denies_without_desktop_grant() {
    let err = run("desktop.move", json!({ "x": 10.0, "y": 20.0 }))
        .await
        .unwrap_err();
    assert!(err.contains("capability denied"), "got: {err}");
    assert!(err.contains("desktop"), "got: {err}");
}

#[tokio::test]
async fn click_denies_without_desktop_grant() {
    let err = run("desktop.click", json!({ "x": 1.0, "y": 2.0 }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

#[tokio::test]
async fn type_denies_without_desktop_grant() {
    let err = run("desktop.type", json!({ "text": "你好" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// Category granularity: a `mouse`-only grant must not unlock keyboard actions.
#[tokio::test]
async fn keyboard_action_denied_with_only_mouse_grant() {
    let caps = Capabilities {
        desktop: vec!["mouse".into()],
        ..Default::default()
    };
    let err = run_with("desktop.key", json!({ "keys": "ctrl+c" }), caps)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

// ─── input validation (CI; granted, but fail before actuation) ──────────────

#[tokio::test]
async fn click_rejects_unknown_field() {
    let err = run_with(
        "desktop.click",
        json!({ "x": 1.0, "y": 2.0, "bogus": true }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn click_rejects_one_sided_coordinates() {
    // x without y is ambiguous → rejected before any actuation.
    let err = run_with("desktop.click", json!({ "x": 5.0 }), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("both x and y"), "got: {err}");
}

#[tokio::test]
async fn drag_rejects_zero_duration_before_actuation() {
    let err = run_with(
        "desktop.drag",
        json!({"from_x": 0.0, "from_y": 0.0, "to_x": 10.0, "to_y": 10.0, "duration_ms": 0}),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("duration_ms") && err.contains(">= 1"),
        "got: {err}"
    );
}

#[tokio::test]
async fn key_rejects_unknown_token() {
    let err = run_with(
        "desktop.key",
        json!({ "keys": "ctrl+nope" }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown key token"), "got: {err}");
}

#[tokio::test]
async fn key_rejects_modifier_only_combo() {
    let err = run_with("desktop.key", json!({ "keys": "ctrl+" }), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("no non-modifier key"), "got: {err}");
}

#[tokio::test]
async fn type_rejects_unknown_field() {
    let err = run_with(
        "desktop.type",
        json!({ "text": "hi", "bogus": 1 }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

// ─── actuation (e2e; #[ignore] — needs a display + Accessibility grant) ──────

#[tokio::test]
#[ignore = "actuates real input; needs a display + macOS Accessibility grant"]
async fn move_actuates() {
    let out = ok_with(
        "desktop.move",
        json!({ "x": 100.0, "y": 100.0 }),
        desktop_caps(),
    )
    .await;
    assert_eq!(out.get("x").and_then(|v| v.as_f64()), Some(100.0));
}

#[tokio::test]
#[ignore = "actuates real input; needs a display + macOS Accessibility grant"]
async fn type_actuates_unicode() {
    // "你好 hello" = 8 Unicode scalar values; clipboard-paste handles the CJK.
    let out = ok_with(
        "desktop.type",
        json!({ "text": "你好 hello" }),
        desktop_caps(),
    )
    .await;
    assert_eq!(out.get("typed").and_then(|v| v.as_u64()), Some(8));
}

// ─── desktop.screenshot / window.*(P0 缺口:截屏 + 窗口管理) ────────────────
//
// 与上面同一约定:CI 只跑能力闸门 + 入参校验(全部在触 xcap/显示器之前返回),
// 真实截屏/枚举标 #[ignore](需显示器;macOS 另需「屏幕录制」授权)。

/// desktop 全类别 + 限定目录的 fs.write(desktop.screenshot 落盘要双闸门)。
fn screen_caps(dir: &std::path::Path) -> Capabilities {
    Capabilities {
        desktop: vec!["*".into()],
        fs_write: vec![format!("{}/**", dir.display())],
        ..Default::default()
    }
}

#[tokio::test]
async fn screenshot_denies_without_desktop_grant() {
    let err = run("desktop.screenshot", json!({ "path": "/tmp/x.png" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// 类别粒度:mouse 授权不解锁 screen。
#[tokio::test]
async fn screenshot_denied_with_only_mouse_grant() {
    let caps = Capabilities {
        desktop: vec!["mouse".into()],
        ..Default::default()
    };
    let err = run_with("desktop.screenshot", json!({ "path": "/tmp/x.png" }), caps)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// screen 已授权但落盘路径未授 fs.write —— 第二道闸门必须独立拦住。
#[tokio::test]
async fn screenshot_denies_ungranted_destination() {
    let err = run_with(
        "desktop.screenshot",
        json!({ "path": "/definitely/not/granted/x.png" }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("fs.write"),
        "got: {err}"
    );
}

#[tokio::test]
async fn screenshot_rejects_zero_size_region() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shot.png");
    let err = run_with(
        "desktop.screenshot",
        json!({
            "path": path.to_string_lossy(),
            "region": { "x": 0, "y": 0, "width": 0, "height": 10 },
        }),
        screen_caps(dir.path()),
    )
    .await
    .unwrap_err();
    assert!(err.contains("width/height must be > 0"), "got: {err}");
}

#[tokio::test]
async fn screenshot_rejects_unknown_field() {
    let err = run_with(
        "desktop.screenshot",
        json!({ "path": "/tmp/x.png", "bogus": 1 }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn window_list_denies_without_window_grant() {
    let err = run("window.list", json!({})).await.unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// 类别粒度:keyboard 授权不解锁 window。
#[tokio::test]
async fn window_action_denied_with_only_keyboard_grant() {
    let caps = Capabilities {
        desktop: vec!["keyboard".into()],
        ..Default::default()
    };
    let err = run_with("window.activate", json!({ "id": 1 }), caps)
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

#[tokio::test]
async fn window_activate_requires_exactly_one_selector() {
    // 都不给 → 无从选。
    let err = run_with("window.activate", json!({}), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("exactly one"), "got: {err}");
    // 都给 → 歧义,同样拒绝。
    let err = run_with(
        "window.activate",
        json!({ "id": 1, "title_contains": "x" }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("exactly one"), "got: {err}");
}

#[tokio::test]
async fn window_bounds_requires_exactly_one_selector() {
    let err = run_with("window.bounds", json!({}), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("exactly one"), "got: {err}");
}

#[tokio::test]
async fn window_control_actions_require_exactly_one_selector() {
    for id in ["window.close", "window.minimize", "window.maximize"] {
        let err = run_with(id, json!({}), desktop_caps()).await.unwrap_err();
        assert!(err.contains("exactly one"), "{id}: {err}");
    }
}

#[tokio::test]
async fn window_bounds_rejects_zero_size_set() {
    let err = run_with(
        "window.bounds",
        json!({ "id": 1, "set": { "x": 0, "y": 0, "width": 0, "height": 100 } }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("must be > 0"), "got: {err}");
}

#[tokio::test]
async fn window_list_rejects_unknown_field() {
    let err = run_with("window.list", json!({ "bogus": 1 }), desktop_caps())
        .await
        .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

// ─── 真实截屏/窗口枚举(e2e; #[ignore] — 需显示器 + macOS 屏幕录制授权) ──────

#[tokio::test]
#[ignore = "captures the real screen; needs a display + macOS Screen Recording grant"]
async fn screenshot_captures_primary_display() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shot.png");
    let out = ok_with(
        "desktop.screenshot",
        json!({ "path": path.to_string_lossy() }),
        screen_caps(dir.path()),
    )
    .await;
    assert!(path.exists(), "png not written");
    assert!(out.get("width").and_then(|v| v.as_u64()).unwrap_or(0) > 0);
    assert!(out.get("height").and_then(|v| v.as_u64()).unwrap_or(0) > 0);
}

#[tokio::test]
#[ignore = "captures the real screen; needs a display + macOS Screen Recording grant"]
async fn screenshot_region_is_cropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("region.png");
    let out = ok_with(
        "desktop.screenshot",
        json!({
            "path": path.to_string_lossy(),
            "region": { "x": 0, "y": 0, "width": 64, "height": 32 },
        }),
        screen_caps(dir.path()),
    )
    .await;
    assert_eq!(out.get("width").and_then(|v| v.as_u64()), Some(64));
    assert_eq!(out.get("height").and_then(|v| v.as_u64()), Some(32));
}

#[tokio::test]
#[ignore = "enumerates real windows; needs a display + macOS Screen Recording grant"]
async fn window_list_returns_windows() {
    let out = ok_with("window.list", json!({}), desktop_caps()).await;
    let count = out.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(count > 0, "expected at least one visible window: {out}");
    let first = &out["windows"][0];
    assert!(first.get("id").is_some() && first.get("title").is_some());
}

// ─── desktop.click_text(P1:OCR 文本定位点击) ────────────────────────────────
//
// 同一约定:CI 只跑能力闸门 + 入参校验 + provider 缺失(全部在触 xcap/rdev 之前
// 返回);纯函数(解析/匹配/坐标换算)在 src/desktop_text.rs 内联单测覆盖。
// 真截屏 + 真点击不进测试(需授权且会动鼠标)。

/// screen + llm,但**无 mouse**:dry_run 的最小授权面。
fn click_text_dry_caps() -> Capabilities {
    Capabilities {
        desktop: vec!["screen".into()],
        llm: vec!["*".into()],
        ..Default::default()
    }
}

#[tokio::test]
async fn click_text_denies_without_desktop_grant() {
    let err = run("desktop.click_text", json!({ "text": "登录" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// 真点击需要 screen + mouse 双类别:只授 screen 必须被 mouse 闸门拦下。
#[tokio::test]
async fn click_text_requires_mouse_for_real_click() {
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "登录" }),
        click_text_dry_caps(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("capability denied") && err.contains("desktop"),
        "got: {err}"
    );
}

/// dry_run 免 mouse:只授 screen + llm 可通过全部闸门,卡在 provider 缺失
/// (provider 检查先于截屏,CI 无显示器也走得到这一步)。
#[tokio::test]
async fn click_text_dry_run_needs_only_screen_then_provider() {
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "登录", "dry_run": true }),
        click_text_dry_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("requires AI provider"), "got: {err}");
}

/// OCR 走 LLM 通路:desktop 全授但缺 llm 能力必须被拦(与 image.ocr 同语义)。
#[tokio::test]
async fn click_text_requires_llm_capability() {
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "登录", "dry_run": true }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("llm"), "got: {err}");
}

#[tokio::test]
async fn click_text_rejects_empty_text() {
    let mut caps = desktop_caps();
    caps.llm = vec!["*".into()];
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "   ", "dry_run": true }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("text must not be empty"), "got: {err}");
}

#[tokio::test]
async fn click_text_rejects_zero_size_region() {
    let mut caps = desktop_caps();
    caps.llm = vec!["*".into()];
    let err = run_with(
        "desktop.click_text",
        json!({
            "text": "登录",
            "dry_run": true,
            "region": { "x": 0, "y": 0, "width": 0, "height": 10 },
        }),
        caps,
    )
    .await
    .unwrap_err();
    assert!(err.contains("width/height must be > 0"), "got: {err}");
}

#[tokio::test]
async fn click_text_rejects_unknown_field() {
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "登录", "bogus": 1 }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}

#[tokio::test]
async fn click_text_rejects_bad_match_mode() {
    let err = run_with(
        "desktop.click_text",
        json!({ "text": "登录", "match": "fuzzy" }),
        desktop_caps(),
    )
    .await
    .unwrap_err();
    assert!(err.contains("input invalid"), "got: {err}");
}
