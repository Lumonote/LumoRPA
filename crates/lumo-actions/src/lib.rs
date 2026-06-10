//! LumoRPA built-in actions.

pub mod archive;
pub mod browser;
pub mod clipboard;
pub mod control;
pub mod csv_ops;
pub mod data;
pub mod date_ops;
pub mod db_ops;
#[cfg(feature = "desktop")]
pub mod desktop;
#[cfg(feature = "desktop")]
pub mod desktop_screen;
#[cfg(feature = "desktop")]
pub mod desktop_text;
pub mod docx;
pub mod email;
pub mod excel;
pub mod file;
pub mod hash_ops;
pub mod http;
pub mod human;
pub mod image_match;
pub mod json_ops;
pub mod list_ops;
pub mod math_ops;
pub mod mcp;
pub mod notify;
pub mod pdf;
pub mod regex_ops;
pub mod resource_store;
pub mod schema;
pub mod selector_stats;
pub mod selectors;
pub mod string_ops;
pub mod system_ops;
pub mod table_ops;
pub mod transfer;
pub mod vision;
pub mod xml_ops;

use lumo_core::ActionRegistry;

/// Register all built-in actions into a registry.
pub fn register_all(registry: &mut ActionRegistry) {
    control::register(registry);
    data::register(registry);
    file::register(registry);
    archive::register(registry);
    http::register(registry);
    excel::register(registry);
    browser::register(registry);
    mcp::register(registry);
    pdf::register(registry);
    image_match::register(registry);
    // F-1 桌面输入(desktop.move/click/scroll/key/type)—— 默认关闭,信创核心不含;
    // 桌面端 `--features desktop` 开启(rdev,需 OS 级输入 C API)。
    #[cfg(feature = "desktop")]
    desktop::register(registry);
    // 指令集缺口 P0:原生截屏 + 窗口管理(desktop.screenshot / window.*,xcap)。
    // 与输入模拟同走 desktop feature,补齐纯桌面的图像驱动闭环与窗口前置。
    #[cfg(feature = "desktop")]
    desktop_screen::register(registry);
    // 指令集缺口 P1:OCR 文本定位点击(desktop.click_text)。截屏 + image.ocr 通路
    // + 坐标点击三者拼合,文本驱动补齐 F-1「坐标 + 图像」路线。
    #[cfg(feature = "desktop")]
    desktop_text::register(registry);

    // 第二批：通用数据/系统/AI 周边指令
    string_ops::register(registry);
    regex_ops::register(registry);
    date_ops::register(registry);
    math_ops::register(registry);
    list_ops::register(registry);
    hash_ops::register(registry);
    json_ops::register(registry);
    csv_ops::register(registry);
    system_ops::register(registry);
    db_ops::register(registry);
    notify::register(registry);
    // P1（人机交互）：human.input / confirm / approve。无 capability 门禁
    // （不动宿主 UI 之外的世界）；approve 的 notify 部分沿用 notify 自身门禁。
    human::register(registry);
    email::register(registry);
    clipboard::register(registry);
    table_ops::register(registry);
    transfer::register(registry);
    docx::register(registry);
    // 指令集缺口 P0:XML 族(SOAP/政企对接/电子发票)。纯数据变换,不进 capability gate。
    xml_ops::register(registry);
}

/// X-07(时光回溯):以 best-effort 方式把 blob 归档为当前步骤的 artifact。
///
/// `StepCtx::attach_artifact` 的语义是「blob 落 `{artifacts_dir}/{run_id}/` +
/// `artifacts` 表插行」;宿主未接 artifacts_dir(如 `--no-store` 的 CLI 路径)时
/// 它本身就是无害 no-op(返回空 id)。本封装再把归档 *失败* 也降级为告警 ——
/// artifact 只是回放侧的可观测性副本,绝不连坐动作本身的成败。
///
/// 返回值可直接嵌进动作输出:成功 → artifact ULID 字符串;no-op / 失败 → `null`。
pub(crate) fn attach_artifact_lenient(
    ctx: &lumo_core::StepCtx,
    action: &str,
    kind: &str,
    mime: &str,
    data: &[u8],
) -> serde_json::Value {
    match ctx.attach_artifact(kind, mime, data) {
        Ok(id) if !id.is_empty() => serde_json::Value::String(id),
        Ok(_) => serde_json::Value::Null,
        Err(e) => {
            tracing::warn!(target: "lumo.flow", "{action}: attach {kind} artifact failed: {e}");
            serde_json::Value::Null
        }
    }
}

#[cfg(test)]
mod resource_kind_sync {
    //! P1-2:lumo-dsl 的 `KNOWN_RESOURCE_KINDS` 白名单是各 action crate KIND
    //! 常量的静态副本(lumo-dsl 不能依赖 lumo-actions,层次反转)。本测试断言
    //! 两边一致——新增/改名资源 kind 而忘了同步白名单时在这里炸,而不是让
    //! 合法 flow 过不了 validate(或拼错 kind 静默漏网)。
    use std::collections::BTreeSet;

    #[test]
    fn dsl_whitelist_matches_action_kind_constants() {
        let actual: BTreeSet<&str> = [
            crate::browser::BROWSER_KIND,
            crate::db_ops::SQLITE_KIND,
            crate::db_ops::PG_KIND,
            crate::db_ops::MYSQL_KIND,
            crate::http::HTTP_KIND,
            crate::email::SMTP_KIND,
            crate::transfer::FTP_KIND,
        ]
        .into_iter()
        .collect();
        let whitelist: BTreeSet<&str> = lumo_dsl::KNOWN_RESOURCE_KINDS.iter().copied().collect();
        assert_eq!(
            whitelist, actual,
            "lumo-dsl KNOWN_RESOURCE_KINDS 与 action crate 的 KIND 常量漂移了——两边必须同步更新"
        );
    }
}
