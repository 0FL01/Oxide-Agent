//! Feature-gated storage backend modules and factories.

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::AgentSettings;

use super::{PersistedAgentMemoryStore, R2StorageConfig, StorageError, StorageProvider};

/// Built storage services exposed by the selected storage backend module.
pub struct BuiltStorageBackend {
    /// Stable storage backend module ID.
    pub module_id: &'static str,
    /// Primary storage provider consumed by runtime and transport code.
    pub provider: Arc<dyn StorageProvider>,
    /// Optional maintenance interface for backends that can enumerate agent memories.
    pub persisted_agent_memory: Option<Arc<dyn PersistedAgentMemoryStore>>,
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
/// S3/R2 is currently the only durable storage backend accepted by the target
/// architecture, so this factory fails if `storage/r2` is disabled at runtime.
#[cfg(feature = "storage-s3-r2")]
pub async fn build_primary_storage(
    settings: &AgentSettings,
) -> Result<BuiltStorageBackend, StorageError> {
    R2StorageModule.build(settings).await
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
                "{} is required because S3/R2 is the only durable storage backend",
                self.module_id()
            )));
        }

        let config = R2StorageConfig::from_agent_settings(settings)?;
        let storage = Arc::new(super::R2Storage::new(&config).await?);
        let provider_storage = Arc::clone(&storage);
        let provider: Arc<dyn StorageProvider> = provider_storage;
        let persisted_agent_memory: Arc<dyn PersistedAgentMemoryStore> = storage;

        Ok(BuiltStorageBackend {
            module_id: self.module_id(),
            provider,
            persisted_agent_memory: Some(persisted_agent_memory),
        })
    }
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
            error.to_string().contains(
                "storage/r2 is required because S3/R2 is the only durable storage backend"
            ),
            "unexpected storage error: {error}"
        );
    }
}
