//! LumoRPA execution core.
//!
//! Provides the `Action` trait, `ActionRegistry`, `FlowVm`, `StepCtx`,
//! and durable step execution semantics inspired by Inngest/Temporal.

pub mod action;
pub mod ai_hook;
pub mod ctx;
pub mod error;
pub mod registry;
pub mod vm;

pub use action::{Action, ActionResult};
pub use ai_hook::{AiCallUsage, AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark};
pub use ctx::{clamp_capabilities, CancelToken, RunStats, StepCtx};
pub use error::{CapKind, ErrorKind, ExecError, StepError};
pub use registry::{ActionRegistry, RunTeardown};
pub use vm::{FlowVm, RunHandle, RunOptions, RunReport};
