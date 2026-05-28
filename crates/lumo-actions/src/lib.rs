//! LumoRPA built-in actions.

pub mod browser;
pub mod control;
pub mod data;
pub mod excel;
pub mod file;
pub mod http;
pub mod mcp;
pub mod selector_stats;
pub mod selectors;
pub mod vision;

use lumo_core::ActionRegistry;

/// Register all built-in actions into a registry.
pub fn register_all(registry: &mut ActionRegistry) {
    control::register(registry);
    data::register(registry);
    file::register(registry);
    http::register(registry);
    excel::register(registry);
    browser::register(registry);
    mcp::register(registry);
}
