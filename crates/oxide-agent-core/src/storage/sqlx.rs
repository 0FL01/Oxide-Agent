//! SQLx/Postgres storage foundation.
//!
//! Phase 1 wires the shared Postgres pool, connectivity check, and migration
//! runner. Business storage methods intentionally remain unsupported until the
//! later porting phases replace R2 object operations with SQL entities.

use async_trait::async_trait;
use sqlx_core::migrate::Migrator;
use sqlx_core::query::query;
use sqlx_postgres::{PgPool, PgPoolOptions, Postgres};

use super::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, StorageError,
    StorageProvider, TopicBindingRecord, UpsertAgentProfileOptions, UpsertTopicBindingOptions,
    UserConfig,
};
use crate::agent::memory::AgentMemory;

use super::SqlxStorageConfig;

/// Shared SQLx/Postgres handle for durable storage.
#[derive(Clone)]
pub struct SqlxStorage {
    config: SqlxStorageConfig,
    pool: PgPool,
}

impl SqlxStorage {
    /// Builds the shared Postgres pool and verifies connectivity.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] when the pool or health query fails,
    /// and [`StorageError::DatabaseMigration`] when startup migrations fail.
    pub async fn connect(config: SqlxStorageConfig) -> Result<Self, StorageError> {
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(config.connect_timeout)
            .connect(&config.database_url)
            .await
            .map_err(|error| StorageError::Database(error.to_string()))?;

        let storage = Self { config, pool };
        storage.check_database_connection().await?;
        if storage.config.migrate_on_startup {
            storage.run_configured_migrations().await?;
        }

        Ok(storage)
    }

    /// Returns the shared SQLx pool.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Returns the resolved SQLx storage config.
    #[must_use]
    pub const fn config(&self) -> &SqlxStorageConfig {
        &self.config
    }

    /// Runs a minimal database health query against the shared pool.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] when the query fails.
    pub async fn check_database_connection(&self) -> Result<(), StorageError> {
        query::<Postgres>("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|error| StorageError::Database(error.to_string()))?;
        Ok(())
    }

    /// Runs configured startup migrations.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::DatabaseMigration`] when migration discovery or
    /// execution fails.
    pub async fn run_configured_migrations(&self) -> Result<(), StorageError> {
        self.run_migrations_from_path(&self.config.migrations_dir)
            .await
    }

    /// Runs SQLx migrations from a runtime path.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::DatabaseMigration`] when migration discovery or
    /// execution fails.
    pub async fn run_migrations_from_path(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<(), StorageError> {
        let migrator = Migrator::new(path.as_ref())
            .await
            .map_err(|error| StorageError::DatabaseMigration(error.to_string()))?;

        migrator
            .run(&self.pool)
            .await
            .map_err(|error| StorageError::DatabaseMigration(error.to_string()))
    }
}

fn unsupported<T>(operation: &str) -> Result<T, StorageError> {
    Err(StorageError::Unsupported(format!(
        "SQLx storage operation `{operation}` is not implemented until the SQL entity porting phases"
    )))
}

#[async_trait]
impl StorageProvider for SqlxStorage {
    async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
        unsupported("get_user_config")
    }

    async fn update_user_config(
        &self,
        _user_id: i64,
        _config: UserConfig,
    ) -> Result<(), StorageError> {
        unsupported("update_user_config")
    }

    async fn update_user_state(&self, _user_id: i64, _state: String) -> Result<(), StorageError> {
        unsupported("update_user_state")
    }

    async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
        unsupported("get_user_state")
    }

    async fn save_agent_memory(
        &self,
        _user_id: i64,
        _memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        unsupported("save_agent_memory")
    }

    async fn load_agent_memory(&self, _user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        unsupported("load_agent_memory")
    }

    async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
        unsupported("clear_agent_memory")
    }

    async fn get_agent_flow_record(
        &self,
        _user_id: i64,
        _context_key: String,
        _flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        unsupported("get_agent_flow_record")
    }

    async fn upsert_agent_flow_record(
        &self,
        _user_id: i64,
        _context_key: String,
        _flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        unsupported("upsert_agent_flow_record")
    }

    async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
        unsupported("clear_all_context")
    }

    async fn check_connection(&self) -> Result<(), String> {
        self.check_database_connection()
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        unsupported("get_agent_profile")
    }

    async fn upsert_agent_profile(
        &self,
        _options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        unsupported("upsert_agent_profile")
    }

    async fn delete_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<(), StorageError> {
        unsupported("delete_agent_profile")
    }

    async fn get_topic_binding(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        unsupported("get_topic_binding")
    }

    async fn upsert_topic_binding(
        &self,
        _options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        unsupported("upsert_topic_binding")
    }

    async fn delete_topic_binding(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<(), StorageError> {
        unsupported("delete_topic_binding")
    }

    async fn append_audit_event(
        &self,
        _options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        unsupported("append_audit_event")
    }

    async fn list_audit_events(
        &self,
        _user_id: i64,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        unsupported("list_audit_events")
    }

    async fn list_audit_events_page(
        &self,
        _user_id: i64,
        _before_version: Option<u64>,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        unsupported("list_audit_events_page")
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{SqlxStorage, SqlxStorageConfig};

    #[tokio::test]
    async fn sqlx_storage_connects_and_runs_migrations_when_test_url_is_set() {
        let Ok(database_url) = std::env::var("OXIDE_DATABASE_TEST_URL") else {
            eprintln!("OXIDE_DATABASE_TEST_URL not set; skipping SQLx/Postgres smoke test");
            return;
        };

        let migrations_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("migrations");
        let config = SqlxStorageConfig {
            database_url,
            max_connections: 1,
            connect_timeout: Duration::from_secs(5),
            migrate_on_startup: true,
            migrations_dir,
        };

        let storage = SqlxStorage::connect(config)
            .await
            .expect("SQLx storage should connect and run foundation migrations");

        storage
            .check_database_connection()
            .await
            .expect("SQLx storage health query should pass after migrations");
    }
}
