//! LumoRPA execution core.
//!
//! Provides the `Action` trait, `ActionRegistry`, `FlowVm`, `StepCtx`,
//! and durable step execution semantics inspired by Inngest/Temporal.

pub mod action;
pub mod ai_hook;
pub mod ctx;
pub mod error;
pub mod registry;
pub mod resource;
pub mod schema;
pub mod validate;
pub mod vm;

pub use action::{Action, ActionResult};
pub use ai_hook::{AiCallUsage, AiHookProvider, Decision, HealedSelector, LocatedTarget, SoMMark};
pub use ctx::{
    clamp_capabilities, host_matches_grants, CancelToken, ResumeMemo, RunStats, StepCtx,
};
pub use error::{CapKind, ErrorKind, ExecError, StepError};
pub use registry::{ActionRegistry, RunTeardown};
pub use resource::ResourceFactory;
pub use validate::validate_steps;
pub use vm::{FlowVm, RunHandle, RunOptions, RunReport};
