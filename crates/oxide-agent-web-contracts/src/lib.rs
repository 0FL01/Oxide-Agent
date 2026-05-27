//! Browser-facing API contracts for the Oxide Agent web console.
//!
//! This crate is intentionally independent from core/runtime internals so the
//! backend and Rust/WASM frontend can share one stable JSON contract.

pub mod auth;
pub mod config;
pub mod error;
pub mod events;
pub mod sessions;
pub mod tasks;

pub use auth::*;
pub use config::*;
pub use error::*;
pub use events::*;
pub use sessions::*;
pub use tasks::*;
