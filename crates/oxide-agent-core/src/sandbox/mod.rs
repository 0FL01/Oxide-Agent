//! Docker sandbox management for Agent Mode
//!
//! Provides isolated execution environments for agents using Docker containers.

pub mod admin;
/// Unix-socket sandbox broker protocol, client, and server.
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub mod broker;
#[cfg(feature = "tool-stack-logs")]
pub mod diagnostics;
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
pub mod traits;

pub use admin::SandboxAdminRuntime;
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use broker::SandboxBrokerClient;
#[cfg(feature = "sandbox-backend-docker-direct")]
pub use broker::SandboxBrokerServer;
#[cfg(feature = "tool-stack-logs")]
pub use diagnostics::SandboxDiagnosticsRuntime;
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
pub use traits::SandboxAdmin;
#[cfg(feature = "tool-stack-logs")]
pub use traits::SandboxDiagnostics;
pub use traits::{
    SandboxBackend, SandboxBackendId, SandboxCapability, SandboxExec, SandboxFileListing,
    SandboxFileOps, SandboxLifecycle,
};
