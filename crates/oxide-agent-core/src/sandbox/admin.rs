//! Sandbox administration backend facade.

use super::{
    SandboxAdmin, SandboxBackend, SandboxBackendId, SandboxCapability, SandboxContainerRecord,
    SandboxManager, SandboxScope,
};
use anyhow::Result;
use async_trait::async_trait;

const SANDBOX_ADMIN_BACKEND_ID: SandboxBackendId = SandboxBackendId::new("sandbox/admin-runtime");
const SANDBOX_ADMIN_CAPABILITIES: &[SandboxCapability] = &[SandboxCapability::Admin];

/// Administrative facade for user/topic sandbox inventory and lifecycle operations.
pub struct SandboxAdminRuntime;

impl Default for SandboxAdminRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SandboxAdminRuntime {
    /// Create an admin facade backed by the compiled sandbox admin backend.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl SandboxBackend for SandboxAdminRuntime {
    fn id(&self) -> SandboxBackendId {
        SANDBOX_ADMIN_BACKEND_ID
    }

    fn capabilities(&self) -> &'static [SandboxCapability] {
        SANDBOX_ADMIN_CAPABILITIES
    }
}

#[async_trait]
impl SandboxAdmin for SandboxAdminRuntime {
    async fn destroy_scope(&self, scope: SandboxScope) -> Result<()> {
        let mut sandbox = SandboxManager::new(scope).await?;
        sandbox.destroy().await
    }

    async fn list_user_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        SandboxManager::list_user_sandboxes(user_id).await
    }

    async fn inspect_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        SandboxManager::inspect_sandbox_by_name(user_id, container_name).await
    }

    async fn ensure_scope_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        SandboxManager::ensure_scope_sandbox(scope).await
    }

    async fn recreate_scope_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        SandboxManager::recreate_scope_sandbox(scope).await
    }

    async fn delete_sandbox_by_name(&self, user_id: i64, container_name: &str) -> Result<bool> {
        SandboxManager::delete_sandbox_by_name(user_id, container_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_runtime_exposes_admin_capability() {
        let runtime = SandboxAdminRuntime::new();

        assert_eq!(runtime.id().as_str(), "sandbox/admin-runtime");
        assert!(runtime.capabilities().contains(&SandboxCapability::Admin));
    }
}
