//! Docker sandbox management for Agent Mode
//!
//! Provides isolated execution environments for agents using Docker containers.

/// Unix-socket sandbox broker protocol, client, and server.
pub mod broker;
pub mod manager;
pub mod scope;

pub use broker::{SandboxBrokerClient, SandboxBrokerServer};
pub use manager::{ExecResult, SandboxContainerRecord, SandboxManager};
pub use scope::SandboxScope;
