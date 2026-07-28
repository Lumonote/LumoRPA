//! P1-2:「动作 ID → 所需 capability 类别」的**单一声明表**。
//!
//! 运行期真源是 `lumo-core` `StepCtx` 的 `ensure_fs_read / ensure_fs_write /
//! ensure_network_url / ensure_llm / ensure_mcp_* / ensure_desktop` 门禁(由各
//! 动作实现调用)。此前静态侧有两份互相漂移的硬编码副本 —— lumo-core
//! `validate.rs` 的 match 清单与本 crate `lint.rs` 的前缀启发 —— 都只覆盖零头,
//! 导致 validate/lint 全绿的 flow 跑到一半 `CapabilityDenied`。
//!
//! 现在 validate 与 lint 都从本表派生。表放在 lumo-dsl(依赖层次最低,
//! lumo-core 与 lint 都能看到,而 lumo-dsl 不能反向依赖 lumo-actions);与实际
//! `ensure_*` 调用的同步由 lumo-actions `src/lib.rs` 尾部的
//! `capability_caps_sync` 测试钉住(注册表遍历 + 无门禁白名单,同
//! `KNOWN_RESOURCE_KINDS` 的既有模式):新增动作不归类即测试失败。
//!
//! 语义约定:
//! * `required`:动作每次执行都会过的门禁。静态检查只能判定「grant 列表为空 =
//!   必拒」;列表非空但值不匹配(路径/host 粒度)仍可能运行期拒,静态不可判。
//! * `conditional`:仅当 `with.<key>` 出现(非 null、非空数组)才会过的门禁,
//!   如 `email.send` 的 `attachments`、`human.approve` 的 `notify`、http 族的
//!   `mtls` 客户端证书。
//! * 刻意**不在**表里的:`browser.launch` / `browser.close` / 剪贴板 / 纯数据
//!   变换等 —— 它们运行期就没有任何 `ensure_*` 门禁(browser.open 才是网络
//!   闸口);`system.shell` / `system.process_kill` / `system.app_start` 走
//!   环境变量开关而非 capability,见 [`ENV_GATED_ACTIONS`]。
//!
//! 审计基线:2026-07 `grep -rn "ensure_fs_read\|ensure_fs_write\|
//! ensure_network_url\|ensure_llm\|ensure_mcp\|ensure_desktop"
//! crates/lumo-actions/src/`(外加 lumo-ai 的 `ai.chat`)逐动作核对。

use crate::ast::Capabilities;
use serde_json::Value;

/// 一类 capability 需求。`Desktop` 携带粗粒度类别(mouse/keyboard/screen/window),
/// 与运行期 `ensure_desktop(category)` 一致;静态检查粒度是「desktop 列表非空」,
/// 类别用于提示文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredCap {
    FsRead,
    FsWrite,
    Network,
    Llm,
    Mcp,
    Desktop(&'static str),
}

impl RequiredCap {
    /// `spec.capabilities` 下对应的字段名(YAML 拼写)。
    pub fn spec_field(&self) -> &'static str {
        match self {
            RequiredCap::FsRead => "fs.read",
            RequiredCap::FsWrite => "fs.write",
            RequiredCap::Network => "network",
            RequiredCap::Llm => "llm",
            RequiredCap::Mcp => "mcp",
            RequiredCap::Desktop(_) => "desktop",
        }
    }

    /// lint 稳定 code(Studio 按 code 接「+ add capability」按钮)。
    pub fn lint_code(&self) -> &'static str {
        match self {
            RequiredCap::FsRead => "capability.fs_read",
            RequiredCap::FsWrite => "capability.fs_write",
            RequiredCap::Network => "capability.network",
            RequiredCap::Llm => "capability.llm",
            RequiredCap::Mcp => "capability.mcp",
            RequiredCap::Desktop(_) => "capability.desktop",
        }
    }

    /// 静态可判的授权检查:对应 grant 列表是否非空。空列表 = 运行期必然
    /// `CapabilityDenied`;非空时值是否匹配(路径/host/模型粒度)运行期才知道。
    pub fn is_granted(&self, caps: &Capabilities) -> bool {
        let grants = match self {
            RequiredCap::FsRead => &caps.fs_read,
            RequiredCap::FsWrite => &caps.fs_write,
            RequiredCap::Network => &caps.network,
            RequiredCap::Llm => &caps.llm,
            RequiredCap::Mcp => &caps.mcp,
            RequiredCap::Desktop(_) => &caps.desktop,
        };
        !grants.is_empty()
    }
}

/// 单个动作的 capability 需求声明。
#[derive(Debug)]
pub struct ActionCaps {
    pub action: &'static str,
    /// 每次执行必过的门禁。
    pub required: &'static [RequiredCap],
    /// 仅当 `with.<key>` 出现时才过的门禁:`(with 键名, 需求)`。
    pub conditional: &'static [(&'static str, RequiredCap)],
}

const fn entry(
    action: &'static str,
    required: &'static [RequiredCap],
    conditional: &'static [(&'static str, RequiredCap)],
) -> ActionCaps {
    ActionCaps {
        action,
        required,
        conditional,
    }
}

use RequiredCap::{Desktop, FsRead, FsWrite, Llm, Mcp, Network};

/// 声明表本体。**按 action 字典序排列**(`action_caps` 走二分;顺序有测试钉住)。
pub static ACTION_CAPS: &[ActionCaps] = &[
    entry("ai.chat", &[Llm], &[]),
    entry("archive.unzip", &[FsRead, FsWrite], &[]),
    entry("archive.zip", &[FsRead, FsWrite], &[]),
    entry("browser.download_wait", &[FsWrite], &[]),
    entry("browser.open", &[Network], &[]),
    entry("browser.print_pdf", &[FsWrite], &[]),
    entry("browser.screenshot", &[FsWrite], &[]),
    entry("browser.upload", &[FsRead], &[]),
    entry("csv.read", &[FsRead], &[]),
    entry("csv.write", &[FsWrite], &[]),
    entry("db.mysql_batch", &[Network], &[]),
    entry("db.mysql_exec", &[Network], &[]),
    entry("db.mysql_query", &[Network], &[]),
    entry("db.postgres_batch", &[Network], &[]),
    entry("db.postgres_exec", &[Network], &[]),
    entry("db.postgres_query", &[Network], &[]),
    entry("db.sqlite_batch", &[FsWrite], &[]),
    entry("db.sqlite_exec", &[FsWrite], &[]),
    entry("db.sqlite_query", &[FsRead], &[]),
    entry("desktop.click", &[Desktop("mouse")], &[]),
    // click_text:截屏(screen)+ OCR(llm)必过;真点击另过 mouse(dry_run 免),
    // 静态粒度上 screen 已要求 desktop 非空,mouse 不再单列。
    entry("desktop.click_text", &[Llm, Desktop("screen")], &[]),
    entry("desktop.drag", &[Desktop("mouse")], &[]),
    entry("desktop.key", &[Desktop("keyboard")], &[]),
    entry("desktop.move", &[Desktop("mouse")], &[]),
    entry("desktop.screenshot", &[FsWrite, Desktop("screen")], &[]),
    entry("desktop.scroll", &[Desktop("mouse")], &[]),
    entry("desktop.type", &[Desktop("keyboard")], &[]),
    entry("docx.read_text", &[FsRead], &[]),
    entry("docx.replace_placeholders", &[FsRead, FsWrite], &[]),
    entry(
        "email.fetch",
        &[Network],
        &[("save_attachments_to", FsWrite)],
    ),
    entry("email.mark", &[Network], &[]),
    entry("email.move", &[Network], &[]),
    entry("email.send", &[Network], &[("attachments", FsRead)]),
    entry("excel.add_chart", &[FsRead, FsWrite], &[]),
    entry("excel.add_sheet", &[FsRead, FsWrite], &[]),
    entry("excel.autofit_columns", &[FsRead, FsWrite], &[]),
    entry("excel.delete_columns", &[FsRead, FsWrite], &[]),
    entry("excel.delete_rows", &[FsRead, FsWrite], &[]),
    entry("excel.delete_sheet", &[FsRead, FsWrite], &[]),
    entry("excel.find_replace", &[FsRead, FsWrite], &[]),
    entry("excel.freeze_panes", &[FsRead, FsWrite], &[]),
    entry("excel.insert_columns", &[FsRead, FsWrite], &[]),
    entry("excel.insert_rows", &[FsRead, FsWrite], &[]),
    entry("excel.lookup", &[FsRead], &[]),
    entry("excel.merge_cells", &[FsRead, FsWrite], &[]),
    entry("excel.read_cell", &[FsRead], &[]),
    entry("excel.read_range", &[FsRead], &[]),
    entry("excel.read_rows", &[FsRead], &[]),
    entry("excel.rename_sheet", &[FsRead, FsWrite], &[]),
    entry("excel.set_column_width", &[FsRead, FsWrite], &[]),
    entry("excel.set_comment", &[FsRead, FsWrite], &[]),
    entry("excel.set_conditional_format", &[FsRead, FsWrite], &[]),
    entry("excel.set_data_validation", &[FsRead, FsWrite], &[]),
    entry("excel.set_formula", &[FsRead, FsWrite], &[]),
    entry("excel.set_row_height", &[FsRead, FsWrite], &[]),
    entry("excel.set_style", &[FsRead, FsWrite], &[]),
    entry("excel.sheet_names", &[FsRead], &[]),
    entry("excel.write_cell", &[FsWrite], &[]),
    entry("excel.write_range", &[FsWrite], &[]),
    entry("excel.write_row", &[FsWrite], &[]),
    entry("file.append", &[FsWrite], &[]),
    entry("file.copy", &[FsRead, FsWrite], &[]),
    entry("file.delete", &[FsWrite], &[]),
    entry("file.exists", &[FsRead], &[]),
    entry("file.list", &[FsRead], &[]),
    entry("file.metadata", &[FsRead], &[]),
    entry("file.mkdir", &[FsWrite], &[]),
    // move/rename:整树 read+write 门禁(ensure_tree)+ 目标 write。
    entry("file.move", &[FsRead, FsWrite], &[]),
    entry("file.read", &[FsRead], &[]),
    entry("file.rename", &[FsRead, FsWrite], &[]),
    entry("file.wait", &[FsRead], &[]),
    entry("file.write", &[FsWrite], &[]),
    entry("ftp.download", &[FsWrite, Network], &[]),
    // 传输动词别名(与目标动作同参同门禁):ftp.get ≙ ftp.download,
    // ftp.put ≙ ftp.upload,s3.download ≙ s3.get,s3.upload ≙ s3.put。
    entry("ftp.get", &[FsWrite, Network], &[]),
    entry("ftp.put", &[FsRead, Network], &[]),
    entry("ftp.upload", &[FsRead, Network], &[]),
    entry("hash.md5", &[], &[("path", FsRead)]),
    entry("hash.sha1", &[], &[("path", FsRead)]),
    entry("hash.sha256", &[], &[("path", FsRead)]),
    entry("hash.sha512", &[], &[("path", FsRead)]),
    entry("http.download", &[FsWrite, Network], &[("mtls", FsRead)]),
    entry("http.oauth2_token", &[Network], &[("mtls", FsRead)]),
    entry("http.paginate", &[Network], &[("mtls", FsRead)]),
    entry("http.request", &[Network], &[("mtls", FsRead)]),
    // http.upload 的 mtls 也读文件,但 fs.read 已是必过项,无需再单列条件。
    entry("http.upload", &[FsRead, Network], &[]),
    // human.approve 自身不动外部世界;带 `notify` 字段时复用 notify.send 内核
    // 与网络门禁(human.input/confirm 无任何门禁,不在表内)。
    entry("human.approve", &[], &[("notify", Network)]),
    entry("image.compare", &[FsRead], &[]),
    entry("image.locate", &[FsRead], &[]),
    entry("image.ocr", &[FsRead, Llm], &[]),
    entry("mcp.call", &[Mcp], &[]),
    entry("mcp.discover", &[Mcp], &[]),
    entry("notify.dingtalk", &[Network], &[]),
    entry("notify.feishu", &[Network], &[]),
    entry("notify.send", &[Network], &[]),
    entry("notify.wecom", &[Network], &[]),
    entry("pdf.extract_text", &[FsRead], &[]),
    entry("pdf.info", &[FsRead], &[]),
    entry("pdf.write", &[FsWrite], &[]),
    entry("s3.download", &[FsWrite, Network], &[]),
    entry("s3.get", &[FsWrite, Network], &[]),
    entry("s3.put", &[FsRead, Network], &[]),
    entry("s3.upload", &[FsRead, Network], &[]),
    entry("window.activate", &[Desktop("window")], &[]),
    entry("window.bounds", &[Desktop("window")], &[]),
    entry("window.close", &[Desktop("window")], &[]),
    entry("window.list", &[Desktop("window")], &[]),
    entry("window.maximize", &[Desktop("window")], &[]),
    entry("window.minimize", &[Desktop("window")], &[]),
];

/// 查动作的 capability 声明;不在表内 = 无静态可断言的 capability 需求。
pub fn action_caps(action: &str) -> Option<&'static ActionCaps> {
    ACTION_CAPS
        .binary_search_by(|e| e.action.cmp(action))
        .ok()
        .map(|i| &ACTION_CAPS[i])
}

/// 环境变量开关型动作(独立于 capability 体系):`(动作, 运行机需置 1 的环境变量)`。
/// validate/lint 无法检查目标机环境,只能提示。
pub static ENV_GATED_ACTIONS: &[(&str, &str)] = &[
    ("system.app_start", "LUMO_ALLOW_PROCESS"),
    ("system.process_kill", "LUMO_ALLOW_PROCESS"),
    ("system.shell", "LUMO_ALLOW_SHELL"),
];

/// 动作若受环境变量开关门禁,返回该变量名。
pub fn env_gate(action: &str) -> Option<&'static str> {
    ENV_GATED_ACTIONS
        .iter()
        .find(|(a, _)| *a == action)
        .map(|(_, var)| *var)
}

/// `conditional` 键是否「生效」:出现且非 null、非空数组(空 `attachments: []`
/// 运行期不会触发门禁,静态也不误报)。
pub fn conditional_key_engaged(with: &Value, key: &str) -> bool {
    match with.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// P2-6:network grant 形状静态校验。
///
/// 运行期门禁(`StepCtx::ensure_network_url`)先用 `extract_host` 把 URL 剥成
/// **裸 host**(去 scheme/path/userinfo/端口、转小写),再与 grant 做等值/通配
/// 匹配 —— 所以写成 `https://example.com`、`example.com/api`、`host:5432` 的
/// grant **永远匹配不上**,是静默死配置。此函数对这类 grant 返回错误文案
/// (含正确写法示例);合法形状返回 `None`。
///
/// 含 `${VAR}` 的 grant 运行期才展开,形状不可静态判定,直接放行。
pub fn network_grant_shape_error(grant: &str) -> Option<String> {
    let g = grant.trim();
    if g.is_empty() {
        return Some(
            "network grant is empty — it can never match a host; \
             use a bare host like `example.com`, or `*` to allow all hosts"
                .into(),
        );
    }
    if g.contains("${") {
        return None; // 环境变量运行期展开,静态放行
    }
    let offending = if g.contains("://") {
        "a scheme"
    } else if g.contains('/') {
        "a path"
    } else if g.contains('@') {
        "userinfo"
    } else if g.contains(':') {
        "a port"
    } else {
        return None;
    };
    // 按运行期 extract_host 同样的规则剥出裸 host,作为修正建议。
    let after_scheme = g.split_once("://").map(|(_, rest)| rest).unwrap_or(g);
    let host = after_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let suggestion = if host.is_empty() {
        "example.com".to_string()
    } else {
        host
    };
    Some(format!(
        "network grant `{g}` contains {offending}, but the runtime gate matches the bare \
         host only, so this grant can NEVER match — write `{suggestion}` instead \
         (valid shapes: `example.com`, `*.example.com`, `*`)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_is_sorted_and_unique() {
        // `action_caps` 依赖二分查找;顺序/去重坏掉时这里先炸。
        for pair in ACTION_CAPS.windows(2) {
            assert!(
                pair[0].action < pair[1].action,
                "ACTION_CAPS 必须按 action 字典序且不重复:`{}` >= `{}`",
                pair[0].action,
                pair[1].action
            );
        }
    }

    #[test]
    fn lookup_hits_and_misses() {
        let first = ACTION_CAPS.first().unwrap().action;
        let last = ACTION_CAPS.last().unwrap().action;
        assert!(action_caps(first).is_some());
        assert!(action_caps(last).is_some());
        let http = action_caps("http.request").expect("http.request in table");
        assert!(http.required.contains(&RequiredCap::Network));
        assert!(action_caps("control.log").is_none());
        assert!(action_caps("no.such_action").is_none());
    }

    #[test]
    fn env_gate_matches_declared_actions() {
        assert_eq!(env_gate("system.shell"), Some("LUMO_ALLOW_SHELL"));
        assert_eq!(env_gate("system.process_kill"), Some("LUMO_ALLOW_PROCESS"));
        assert_eq!(env_gate("system.app_start"), Some("LUMO_ALLOW_PROCESS"));
        assert_eq!(env_gate("system.process_list"), None); // 只读枚举,无闸门
        assert_eq!(env_gate("control.log"), None);
    }

    #[test]
    fn conditional_key_engagement_rules() {
        let with = json!({ "attachments": ["./a.pdf"], "empty": [], "nul": null, "notify": { "url": "x" } });
        assert!(conditional_key_engaged(&with, "attachments"));
        assert!(conditional_key_engaged(&with, "notify"));
        assert!(!conditional_key_engaged(&with, "empty"));
        assert!(!conditional_key_engaged(&with, "nul"));
        assert!(!conditional_key_engaged(&with, "missing"));
        assert!(!conditional_key_engaged(&Value::Null, "any"));
    }

    #[test]
    fn network_grant_shapes() {
        // 合法:裸 host / 通配 / 全放行 / 环境变量。
        for ok in [
            "example.com",
            "*.example.com",
            "*",
            "10.0.*",
            "${DB_HOST}",
            "localhost",
        ] {
            assert!(
                network_grant_shape_error(ok).is_none(),
                "`{ok}` 应是合法 grant"
            );
        }
        // 死配置:scheme / path / port / userinfo / 空串。
        for bad in [
            "https://example.com",
            "example.com/api",
            "db.internal:5432",
            "user@example.com",
            "  ",
        ] {
            assert!(network_grant_shape_error(bad).is_some(), "`{bad}` 应被拒绝");
        }
        // 报错文案给出剥好的建议写法。
        let msg = network_grant_shape_error("https://Example.com/api").unwrap();
        assert!(
            msg.contains("`example.com`"),
            "建议写法应剥出裸 host: {msg}"
        );
        let msg = network_grant_shape_error("db.internal:5432").unwrap();
        assert!(msg.contains("`db.internal`"), "端口应剥掉: {msg}");
    }
}
