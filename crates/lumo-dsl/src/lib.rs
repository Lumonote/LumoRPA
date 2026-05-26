//! LumoRPA Flow DSL.
//!
//! See docs/02-Architecture-Design.md §4 for the formal spec.

pub mod ast;
pub mod parse;
pub mod template;
pub mod validate;

pub use ast::*;
pub use parse::{parse_str, parse_file, ParseError};
pub use template::{render, TemplateError, TemplateCtx};
pub use validate::{validate, ValidationError};
