//! Postgres source-of-truth boundary for life-mode storage.

/// SQLx row models will live here with the migration-backed implementation.
pub mod models;
/// Repository traits will live here once the SQL schema is introduced.
pub mod repository;
/// SQLx/Postgres repository implementation.
pub mod sqlx;

pub use repository::*;
pub use sqlx::*;
