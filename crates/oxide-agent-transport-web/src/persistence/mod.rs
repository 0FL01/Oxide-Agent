//! Durable web console persistence contracts.
//!
//! The SQLx/Postgres implementation is the production path. The in-memory store
//! exists for hermetic tests and explicit development use only.

mod in_memory;
mod models;
#[cfg(any(feature = "storage-s3-r2", test))]
mod r2;
#[cfg(feature = "storage-sqlx")]
mod sqlx;
mod store;

pub use in_memory::InMemoryWebUiStore;
pub use models::*;
#[cfg(feature = "storage-s3-r2")]
pub use r2::R2WebUiStore;
#[cfg(feature = "storage-sqlx")]
pub use sqlx::SqlxWebUiStore;
pub use store::*;
