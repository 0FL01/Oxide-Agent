//! Durable web console persistence contracts.
//!
//! The R2/S3 implementation is the production path. The in-memory store exists
//! for hermetic tests and explicit development use only.

mod in_memory;
mod models;
#[cfg(any(feature = "storage-s3-r2", test))]
mod r2;
mod store;

pub use in_memory::InMemoryWebUiStore;
pub use models::*;
#[cfg(feature = "storage-s3-r2")]
pub use r2::R2WebUiStore;
pub use store::*;
