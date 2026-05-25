//! Bubblewrap sandbox backend.

use anyhow::{anyhow, Result};

use super::{ExecResult, SandboxContainerRecord, SandboxScope};

/// Bubblewrap sandbox manager.
#[derive(Clone)]
pub(crate) struct BwrapSandboxManager {
    scope: SandboxScope,
    instance_id: String,
}

impl BwrapSandboxManager {
    /// Create a bwrap manager for the provided scope.
    pub(crate) async fn new(scope: SandboxScope) -> Result<Self> {
        let instance_id = format!("bwrap:{}", scope.stable_name());
        Ok(Self { scope, instance_id })
    }

    pub(crate) const fn is_running(&self) -> bool {
        false
    }

    pub(crate) fn container_id(&self) -> Option<&str> {
        Some(&self.instance_id)
    }

    pub(crate) const fn scope(&self) -> &SandboxScope {
        &self.scope
    }

    pub(crate) async fn create_sandbox(&mut self) -> Result<()> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn exec_command(
        &mut self,
        _cmd: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn write_file(&mut self, _path: &str, _content: &[u8]) -> Result<()> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn read_file(&mut self, _path: &str) -> Result<Vec<u8>> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn upload_file(&mut self, path: &str, content: &[u8]) -> Result<()> {
        self.write_file(path, content).await
    }

    pub(crate) async fn download_file(&mut self, path: &str) -> Result<Vec<u8>> {
        self.read_file(path).await
    }

    pub(crate) async fn get_uploads_size(&mut self) -> Result<u64> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn cleanup_old_downloads(&mut self) -> Result<u64> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn destroy(&mut self) -> Result<()> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn recreate(&mut self) -> Result<()> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn file_size_bytes(
        &mut self,
        _path: &str,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<u64> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn list_user_sandboxes(_user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn inspect_sandbox_by_name(
        _user_id: i64,
        _container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn ensure_scope_sandbox(
        _scope: SandboxScope,
    ) -> Result<SandboxContainerRecord> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn recreate_scope_sandbox(
        _scope: SandboxScope,
    ) -> Result<SandboxContainerRecord> {
        Err(bwrap_backend_pending())
    }

    pub(crate) async fn delete_sandbox_by_name(
        _user_id: i64,
        _container_name: &str,
    ) -> Result<bool> {
        Err(bwrap_backend_pending())
    }
}

fn bwrap_backend_pending() -> anyhow::Error {
    anyhow!(
        "SANDBOX_BACKEND=bwrap is compiled, but the bwrap backend implementation is not wired yet"
    )
}
