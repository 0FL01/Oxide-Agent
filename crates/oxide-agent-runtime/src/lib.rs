#![deny(missing_docs)]
//! Oxide Agent runtime helpers.
//!
//! Provides transport-agnostic runtime orchestration for the agent.

/// Agent runtime modules.
pub mod agent;
/// Session registry and lifecycle utilities.
pub mod session_registry;

pub use agent::runtime::{
    spawn_progress_runtime, AgentTransport, DeliveryMode, ProgressRuntimeConfig,
};
pub use session_registry::SessionRegistry;
