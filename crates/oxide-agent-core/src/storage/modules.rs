//! Feature-gated storage backend modules and factories.

use std::sync::Arc;

use async_trait::async_trait;

use super::{SQLX_STORAGE_MODULE_ID, SqlxStorage, SqlxStorageConfig};
use super::{StorageError, StorageProvider};
use crate::config::AgentSettings;

/// Built storage services exposed by the selected storage backend module.
pub struct BuiltStorageBackend {
    /// Stable storage backend module ID.
    pub module_id: &'static str,
    /// Primary storage provider consumed by runtime and transport code.
    pub provider: Arc<dyn StorageProvider>,
    /// Shared Postgres storage handle used by transports that need SQL-specific stores.
    pub sqlx: Option<Arc<SqlxStorage>>,
}

/// Storage backend module descriptor and factory.
#[async_trait]
pub trait StorageBackendModule: Send + Sync {
    /// Stable storage backend module ID from the compiled capability manifest.
    fn module_id(&self) -> &'static str;

    /// Builds the storage services exposed by this backend.
    async fn build(&self, settings: &AgentSettings) -> Result<BuiltStorageBackend, StorageError>;
}

/// Builds the configured primary storage backend.
///
/// SQLx/Postgres is the only durable runtime storage backend.
pub async fn build_primary_storage(
    settings: &AgentSettings,
) -> Result<BuiltStorageBackend, StorageError> {
    if settings.is_module_enabled(SQLX_STORAGE_MODULE_ID) {
        return SqlxStorageModule.build(settings).await;
    }

    Err(StorageError::Config(
        "no durable storage backend module is enabled".to_string(),
    ))
}

struct SqlxStorageModule;

#[async_trait]
impl StorageBackendModule for SqlxStorageModule {
    fn module_id(&self) -> &'static str {
        SQLX_STORAGE_MODULE_ID
    }

    async fn build(&self, settings: &AgentSettings) -> Result<BuiltStorageBackend, StorageError> {
        if !settings.is_module_enabled(self.module_id()) {
            return Err(StorageError::Config(format!(
                "{} is disabled and cannot be selected as primary storage",
                self.module_id()
            )));
        }

        let config = SqlxStorageConfig::from_agent_settings(settings)?;
        let storage = Arc::new(SqlxStorage::connect(config).await?);
        let provider_storage = Arc::clone(&storage);
        let provider: Arc<dyn StorageProvider> = provider_storage;

        Ok(BuiltStorageBackend {
            module_id: self.module_id(),
            provider,
            sqlx: Some(storage),
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sqlx_storage_module_uses_compiled_manifest_id() {
        use super::{SqlxStorageModule, StorageBackendModule};

        assert_eq!(SqlxStorageModule.module_id(), "storage/sqlx");
    }
}
