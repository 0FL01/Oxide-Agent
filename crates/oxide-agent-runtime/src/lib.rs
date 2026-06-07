#![deny(missing_docs)]
//! Oxide Agent runtime helpers.
//!
//! Provides transport-agnostic runtime orchestration for the agent.

/// Agent runtime modules.
pub mod agent;
/// Session registry and lifecycle utilities.
pub mod session_registry;

pub use agent::runtime::{
    AgentTransport, DeliveryMode, ProgressRuntimeConfig, spawn_progress_runtime,
};
pub use session_registry::SessionRegistry;
