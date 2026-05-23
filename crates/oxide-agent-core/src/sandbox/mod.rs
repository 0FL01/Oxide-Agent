//! Docker sandbox management for Agent Mode
//!
//! Provides isolated execution environments for agents using Docker containers.

/// Unix-socket sandbox broker protocol, client, and server.
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub mod broker;
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub mod manager;
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
)))]
mod manager_stub;
pub mod scope;

#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use broker::{SandboxBrokerClient, SandboxBrokerServer};
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use manager::{ExecResult, SandboxContainerRecord, SandboxManager};
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
)))]
pub use manager_stub::{ExecResult, SandboxContainerRecord, SandboxManager};
pub use scope::SandboxScope;
