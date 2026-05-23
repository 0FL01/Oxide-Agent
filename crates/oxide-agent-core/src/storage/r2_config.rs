//! Module-owned configuration for the S3/R2 storage backend.

use crate::config::{AgentSettings, ModuleRuntimeConfig};

use super::StorageError;

const R2_MODULE_ID: &str = "storage/r2";

/// Resolved configuration for the S3/R2 storage backend.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct R2StorageConfig {
    /// S3-compatible endpoint URL.
    pub endpoint_url: String,
    /// Object storage bucket name.
    pub bucket_name: String,
    /// Access key ID used to authenticate with the storage backend.
    pub access_key_id: String,
    /// Secret access key used to authenticate with the storage backend.
    pub secret_access_key: String,
    /// S3-compatible region.
    pub region: String,
}

impl R2StorageConfig {
    /// Resolves R2 config from the `storage/r2` module config and module-owned
    /// direct environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Config`] when required endpoint, bucket, or
    /// credential values are missing.
    pub fn from_agent_settings(settings: &AgentSettings) -> Result<Self, StorageError> {
        let module_config = settings.modules.get(R2_MODULE_ID);

        Ok(Self {
            endpoint_url: required_value(
                module_config,
                &[ValueSource::module("endpoint"), ValueSource::module("endpoint_url")],
                &["OXIDE_R2_ENDPOINT_URL", "OXIDE_R2_ENDPOINT"],
                "modules.storage/r2.endpoint or OXIDE_R2_ENDPOINT_URL is missing",
            )?,
            bucket_name: required_value(
                module_config,
                &[ValueSource::module("bucket"), ValueSource::module("bucket_name")],
                &["OXIDE_R2_BUCKET_NAME", "OXIDE_R2_BUCKET"],
                "modules.storage/r2.bucket or OXIDE_R2_BUCKET_NAME is missing",
            )?,
            access_key_id: required_value(
                module_config,
                &[
                    ValueSource::nested("credentials", "access_key_id"),
                    ValueSource::module("access_key_id"),
                ],
                &["OXIDE_R2_ACCESS_KEY_ID"],
                "modules.storage/r2.credentials.access_key_id or OXIDE_R2_ACCESS_KEY_ID is missing",
            )?,
            secret_access_key: required_value(
                module_config,
                &[
                    ValueSource::nested("credentials", "secret_access_key"),
                    ValueSource::module("secret_access_key"),
                ],
                &["OXIDE_R2_SECRET_ACCESS_KEY"],
                "modules.storage/r2.credentials.secret_access_key or OXIDE_R2_SECRET_ACCESS_KEY is missing",
            )?,
            region: optional_value(
                module_config,
                &[ValueSource::module("region")],
                &["OXIDE_R2_REGION"],
            )
            .unwrap_or_else(|| "auto".to_string()),
        })
    }
}

#[derive(Clone, Copy)]
enum ValueSource<'a> {
    Module(&'a str),
    Nested(&'a str, &'a str),
}

impl<'a> ValueSource<'a> {
    const fn module(key: &'a str) -> Self {
        Self::Module(key)
    }

    const fn nested(object_key: &'a str, key: &'a str) -> Self {
        Self::Nested(object_key, key)
    }
}

fn required_value(
    module_config: Option<&ModuleRuntimeConfig>,
    module_sources: &[ValueSource<'_>],
    env_vars: &[&str],
    missing_message: &str,
) -> Result<String, StorageError> {
    optional_value(module_config, module_sources, env_vars)
        .ok_or_else(|| StorageError::Config(missing_message.to_string()))
}

fn optional_value(
    module_config: Option<&ModuleRuntimeConfig>,
    module_sources: &[ValueSource<'_>],
    env_vars: &[&str],
) -> Option<String> {
    module_sources
        .iter()
        .filter_map(|source| module_config.and_then(|config| source.resolve(config)))
        .find(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            env_vars
                .iter()
                .filter_map(|env_var| std::env::var(env_var).ok())
                .find(|value| !value.trim().is_empty())
        })
}

impl ValueSource<'_> {
    fn resolve(self, config: &ModuleRuntimeConfig) -> Option<&str> {
        match self {
            Self::Module(key) => config.string_value(key),
            Self::Nested(object_key, key) => config.nested_string_value(object_key, key),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    #[test]
    fn resolves_from_storage_r2_module_config() {
        let settings = AgentSettings {
            modules: BTreeMap::from([(
                R2_MODULE_ID.to_string(),
                serde_json::from_value(json!({
                    "enabled": true,
                    "endpoint": "https://r2.example.test",
                    "bucket": "oxide-agent",
                    "region": "auto",
                    "credentials": {
                        "access_key_id": "access",
                        "secret_access_key": "secret"
                    }
                }))
                .expect("module config should deserialize"),
            )]),
            ..AgentSettings::default()
        };

        let config = R2StorageConfig::from_agent_settings(&settings)
            .expect("storage/r2 module config should resolve");

        assert_eq!(config.endpoint_url, "https://r2.example.test");
        assert_eq!(config.bucket_name, "oxide-agent");
        assert_eq!(config.region, "auto");
        assert_eq!(config.access_key_id, "access");
        assert_eq!(config.secret_access_key, "secret");
    }

    #[test]
    fn module_config_takes_precedence_over_env() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var("OXIDE_R2_ENDPOINT_URL", "https://env.example.test");
        std::env::set_var("OXIDE_R2_BUCKET_NAME", "env-bucket");
        std::env::set_var("OXIDE_R2_ACCESS_KEY_ID", "env-access");
        std::env::set_var("OXIDE_R2_SECRET_ACCESS_KEY", "env-secret");
        std::env::set_var("OXIDE_R2_REGION", "env-region");

        let settings = AgentSettings {
            modules: BTreeMap::from([(
                R2_MODULE_ID.to_string(),
                serde_json::from_value(json!({
                    "endpoint_url": "https://module.example.test",
                    "bucket_name": "module-bucket",
                    "access_key_id": "module-access",
                    "secret_access_key": "module-secret",
                    "region": "module-region"
                }))
                .expect("module config should deserialize"),
            )]),
            ..AgentSettings::default()
        };

        let config = R2StorageConfig::from_agent_settings(&settings)
            .expect("storage/r2 module config should resolve");

        assert_eq!(config.endpoint_url, "https://module.example.test");
        assert_eq!(config.bucket_name, "module-bucket");
        assert_eq!(config.access_key_id, "module-access");
        assert_eq!(config.secret_access_key, "module-secret");
        assert_eq!(config.region, "module-region");

        std::env::remove_var("OXIDE_R2_ENDPOINT_URL");
        std::env::remove_var("OXIDE_R2_BUCKET_NAME");
        std::env::remove_var("OXIDE_R2_ACCESS_KEY_ID");
        std::env::remove_var("OXIDE_R2_SECRET_ACCESS_KEY");
        std::env::remove_var("OXIDE_R2_REGION");
    }

    #[test]
    fn resolves_from_module_owned_env() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var("OXIDE_R2_ENDPOINT_URL", "https://env.example.test");
        std::env::set_var("OXIDE_R2_BUCKET_NAME", "env-bucket");
        std::env::set_var("OXIDE_R2_ACCESS_KEY_ID", "env-access");
        std::env::set_var("OXIDE_R2_SECRET_ACCESS_KEY", "env-secret");
        std::env::remove_var("OXIDE_R2_REGION");

        let config = R2StorageConfig::from_agent_settings(&AgentSettings::default())
            .expect("module-owned env config should resolve");

        assert_eq!(config.endpoint_url, "https://env.example.test");
        assert_eq!(config.bucket_name, "env-bucket");
        assert_eq!(config.access_key_id, "env-access");
        assert_eq!(config.secret_access_key, "env-secret");
        assert_eq!(config.region, "auto");

        std::env::remove_var("OXIDE_R2_ENDPOINT_URL");
        std::env::remove_var("OXIDE_R2_BUCKET_NAME");
        std::env::remove_var("OXIDE_R2_ACCESS_KEY_ID");
        std::env::remove_var("OXIDE_R2_SECRET_ACCESS_KEY");
    }
}
