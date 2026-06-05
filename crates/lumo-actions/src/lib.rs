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
pub mod docx;
pub mod email;
pub mod excel;
pub mod file;
pub mod hash_ops;
pub mod http;
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
    email::register(registry);
    clipboard::register(registry);
    table_ops::register(registry);
    transfer::register(registry);
    docx::register(registry);
}
