//! Sandbox management for Agent Mode.
//!
//! Provides isolated execution environments for agents through compiled backends
//! such as Docker and sandboxd.

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
pub mod error;
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
pub use error::SandboxError;
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use manager::sandbox_backend_available;
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use manager::{ExecResult, SandboxContainerRecord, SandboxInstanceRecord, SandboxManager};
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
)))]
pub use manager_stub::sandbox_backend_available;
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
)))]
pub use manager_stub::{ExecResult, SandboxContainerRecord, SandboxInstanceRecord, SandboxManager};
pub use scope::SandboxScope;
pub use traits::SandboxAdmin;
#[cfg(feature = "tool-stack-logs")]
pub use traits::SandboxDiagnostics;
pub use traits::{
    SandboxApplyFileEditResult, SandboxBackend, SandboxBackendId, SandboxCapability,
    SandboxEditReadGuard, SandboxExec, SandboxFileEdit, SandboxFileListing, SandboxFileOps,
    SandboxLifecycle,
};

/// Run startup checks for explicitly selected sandbox backends.
///
/// This is intentionally fail-fast only for explicitly configured backends so
/// profiles without sandbox tools can still start when no sandbox backend is
/// configured.
pub async fn preflight_sandbox_backend() -> Result<(), SandboxError> {
    Ok(())
}
