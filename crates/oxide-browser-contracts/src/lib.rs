//! Shared REST contract types between the native browser sidecar binary and
//! the `oxide-agent-core` browser-live client.
//!
//! This crate is intentionally independent from core/runtime internals so the
//! sidecar binary and the Oxide client share one stable JSON contract —
//! eliminating the class of contract-drift bugs where the sidecar's Python
//! types diverged from the Rust client's types while all tests stayed green.

#![allow(missing_docs)]

pub mod types;

pub use types::*;
