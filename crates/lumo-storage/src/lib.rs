//! LumoRPA storage layer.
//!
//! Default backend: SQLite via `rusqlite` with WAL mode.
//! A `Repo` trait abstracts the backend so libSQL can be plugged in later
//! without touching call sites.

pub mod error;
pub mod schema;
pub mod repo;
pub mod types;

pub use error::StorageError;
pub use repo::Repo;
pub use types::*;
