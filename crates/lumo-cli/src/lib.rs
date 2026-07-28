//! lumo-cli 的库入口。
//!
//! 二进制入口仍在 `main.rs`；这里把 `cmd` 模块暴露成 lib target，唯一目的
//! 是让集成测试（`tests/`）能触达宿主侧的 VM 组装入口（[`cmd::host_vm`]）
//! 做契约测试 —— bin-only crate 的 `tests/` 无法 `use` 自身代码。

pub mod cmd;
