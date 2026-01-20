//! Transport-agnostic runtime helpers.
//!
//! This module hosts orchestration logic that should not depend on any specific chat platform.

/// Progress runtime loop and transport abstractions.
pub mod progress;

pub use progress::{
    AgentTransport, DeliveryMode, ProgressRuntimeConfig, spawn_progress_runtime,
};
