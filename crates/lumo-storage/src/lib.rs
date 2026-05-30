//! LumoRPA storage layer.
//!
//! Default backend: SQLite via `rusqlite` with WAL mode.
//! A `Repo` trait abstracts the backend so libSQL can be plugged in later
//! without touching call sites.

pub mod error;
pub mod repo;
pub mod schema;
pub mod types;
pub mod vault;

pub use error::StorageError;
pub use repo::Repo;
pub use types::*;
pub use vault::VaultIdentity;
