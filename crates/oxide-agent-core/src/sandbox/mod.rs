//! Sandbox management for Agent Mode.
//!
//! Provides isolated execution environments for agents through compiled backends
//! such as Docker and sandboxd.

pub mod admin;
/// Unix-socket sandbox broker protocol, client, and server.
#[cfg(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
))]
pub mod broker;
#[cfg(oxide_module_tool_stack_logs)]
pub mod diagnostics;
pub mod error;
#[cfg(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
))]
pub mod manager;
#[cfg(not(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
)))]
mod manager_stub;
pub mod scope;
pub mod traits;

pub use admin::SandboxAdminRuntime;
#[cfg(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
))]
pub use broker::SandboxBrokerClient;
#[cfg(oxide_module_sandbox_backend_docker_direct)]
pub use broker::SandboxBrokerServer;
#[cfg(oxide_module_tool_stack_logs)]
pub use diagnostics::SandboxDiagnosticsRuntime;
pub use error::SandboxError;
#[cfg(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
))]
pub use manager::sandbox_backend_available;
#[cfg(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
))]
pub use manager::{ExecResult, SandboxContainerRecord, SandboxInstanceRecord, SandboxManager};
#[cfg(not(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
)))]
pub use manager_stub::sandbox_backend_available;
#[cfg(not(any(
    oxide_module_sandbox_backend_docker_direct,
    oxide_module_sandbox_backend_sandboxd_client,
    oxide_module_sandbox_daemon_sandboxd,
    oxide_module_tool_stack_logs
)))]
pub use manager_stub::{ExecResult, SandboxContainerRecord, SandboxInstanceRecord, SandboxManager};
pub use scope::SandboxScope;
pub use traits::SandboxAdmin;
#[cfg(oxide_module_tool_stack_logs)]
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
