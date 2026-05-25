//! Sandbox diagnostics backend facade.

use super::broker::{
    StackLogsFetchRequest, StackLogsFetchResponse, StackLogsListSourcesRequest,
    StackLogsListSourcesResponse,
};
use super::{
    SandboxBackend, SandboxBackendId, SandboxCapability, SandboxDiagnostics, SandboxManager,
};
use anyhow::Result;
use async_trait::async_trait;

const SANDBOX_DIAGNOSTICS_BACKEND_ID: SandboxBackendId =
    SandboxBackendId::new("sandbox/diagnostics-runtime");
const SANDBOX_DIAGNOSTICS_CAPABILITIES: &[SandboxCapability] = &[SandboxCapability::Diagnostics];

/// Diagnostics facade for stack-level sandbox and compose logs.
pub struct SandboxDiagnosticsRuntime;

impl Default for SandboxDiagnosticsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxDiagnosticsRuntime {
    /// Create a diagnostics facade backed by the compiled sandbox diagnostics backend.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SandboxBackend for SandboxDiagnosticsRuntime {
    fn id(&self) -> SandboxBackendId {
        SANDBOX_DIAGNOSTICS_BACKEND_ID
    }

    fn capabilities(&self) -> &'static [SandboxCapability] {
        SANDBOX_DIAGNOSTICS_CAPABILITIES
    }
}

#[async_trait]
impl SandboxDiagnostics for SandboxDiagnosticsRuntime {
    async fn list_stack_log_sources(
        &self,
        request: StackLogsListSourcesRequest,
    ) -> Result<StackLogsListSourcesResponse> {
        SandboxManager::list_stack_log_sources(request).await
    }

    async fn fetch_stack_logs(
        &self,
        request: StackLogsFetchRequest,
    ) -> Result<StackLogsFetchResponse> {
        SandboxManager::fetch_stack_logs(request).await
    }
}
