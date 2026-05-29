//! LumoRPA Flow DSL.
//!
//! See docs/02-Architecture-Design.md §4 for the formal spec.

pub mod ast;
pub mod lint;
pub mod parse;
pub mod template;
pub mod validate;

pub use ast::*;
pub use lint::{lint_flow, LintIssue, LintSeverity};
pub use parse::{parse_file, parse_str, ParseError};
pub use template::{render, TemplateCtx, TemplateError};
pub use validate::{validate, ValidationError};
