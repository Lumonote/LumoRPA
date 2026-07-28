//! LumoRPA built-in actions.

pub mod archive;
pub mod browser;
pub mod clipboard;
pub(crate) mod contracts;
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
///
/// P1-2:新增动作时必须同步归类它的 capability 需求 —— 有 `ensure_*` 门禁的
/// 进 lumo-dsl 的声明表 `lumo_dsl::caps::ACTION_CAPS`(validate/lint 的静态
/// 能力真源,与运行期 `StepCtx::ensure_*` 同步);无门禁的加进本文件尾部
/// `capability_caps_sync::NO_CAP_ACTIONS` 白名单。漏归类会被同名测试拦下。
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
            crate::email::IMAP_KIND,
            crate::excel::XLSX_KIND,
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

#[cfg(test)]
mod capability_caps_sync {
    //! P1-2:capability 静态声明表(`lumo_dsl::caps::ACTION_CAPS`,validate/lint
    //! 共用的单一真源)与本 crate 实际注册动作之间的防漂移测试,仿
    //! `resource_kind_sync` 的既有模式(表在 lumo-dsl 静态声明——它不能依赖
    //! lumo-actions;真值由本侧测试钉住)。
    //!
    //! 强制不变量:**每个注册进 `register_all` 的动作必须被归类**——要么在
    //! `ACTION_CAPS`(有 `ensure_*` 门禁,审计自 `grep -rn "ensure_fs_read\|
    //! ensure_fs_write\|ensure_network_url\|ensure_llm\|ensure_mcp\|
    //! ensure_desktop" crates/lumo-actions/src/`),要么在下面的
    //! `NO_CAP_ACTIONS` 白名单(实现里没有任何门禁)。新增动作不归类、或表里
    //! 残留已改名/删除的动作,这里先炸,而不是等用户的 flow validate 全绿后
    //! 运行到一半 `CapabilityDenied`。
    use std::collections::BTreeSet;

    /// 运行期不调用任何 `ensure_*` 门禁的动作(纯数据变换 / 进程内控制流 /
    /// 宿主 UI 交互 / 已建会话上的浏览器操作)。注意:
    /// * `browser.launch`/`browser.close` 等确实无门禁——网络闸口在
    ///   `browser.open`(URL 级),不在进程拉起。
    /// * `system.shell`/`system.process_kill`/`system.app_start` 走
    ///   `LUMO_ALLOW_SHELL`/`LUMO_ALLOW_PROCESS` 环境变量开关,不是 capability
    ///   体系(见 `lumo_dsl::caps::ENV_GATED_ACTIONS`,lint 有 warning 提示)。
    /// * `human.input`/`human.confirm` 只等宿主回执;`human.approve` 因可选
    ///   `notify` 字段在 ACTION_CAPS 里(条件性 network)。
    const NO_CAP_ACTIONS: &[&str] = &[
        "browser.back",
        "browser.click",
        "browser.close",
        "browser.cookies",
        "browser.dialog",
        "browser.drag_and_drop",
        "browser.eval",
        "browser.extract",
        "browser.extract_table",
        "browser.frame",
        "browser.forward",
        "browser.hover",
        "browser.info",
        "browser.launch",
        "browser.reload",
        "browser.scroll",
        "browser.select",
        "browser.set_cookie",
        "browser.tab",
        "browser.tabs",
        "browser.type",
        "browser.wait",
        "browser.wait_response",
        "clipboard.get",
        "clipboard.set",
        "control.break",
        "control.continue",
        "control.fail",
        "control.for",
        "control.for_each",
        "control.if",
        "control.log",
        "control.parallel",
        "control.set_var",
        "control.sleep",
        "control.try",
        "control.while",
        "csv.parse",
        "csv.stringify",
        "data.dedup",
        "data.filter",
        "data.group_by",
        "data.join",
        "data.json_format",
        "data.json_parse",
        "data.sort_multi",
        "date.add",
        "date.diff",
        "date.format",
        "date.now",
        "date.parse",
        "date.weekday",
        "date.workday_add",
        "human.confirm",
        "human.input",
        "json.delete",
        "json.get",
        "json.keys",
        "json.merge",
        "json.set",
        "json.values",
        "list.append",
        "list.contains",
        "list.get",
        "list.length",
        "list.pluck",
        "list.range",
        "list.reverse",
        "list.slice",
        "list.sort",
        "list.unique",
        "math.abs",
        "math.avg",
        "math.max",
        "math.min",
        "math.random",
        "math.round",
        "math.sum",
        "regex.captures",
        "regex.find_all",
        "regex.match",
        "regex.replace",
        "string.contains",
        "string.encode_convert",
        "string.ends_with",
        "string.format",
        "string.join",
        "string.length",
        "string.lower",
        "string.pad_left",
        "string.pad_right",
        "string.repeat",
        "string.replace",
        "string.split",
        "string.starts_with",
        "string.substring",
        "string.trim",
        "string.upper",
        "system.app_start",
        "system.env_get",
        "system.platform",
        "system.process_kill",
        "system.process_list",
        "system.shell",
        "system.sleep",
        "util.base64_decode",
        "util.base64_encode",
        "util.url_decode",
        "util.url_encode",
        "util.uuid",
        "xml.build",
        "xml.parse",
        "xml.xpath",
    ];

    /// 表里存在、但不由本 crate 的 `register_all` 注册的动作:`ai.chat` 在
    /// lumo-ai;desktop 特性关闭时 `desktop.*` / `window.*` 不注册。
    fn externally_registered() -> BTreeSet<&'static str> {
        // desktop 特性开启时 extend 分支被 cfg 掉,mut 只在关闭路径用到。
        #[allow(unused_mut)]
        let mut ids: BTreeSet<&'static str> = ["ai.chat"].into_iter().collect();
        #[cfg(not(feature = "desktop"))]
        ids.extend([
            "desktop.click",
            "desktop.click_text",
            "desktop.drag",
            "desktop.key",
            "desktop.move",
            "desktop.screenshot",
            "desktop.scroll",
            "desktop.type",
            "window.activate",
            "window.bounds",
            "window.close",
            "window.list",
            "window.maximize",
            "window.minimize",
        ]);
        ids
    }

    fn registered_ids() -> BTreeSet<String> {
        let mut registry = lumo_core::ActionRegistry::new();
        crate::register_all(&mut registry);
        registry.iter_ids().collect()
    }

    #[test]
    fn every_registered_action_is_classified() {
        let no_caps: BTreeSet<&str> = NO_CAP_ACTIONS.iter().copied().collect();
        let mut unclassified = Vec::new();
        let mut double_classified = Vec::new();
        for id in registered_ids() {
            let in_table = lumo_dsl::caps::action_caps(&id).is_some();
            let in_whitelist = no_caps.contains(id.as_str());
            match (in_table, in_whitelist) {
                (false, false) => unclassified.push(id),
                (true, true) => double_classified.push(id),
                _ => {}
            }
        }
        assert!(
            unclassified.is_empty(),
            "未归类动作(有 ensure_* 门禁 → 进 lumo_dsl::caps::ACTION_CAPS;\
             无门禁 → 进 NO_CAP_ACTIONS):{unclassified:?}"
        );
        assert!(
            double_classified.is_empty(),
            "动作不能同时在 ACTION_CAPS 与 NO_CAP_ACTIONS:{double_classified:?}"
        );
    }

    #[test]
    fn caps_table_and_whitelist_refer_to_real_actions() {
        let registered = registered_ids();
        let external = externally_registered();
        let mut ghosts = Vec::new();
        for entry in lumo_dsl::caps::ACTION_CAPS {
            if !registered.contains(entry.action) && !external.contains(entry.action) {
                ghosts.push(entry.action);
            }
        }
        for id in NO_CAP_ACTIONS {
            if !registered.contains(*id) {
                ghosts.push(id);
            }
        }
        assert!(
            ghosts.is_empty(),
            "声明表/白名单引用了不存在的动作(改名或删除后忘同步):{ghosts:?}"
        );
    }
}
