//! Postgres-backed persistence for typed long-term memory.

pub mod mapping;
pub mod repo;

use sqlx::migrate::Migrator;

static MIGRATOR: Migrator = sqlx::migrate!();

pub use repo::PgMemoryRepository;

/// Returns the embedded Postgres migrator for the memory schema.
#[must_use]
pub fn migrator() -> &'static Migrator {
    &MIGRATOR
}
