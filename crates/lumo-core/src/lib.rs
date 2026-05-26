//! LumoRPA execution core.
//!
//! Provides the `Action` trait, `ActionRegistry`, `FlowVm`, `StepCtx`,
//! and durable step execution semantics inspired by Inngest/Temporal.

pub mod action;
pub mod ctx;
pub mod error;
pub mod registry;
pub mod vm;

pub use action::{Action, ActionResult};
pub use ctx::StepCtx;
pub use error::{ExecError, StepError};
pub use registry::ActionRegistry;
pub use vm::{FlowVm, RunHandle, RunOptions, RunReport};
