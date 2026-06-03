//! Sandbox management for Agent Mode.
//!
//! Provides isolated execution environments for agents through compiled backends
//! such as Docker, sandboxd, and Bubblewrap.

pub mod admin;
/// Unix-socket sandbox broker protocol, client, and server.
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub mod broker;
#[cfg(feature = "sandbox-backend-bwrap")]
pub(crate) mod bwrap;
#[cfg(feature = "tool-stack-logs")]
pub mod diagnostics;
#[cfg(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-backend-bwrap",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub mod manager;
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-backend-bwrap",
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
    feature = "sandbox-backend-bwrap",
    feature = "sandbox-daemon",
    feature = "tool-stack-logs"
))]
pub use manager::{ExecResult, SandboxContainerRecord, SandboxInstanceRecord, SandboxManager};
#[cfg(not(any(
    feature = "sandbox-backend-docker-direct",
    feature = "sandbox-backend-sandboxd-client",
    feature = "sandbox-backend-bwrap",
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
/// This is intentionally fail-fast only for explicit `SANDBOX_BACKEND=bwrap` so
/// profiles without sandbox tools can still start when no sandbox backend is
/// configured.
pub async fn preflight_sandbox_backend() -> anyhow::Result<()> {
    let Some(backend) = std::env::var_os("SANDBOX_BACKEND") else {
        return Ok(());
    };
    if !backend
        .to_string_lossy()
        .trim()
        .eq_ignore_ascii_case("bwrap")
    {
        return Ok(());
    }
    preflight_bwrap_backend().await
}

#[cfg(feature = "sandbox-backend-bwrap")]
async fn preflight_bwrap_backend() -> anyhow::Result<()> {
    bwrap::preflight_from_env()?;
    bwrap::bootstrap_image_from_env().await
}

#[cfg(not(feature = "sandbox-backend-bwrap"))]
async fn preflight_bwrap_backend() -> anyhow::Result<()> {
    anyhow::bail!(
        "SANDBOX_BACKEND=bwrap was selected, but this binary was not compiled with sandbox-backend-bwrap. Build with --features sandbox-backend-bwrap or choose another sandbox backend with SANDBOX_BACKEND=docker|broker."
    )
}
