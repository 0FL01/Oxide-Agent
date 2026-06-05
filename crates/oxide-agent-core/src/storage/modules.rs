//! Feature-gated storage backend modules and factories.

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::AgentSettings;

#[cfg(feature = "storage-s3-r2")]
use super::R2StorageConfig;
#[cfg(feature = "storage-sqlx")]
use super::{SqlxStorage, SqlxStorageConfig, SQLX_STORAGE_MODULE_ID};
use super::{StorageError, StorageProvider};

/// Built storage services exposed by the selected storage backend module.
pub struct BuiltStorageBackend {
    /// Stable storage backend module ID.
    pub module_id: &'static str,
    /// Primary storage provider consumed by runtime and transport code.
    pub provider: Arc<dyn StorageProvider>,
    /// Optional shared Postgres handle used while the SQL backend is staged in.
    #[cfg(feature = "storage-sqlx")]
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
/// During the staged R2-to-Postgres migration, R2 remains the primary backend
/// when enabled; SQLx can be selected by disabling `storage/r2` and enabling
/// `storage/sqlx`.
#[cfg(any(feature = "storage-s3-r2", feature = "storage-sqlx"))]
pub async fn build_primary_storage(
    settings: &AgentSettings,
) -> Result<BuiltStorageBackend, StorageError> {
    #[cfg(feature = "storage-s3-r2")]
    if settings.is_module_enabled("storage/r2") {
        return R2StorageModule.build(settings).await;
    }

    #[cfg(feature = "storage-sqlx")]
    if settings.is_module_enabled(SQLX_STORAGE_MODULE_ID) {
        return SqlxStorageModule.build(settings).await;
    }

    Err(StorageError::Config(
        "no durable storage backend module is enabled".to_string(),
    ))
}

#[cfg(feature = "storage-s3-r2")]
struct R2StorageModule;

#[cfg(feature = "storage-s3-r2")]
#[async_trait]
impl StorageBackendModule for R2StorageModule {
    fn module_id(&self) -> &'static str {
        "storage/r2"
    }

    async fn build(&self, settings: &AgentSettings) -> Result<BuiltStorageBackend, StorageError> {
        if !settings.is_module_enabled(self.module_id()) {
            return Err(StorageError::Config(format!(
                "{} is disabled and no earlier storage backend selected it",
                self.module_id()
            )));
        }

        let config = R2StorageConfig::from_agent_settings(settings)?;
        let storage = Arc::new(super::R2Storage::new(&config).await?);
        let provider_storage = Arc::clone(&storage);
        let provider: Arc<dyn StorageProvider> = provider_storage;
        #[cfg(feature = "storage-sqlx")]
        let sqlx = maybe_build_sqlx_foundation(settings).await?;

        Ok(BuiltStorageBackend {
            module_id: self.module_id(),
            provider,
            #[cfg(feature = "storage-sqlx")]
            sqlx,
        })
    }
}

#[cfg(feature = "storage-sqlx")]
struct SqlxStorageModule;

#[cfg(feature = "storage-sqlx")]
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

#[cfg(all(feature = "storage-s3-r2", feature = "storage-sqlx"))]
async fn maybe_build_sqlx_foundation(
    settings: &AgentSettings,
) -> Result<Option<Arc<SqlxStorage>>, StorageError> {
    if !settings.is_module_enabled(SQLX_STORAGE_MODULE_ID)
        || !SqlxStorageConfig::is_configured(settings)
    {
        return Ok(None);
    }

    let config = SqlxStorageConfig::from_agent_settings(settings)?;
    SqlxStorage::connect(config).await.map(Arc::new).map(Some)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "storage-s3-r2")]
    #[test]
    fn r2_storage_module_uses_compiled_manifest_id() {
        use super::{R2StorageModule, StorageBackendModule};

        assert_eq!(R2StorageModule.module_id(), "storage/r2");
    }

    #[cfg(feature = "storage-s3-r2")]
    #[tokio::test]
    async fn primary_storage_fails_when_r2_module_is_disabled() {
        use crate::config::{AgentSettings, ModuleRuntimeConfig};

        let mut settings = AgentSettings::default();
        settings
            .modules
            .insert("storage/r2".to_string(), ModuleRuntimeConfig::disabled());

        let result = super::build_primary_storage(&settings).await;
        let Err(error) = result else {
            panic!("disabled primary storage module must fail before backend construction");
        };

        assert!(
            error
                .to_string()
                .contains("no durable storage backend module is enabled")
                || error
                    .to_string()
                    .contains("modules.storage/sqlx.database_url or OXIDE_DATABASE_URL is missing"),
            "unexpected storage error: {error}"
        );
    }

    #[cfg(feature = "storage-sqlx")]
    #[test]
    fn sqlx_storage_module_uses_compiled_manifest_id() {
        use super::{SqlxStorageModule, StorageBackendModule};

        assert_eq!(SqlxStorageModule.module_id(), "storage/sqlx");
    }
}
