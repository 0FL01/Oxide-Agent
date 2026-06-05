//! Module-owned configuration for the SQLx/Postgres storage backend.

use std::path::PathBuf;
use std::time::Duration;

use crate::config::AgentSettings;

use super::StorageError;

/// Stable module id for the SQLx/Postgres durable storage backend.
pub const SQLX_STORAGE_MODULE_ID: &str = "storage/sqlx";

const DEFAULT_MAX_CONNECTIONS: u32 = 5;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MIGRATE_ON_STARTUP: bool = false;
const DEFAULT_MIGRATIONS_DIR: &str = "migrations";

/// Resolved configuration for the SQLx/Postgres storage backend.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SqlxStorageConfig {
    /// Postgres connection URL.
    pub database_url: String,
    /// Maximum open connections in the shared pool.
    pub max_connections: u32,
    /// Timeout used when acquiring or opening a connection.
    pub connect_timeout: Duration,
    /// Whether startup should run SQLx migrations before serving traffic.
    pub migrate_on_startup: bool,
    /// Directory containing SQLx migration files.
    pub migrations_dir: PathBuf,
}

impl SqlxStorageConfig {
    /// Returns whether the SQLx backend has enough config to attempt a pool.
    #[must_use]
    pub fn is_configured(settings: &AgentSettings) -> bool {
        database_url(settings).is_some()
    }

    /// Resolves SQLx/Postgres config from `storage/sqlx` module config and env.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Config`] when the database URL is missing or a
    /// numeric/bool setting cannot be parsed.
    pub fn from_agent_settings(settings: &AgentSettings) -> Result<Self, StorageError> {
        let database_url = database_url(settings).ok_or_else(|| {
            StorageError::Config(
                "modules.storage/sqlx.database_url or OXIDE_DATABASE_URL is missing".to_string(),
            )
        })?;

        let max_connections = parse_u32(
            settings.module_string_value_or_env_or_default(
                SQLX_STORAGE_MODULE_ID,
                "max_connections",
                "OXIDE_DATABASE_MAX_CONNECTIONS",
                &DEFAULT_MAX_CONNECTIONS.to_string(),
            ),
            "max_connections",
        )?;
        if max_connections == 0 {
            return Err(StorageError::Config(
                "modules.storage/sqlx.max_connections must be greater than zero".to_string(),
            ));
        }

        let connect_timeout_secs = parse_u64(
            settings.module_string_value_or_env_or_default(
                SQLX_STORAGE_MODULE_ID,
                "connect_timeout_secs",
                "OXIDE_DATABASE_CONNECT_TIMEOUT_SECS",
                &DEFAULT_CONNECT_TIMEOUT_SECS.to_string(),
            ),
            "connect_timeout_secs",
        )?;

        let migrate_on_startup = parse_bool(
            settings.module_string_value_or_env_or_default(
                SQLX_STORAGE_MODULE_ID,
                "migrate_on_startup",
                "OXIDE_DATABASE_MIGRATE_ON_STARTUP",
                if DEFAULT_MIGRATE_ON_STARTUP {
                    "true"
                } else {
                    "false"
                },
            ),
            "migrate_on_startup",
        )?;

        let migrations_dir = settings.module_string_value_or_env_or_default(
            SQLX_STORAGE_MODULE_ID,
            "migrations_dir",
            "OXIDE_DATABASE_MIGRATIONS_DIR",
            DEFAULT_MIGRATIONS_DIR,
        );

        Ok(Self {
            database_url,
            max_connections,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            migrate_on_startup,
            migrations_dir: PathBuf::from(migrations_dir),
        })
    }
}

fn database_url(settings: &AgentSettings) -> Option<String> {
    settings
        .module_string_value_or_envs(
            SQLX_STORAGE_MODULE_ID,
            "database_url",
            &["OXIDE_DATABASE_URL", "DATABASE_URL"],
        )
        .or_else(|| {
            settings.module_string_value_or_envs(
                SQLX_STORAGE_MODULE_ID,
                "url",
                &["OXIDE_DATABASE_URL", "DATABASE_URL"],
            )
        })
}

fn parse_u32(value: String, name: &str) -> Result<u32, StorageError> {
    value.trim().parse::<u32>().map_err(|error| {
        StorageError::Config(format!(
            "modules.storage/sqlx.{name} must be an unsigned integer: {error}"
        ))
    })
}

fn parse_u64(value: String, name: &str) -> Result<u64, StorageError> {
    value.trim().parse::<u64>().map_err(|error| {
        StorageError::Config(format!(
            "modules.storage/sqlx.{name} must be an unsigned integer: {error}"
        ))
    })
}

fn parse_bool(value: String, name: &str) -> Result<bool, StorageError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(StorageError::Config(format!(
            "modules.storage/sqlx.{name} must be a boolean string"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{SqlxStorageConfig, SQLX_STORAGE_MODULE_ID};
    use crate::config::{AgentSettings, ModuleRuntimeConfig};

    #[test]
    fn sqlx_config_reads_module_values() {
        let mut settings = AgentSettings::default();
        settings.modules.insert(
            SQLX_STORAGE_MODULE_ID.to_string(),
            ModuleRuntimeConfig::default()
                .with_string_value("database_url", "postgres://postgres:postgres@localhost/db")
                .with_string_value("max_connections", "3")
                .with_string_value("connect_timeout_secs", "7")
                .with_string_value("migrate_on_startup", "true")
                .with_string_value("migrations_dir", "custom-migrations"),
        );

        let config = SqlxStorageConfig::from_agent_settings(&settings)
            .expect("module SQLx config should parse");

        assert_eq!(config.max_connections, 3);
        assert_eq!(config.connect_timeout.as_secs(), 7);
        assert!(config.migrate_on_startup);
        assert_eq!(
            config.migrations_dir,
            std::path::PathBuf::from("custom-migrations")
        );
    }

    #[test]
    fn sqlx_config_requires_database_url() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .expect("test env mutex should lock");
        std::env::remove_var("OXIDE_DATABASE_URL");
        std::env::remove_var("DATABASE_URL");
        let settings = AgentSettings::default();

        let error = SqlxStorageConfig::from_agent_settings(&settings)
            .expect_err("database URL is required before connecting");

        assert!(
            error
                .to_string()
                .contains("modules.storage/sqlx.database_url or OXIDE_DATABASE_URL is missing"),
            "unexpected config error: {error}"
        );
    }
}
