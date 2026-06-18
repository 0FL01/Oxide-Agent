//! SQLx/Postgres storage foundation.
//!
//! Provides the shared Postgres pool, migration runner, and SQL-backed core
//! durable state used by the transport-agnostic [`StorageProvider`].

use async_trait::async_trait;
use serde_json::Value;
use sqlx_core::{migrate::Migrator, query::query};
use sqlx_postgres::{PgPool, PgPoolOptions, Postgres};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    BrowserArtifactData, BrowserArtifactRecord, CreateReminderJobOptions, ReminderJobRecord,
    ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind, StorageError, StorageProvider,
    TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord, TopicContextRecord,
    TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UpsertTopicContextOptions,
    UpsertTopicInfraConfigOptions, UserConfig, UserContextConfig,
    builders::{
        build_agent_flow_record, build_agent_profile_record, build_audit_event_record,
        build_reminder_job_record, build_topic_agents_md_record, build_topic_binding_record,
        build_topic_context_record, build_topic_infra_config_record, with_next_reminder_version,
    },
    utils::current_timestamp_unix_secs,
    validate_topic_agents_md_content, validate_topic_context_content,
};
use crate::agent::memory::AgentMemory;

use super::SqlxStorageConfig;
use crate::agent::wiki_memory::wiki_context_id;

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

    /// Deletes expired wiki rows in a bounded, idempotent batch.
    ///
    /// Retention timestamps are Unix seconds. A zero `limit` is a no-op so
    /// operators cannot accidentally issue an unbounded cleanup.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Database`] when the cleanup query fails.
    pub async fn cleanup_expired_wiki_pages(
        &self,
        now_unix_secs: u64,
        limit: usize,
    ) -> Result<u64, StorageError> {
        if limit == 0 {
            return Ok(0);
        }
        let result = query::<Postgres>(
            r#"
            DELETE FROM wiki_pages
            WHERE ctid IN (
                SELECT ctid
                FROM wiki_pages
                WHERE retention_expires_at IS NOT NULL
                  AND retention_expires_at <= $1
                ORDER BY retention_expires_at ASC,
                         storage_prefix ASC,
                         scope_kind ASC,
                         context_id ASC,
                         path ASC
                LIMIT $2
            )
            "#,
        )
        .bind(u64_to_i64(
            now_unix_secs,
            "wiki retention cleanup timestamp",
        )?)
        .bind(usize_to_i64(limit, "wiki retention cleanup limit")?)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected())
    }

    async fn save_agent_memory_scope(
        &self,
        user_id: i64,
        context_key: &str,
        flow_id: &str,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, user_id).await?;
        let now = current_timestamp_unix_secs();
        let memory = serde_json::to_value(memory)?;

        query::<Postgres>(
            r#"
            INSERT INTO agent_memory_snapshots (
                user_id, context_key, flow_id, memory, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, 1, $5, $5)
            ON CONFLICT (user_id, context_key, flow_id) DO UPDATE
            SET memory = EXCLUDED.memory,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            WHERE agent_memory_snapshots.memory IS DISTINCT FROM EXCLUDED.memory
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(flow_id)
        .bind(memory)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)
    }

    async fn load_agent_memory_scope(
        &self,
        user_id: i64,
        context_key: &str,
        flow_id: &str,
    ) -> Result<Option<AgentMemory>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT memory
            FROM agent_memory_snapshots
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(flow_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| from_json(row_value::<Value>(&row, "memory")?, "agent memory"))
            .transpose()
    }

    async fn clear_agent_memory_scope(
        &self,
        user_id: i64,
        context_key: &str,
        flow_id: &str,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM agent_memory_snapshots
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(flow_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

#[async_trait]
impl StorageProvider for SqlxStorage {
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT state
            FROM user_configs
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        let state = row
            .map(|row| row_value::<Option<String>>(&row, "state"))
            .transpose()?
            .flatten();

        let rows = query::<Postgres>(
            r#"
            SELECT context_key, state, current_agent_flow_id, chat_id, thread_id,
                   forum_topic_name, forum_topic_icon_color,
                   forum_topic_icon_custom_emoji_id, forum_topic_closed
            FROM user_contexts
            WHERE user_id = $1
            ORDER BY context_key ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        let mut contexts = HashMap::with_capacity(rows.len());
        for row in rows {
            let context_key = row_value(&row, "context_key")?;
            contexts.insert(context_key, row_to_user_context(&row)?);
        }

        Ok(UserConfig { state, contexts })
    }

    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, user_id).await?;
        let now = current_timestamp_unix_secs();

        query::<Postgres>(
            r#"
            INSERT INTO user_configs (user_id, state, schema_version, created_at, updated_at)
            VALUES ($1, $2, 1, $3, $3)
            ON CONFLICT (user_id) DO UPDATE
            SET state = EXCLUDED.state,
                schema_version = EXCLUDED.schema_version,
                version = user_configs.version + 1,
                updated_at = EXCLUDED.updated_at
            WHERE user_configs.state IS DISTINCT FROM EXCLUDED.state
            "#,
        )
        .bind(user_id)
        .bind(config.state)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        let mut contexts = config.contexts.into_iter().collect::<Vec<_>>();
        contexts.sort_by(|left, right| left.0.cmp(&right.0));
        let context_keys = contexts
            .iter()
            .map(|(context_key, _)| context_key.clone())
            .collect::<Vec<_>>();

        query::<Postgres>(
            r#"
            DELETE FROM user_contexts
            WHERE user_id = $1 AND context_key <> ALL($2::text[])
            "#,
        )
        .bind(user_id)
        .bind(&context_keys)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        for (context_key, context) in contexts {
            let forum_topic_icon_color = context.forum_topic_icon_color.map(i64::from);
            query::<Postgres>(
                r#"
                INSERT INTO user_contexts (
                    user_id, context_key, state, current_agent_flow_id, chat_id, thread_id,
                    forum_topic_name, forum_topic_icon_color,
                    forum_topic_icon_custom_emoji_id, forum_topic_closed,
                    schema_version, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 1, $11, $11)
                ON CONFLICT (user_id, context_key) DO UPDATE
                SET state = EXCLUDED.state,
                    current_agent_flow_id = EXCLUDED.current_agent_flow_id,
                    chat_id = EXCLUDED.chat_id,
                    thread_id = EXCLUDED.thread_id,
                    forum_topic_name = EXCLUDED.forum_topic_name,
                    forum_topic_icon_color = EXCLUDED.forum_topic_icon_color,
                    forum_topic_icon_custom_emoji_id = EXCLUDED.forum_topic_icon_custom_emoji_id,
                    forum_topic_closed = EXCLUDED.forum_topic_closed,
                    schema_version = EXCLUDED.schema_version,
                    version = user_contexts.version + 1,
                    updated_at = EXCLUDED.updated_at
                WHERE user_contexts.state IS DISTINCT FROM EXCLUDED.state
                   OR user_contexts.current_agent_flow_id IS DISTINCT FROM EXCLUDED.current_agent_flow_id
                   OR user_contexts.chat_id IS DISTINCT FROM EXCLUDED.chat_id
                   OR user_contexts.thread_id IS DISTINCT FROM EXCLUDED.thread_id
                   OR user_contexts.forum_topic_name IS DISTINCT FROM EXCLUDED.forum_topic_name
                   OR user_contexts.forum_topic_icon_color IS DISTINCT FROM EXCLUDED.forum_topic_icon_color
                   OR user_contexts.forum_topic_icon_custom_emoji_id IS DISTINCT FROM EXCLUDED.forum_topic_icon_custom_emoji_id
                   OR user_contexts.forum_topic_closed IS DISTINCT FROM EXCLUDED.forum_topic_closed
                "#,
            )
            .bind(user_id)
            .bind(context_key)
            .bind(context.state)
            .bind(context.current_agent_flow_id)
            .bind(context.chat_id)
            .bind(context.thread_id)
            .bind(context.forum_topic_name)
            .bind(forum_topic_icon_color)
            .bind(context.forum_topic_icon_custom_emoji_id)
            .bind(context.forum_topic_closed)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }

        tx.commit().await.map_err(db_error)
    }

    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, user_id).await?;
        let now = current_timestamp_unix_secs();

        query::<Postgres>(
            r#"
            INSERT INTO user_configs (user_id, state, schema_version, created_at, updated_at)
            VALUES ($1, $2, 1, $3, $3)
            ON CONFLICT (user_id) DO UPDATE
            SET state = EXCLUDED.state,
                schema_version = EXCLUDED.schema_version,
                version = user_configs.version + 1,
                updated_at = EXCLUDED.updated_at
            WHERE user_configs.state IS DISTINCT FROM EXCLUDED.state
            "#,
        )
        .bind(user_id)
        .bind(state)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)
    }

    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT state
            FROM user_configs
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_value::<Option<String>>(&row, "state"))
            .transpose()
            .map(Option::flatten)
    }

    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_scope(user_id, "", "", memory).await
    }

    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_scope(user_id, &context_key, "", memory)
            .await
    }

    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_scope(user_id, "", "").await
    }

    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_scope(user_id, &context_key, "")
            .await
    }

    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_agent_memory_scope(user_id, "", "").await
    }

    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        query::<Postgres>(
            r#"
            DELETE FROM agent_memory_snapshots
            WHERE user_id = $1 AND context_key = $2
            "#,
        )
        .bind(user_id)
        .bind(&context_key)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            DELETE FROM agent_flows
            WHERE user_id = $1 AND context_key = $2
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)
    }

    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_scope(user_id, &context_key, &flow_id, memory)
            .await
    }

    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_scope(user_id, &context_key, &flow_id)
            .await
    }

    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        query::<Postgres>(
            r#"
            DELETE FROM agent_memory_snapshots
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(user_id)
        .bind(&context_key)
        .bind(&flow_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            DELETE FROM agent_flows
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(flow_id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)
    }

    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, context_key, flow_id, schema_version, created_at, updated_at
            FROM agent_flows
            WHERE user_id = $1 AND context_key = $2 AND flow_id = $3
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(flow_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_agent_flow(&row)).transpose()
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("agent_flow:{user_id}:{context_key}:{flow_id}"),
        )
        .await?;
        let existing =
            get_agent_flow_record_for_update(&mut tx, user_id, &context_key, &flow_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_agent_flow_record(user_id, context_key, flow_id, existing, now);

        query::<Postgres>(
            r#"
            INSERT INTO agent_flows (
                user_id, context_key, flow_id, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (user_id, context_key, flow_id) DO UPDATE
            SET schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.context_key)
        .bind(&record.flow_id)
        .bind(u32_to_i32(
            record.schema_version,
            "agent flow schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn load_wiki_text(&self, storage_key: String) -> Result<Option<String>, StorageError> {
        let address = parse_wiki_storage_key(&storage_key)?;
        let row = query::<Postgres>(
            r#"
            SELECT content
            FROM wiki_pages
            WHERE storage_prefix = $1
              AND scope_kind = $2
              AND context_id = $3
              AND path = $4
            "#,
        )
        .bind(&address.storage_prefix)
        .bind(address.scope_kind.as_str())
        .bind(&address.context_id)
        .bind(&address.path)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_value::<String>(&row, "content"))
            .transpose()
    }

    async fn save_wiki_text(
        &self,
        storage_key: String,
        content: String,
    ) -> Result<(), StorageError> {
        let address = parse_wiki_storage_key(&storage_key)?;
        validate_wiki_content_size(&address, &content)?;
        let now = current_timestamp_unix_secs();
        let content_bytes = usize_to_i64(content.len(), "wiki content_bytes")?;

        query::<Postgres>(
            r#"
            INSERT INTO wiki_pages (
                storage_prefix, scope_kind, context_id, item_kind, path, content,
                content_bytes, retention_expires_at, version, schema_version,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, 1, $8, $9, $9)
            ON CONFLICT (storage_prefix, scope_kind, context_id, path) DO UPDATE
            SET item_kind = EXCLUDED.item_kind,
                content = EXCLUDED.content,
                content_bytes = EXCLUDED.content_bytes,
                schema_version = EXCLUDED.schema_version,
                version = wiki_pages.version + 1,
                updated_at = EXCLUDED.updated_at
            WHERE wiki_pages.content IS DISTINCT FROM EXCLUDED.content
            "#,
        )
        .bind(&address.storage_prefix)
        .bind(address.scope_kind.as_str())
        .bind(&address.context_id)
        .bind(address.item_kind.as_str())
        .bind(&address.path)
        .bind(content)
        .bind(content_bytes)
        .bind(WIKI_SCHEMA_VERSION)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn save_browser_artifact(
        &self,
        record: BrowserArtifactRecord,
    ) -> Result<(), StorageError> {
        let bytes_i64 = usize_to_i64(record.data.len(), "browser artifact bytes")?;
        query::<Postgres>(
            r#"
            INSERT INTO browser_artifacts (
                artifact_uri, user_id, session_id, task_id,
                mime_type, data, bytes, sha256
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (artifact_uri) DO UPDATE
            SET mime_type = EXCLUDED.mime_type,
                data = EXCLUDED.data,
                bytes = EXCLUDED.bytes,
                sha256 = EXCLUDED.sha256
            "#,
        )
        .bind(&record.artifact_uri)
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.task_id)
        .bind(&record.mime_type)
        .bind(&record.data)
        .bind(bytes_i64)
        .bind(&record.sha256)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn load_browser_artifact(
        &self,
        artifact_uri: &str,
    ) -> Result<Option<BrowserArtifactData>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT mime_type, data, bytes
            FROM browser_artifacts
            WHERE artifact_uri = $1
            "#,
        )
        .bind(artifact_uri)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| {
            let mime_type = row_value::<String>(&row, "mime_type")?;
            let bytes = row_value::<i64>(&row, "bytes")?;
            let data = row_value::<Vec<u8>>(&row, "data")?;
            Ok(BrowserArtifactData {
                mime_type,
                data,
                bytes,
            })
        })
        .transpose()
    }

    async fn delete_browser_artifacts_by_session(
        &self,
        session_id: &str,
    ) -> Result<u64, StorageError> {
        let result = query::<Postgres>(
            r#"
            DELETE FROM browser_artifacts
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected())
    }

    async fn delete_wiki_text(&self, storage_key: String) -> Result<(), StorageError> {
        let address = parse_wiki_storage_key(&storage_key)?;
        query::<Postgres>(
            r#"
            DELETE FROM wiki_pages
            WHERE storage_prefix = $1
              AND scope_kind = $2
              AND context_id = $3
              AND path = $4
            "#,
        )
        .bind(&address.storage_prefix)
        .bind(address.scope_kind.as_str())
        .bind(&address.context_id)
        .bind(&address.path)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn delete_wiki_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let context_id = wiki_context_id(user_id, &context_key);
        query::<Postgres>(
            r#"
            DELETE FROM wiki_pages
            WHERE scope_kind = 'context' AND context_id = $1
            "#,
        )
        .bind(context_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_agent_memory(user_id).await
    }

    async fn check_connection(&self) -> Result<(), String> {
        self.check_database_connection()
            .await
            .map_err(|error| error.to_string())
    }

    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, agent_id, profile, version, schema_version, created_at, updated_at
            FROM agent_profiles
            WHERE user_id = $1 AND agent_id = $2
            "#,
        )
        .bind(user_id)
        .bind(agent_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_agent_profile(&row)).transpose()
    }

    async fn list_agent_profiles(
        &self,
        user_id: i64,
    ) -> Result<Vec<AgentProfileRecord>, StorageError> {
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, agent_id, profile, version, schema_version, created_at, updated_at
            FROM agent_profiles
            WHERE user_id = $1
            ORDER BY agent_id ASC, updated_at ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;

        rows.iter().map(row_to_agent_profile).collect()
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("agent_profile:{}:{}", options.user_id, options.agent_id),
        )
        .await?;
        let existing =
            get_agent_profile_for_update(&mut tx, options.user_id, &options.agent_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_agent_profile_record(options, existing, now);

        query::<Postgres>(
            r#"
            INSERT INTO agent_profiles (
                user_id, agent_id, profile, version, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id, agent_id) DO UPDATE
            SET profile = EXCLUDED.profile,
                version = EXCLUDED.version,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.agent_id)
        .bind(&record.profile)
        .bind(u64_to_i64(record.version, "agent profile version")?)
        .bind(u32_to_i32(
            record.schema_version,
            "agent profile schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM agent_profiles
            WHERE user_id = $1 AND agent_id = $2
            "#,
        )
        .bind(user_id)
        .bind(agent_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, topic_id, context, version, schema_version, created_at, updated_at
            FROM topic_contexts
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_topic_context(&row)).transpose()
    }

    async fn upsert_topic_context(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        let context = validate_topic_context_content(&options.context)?;
        let options = UpsertTopicContextOptions { context, ..options };
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("topic_prompt:{}:{}", options.user_id, options.topic_id),
        )
        .await?;
        ensure_topic_prompt_not_duplicated_in_tx(
            &mut tx,
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::Context,
            &options.context,
        )
        .await?;
        let existing =
            get_topic_context_for_update(&mut tx, options.user_id, &options.topic_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_topic_context_record(options, existing, now);

        query::<Postgres>(
            r#"
            INSERT INTO topic_contexts (
                user_id, topic_id, context, version, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id, topic_id) DO UPDATE
            SET context = EXCLUDED.context,
                version = EXCLUDED.version,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.topic_id)
        .bind(&record.context)
        .bind(u64_to_i64(record.version, "topic context version")?)
        .bind(u32_to_i32(
            record.schema_version,
            "topic context schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn delete_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM topic_contexts
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, topic_id, agents_md, version, schema_version, created_at, updated_at
            FROM topic_agents_md
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_topic_agents_md(&row)).transpose()
    }

    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        let agents_md = validate_topic_agents_md_content(&options.agents_md)?;
        let options = UpsertTopicAgentsMdOptions {
            agents_md,
            ..options
        };
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("topic_prompt:{}:{}", options.user_id, options.topic_id),
        )
        .await?;
        ensure_topic_prompt_not_duplicated_in_tx(
            &mut tx,
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::AgentsMd,
            &options.agents_md,
        )
        .await?;
        let existing =
            get_topic_agents_md_for_update(&mut tx, options.user_id, &options.topic_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_topic_agents_md_record(options, existing, now);

        query::<Postgres>(
            r#"
            INSERT INTO topic_agents_md (
                user_id, topic_id, agents_md, version, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (user_id, topic_id) DO UPDATE
            SET agents_md = EXCLUDED.agents_md,
                version = EXCLUDED.version,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.topic_id)
        .bind(&record.agents_md)
        .bind(u64_to_i64(record.version, "topic AGENTS.md version")?)
        .bind(u32_to_i32(
            record.schema_version,
            "topic AGENTS.md schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM topic_agents_md
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn get_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, topic_id, target_name, host, port, remote_user, auth_mode,
                   secret_ref, sudo_secret_ref, environment, tags, allowed_tool_modes,
                   version, schema_version, created_at, updated_at
            FROM topic_infra_configs
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_topic_infra_config(&row)).transpose()
    }

    async fn upsert_topic_infra_config(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("topic_infra:{}:{}", options.user_id, options.topic_id),
        )
        .await?;
        let existing =
            get_topic_infra_config_for_update(&mut tx, options.user_id, &options.topic_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_topic_infra_config_record(options, existing, now);
        let auth_mode = enum_to_sql(&record.auth_mode, "topic infra auth mode")?;
        let allowed_tool_modes =
            enum_vec_to_sql(&record.allowed_tool_modes, "topic infra tool mode")?;

        query::<Postgres>(
            r#"
            INSERT INTO topic_infra_configs (
                user_id, topic_id, target_name, host, port, remote_user, auth_mode,
                secret_ref, sudo_secret_ref, environment, tags, allowed_tool_modes,
                version, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
            ON CONFLICT (user_id, topic_id) DO UPDATE
            SET target_name = EXCLUDED.target_name,
                host = EXCLUDED.host,
                port = EXCLUDED.port,
                remote_user = EXCLUDED.remote_user,
                auth_mode = EXCLUDED.auth_mode,
                secret_ref = EXCLUDED.secret_ref,
                sudo_secret_ref = EXCLUDED.sudo_secret_ref,
                environment = EXCLUDED.environment,
                tags = EXCLUDED.tags,
                allowed_tool_modes = EXCLUDED.allowed_tool_modes,
                version = EXCLUDED.version,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.topic_id)
        .bind(&record.target_name)
        .bind(&record.host)
        .bind(u16_to_i32(record.port))
        .bind(&record.remote_user)
        .bind(auth_mode)
        .bind(&record.secret_ref)
        .bind(&record.sudo_secret_ref)
        .bind(&record.environment)
        .bind(&record.tags)
        .bind(&allowed_tool_modes)
        .bind(u64_to_i64(record.version, "topic infra version")?)
        .bind(u32_to_i32(
            record.schema_version,
            "topic infra schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn delete_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM topic_infra_configs
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn get_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT secret_value
            FROM private_secrets
            WHERE user_id = $1 AND secret_ref = $2
            "#,
        )
        .bind(user_id)
        .bind(secret_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_value::<String>(&row, "secret_value"))
            .transpose()
    }

    async fn put_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, user_id).await?;
        let now = current_timestamp_unix_secs();
        query::<Postgres>(
            r#"
            INSERT INTO private_secrets (user_id, secret_ref, secret_value, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $4)
            ON CONFLICT (user_id, secret_ref) DO UPDATE
            SET secret_value = EXCLUDED.secret_value,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(user_id)
        .bind(secret_ref)
        .bind(value)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)
    }

    async fn delete_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM private_secrets
            WHERE user_id = $1 AND secret_ref = $2
            "#,
        )
        .bind(user_id)
        .bind(secret_ref)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, topic_id, agent_id, binding_kind, chat_id, thread_id,
                   expires_at, last_activity_at, version, schema_version, created_at, updated_at
            FROM topic_bindings
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;

        row.map(|row| row_to_topic_binding(&row)).transpose()
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        advisory_xact_lock(
            &mut tx,
            &format!("topic_binding:{}:{}", options.user_id, options.topic_id),
        )
        .await?;
        let existing =
            get_topic_binding_for_update(&mut tx, options.user_id, &options.topic_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_topic_binding_record(options, existing, now);
        let binding_kind = enum_to_sql(&record.binding_kind, "topic binding kind")?;

        query::<Postgres>(
            r#"
            INSERT INTO topic_bindings (
                user_id, topic_id, agent_id, binding_kind, chat_id, thread_id,
                expires_at, last_activity_at, version, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (user_id, topic_id) DO UPDATE
            SET agent_id = EXCLUDED.agent_id,
                binding_kind = EXCLUDED.binding_kind,
                chat_id = EXCLUDED.chat_id,
                thread_id = EXCLUDED.thread_id,
                expires_at = EXCLUDED.expires_at,
                last_activity_at = EXCLUDED.last_activity_at,
                version = EXCLUDED.version,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.topic_id)
        .bind(&record.agent_id)
        .bind(binding_kind)
        .bind(record.chat_id)
        .bind(record.thread_id)
        .bind(record.expires_at)
        .bind(record.last_activity_at)
        .bind(u64_to_i64(record.version, "topic binding version")?)
        .bind(u32_to_i32(
            record.schema_version,
            "topic binding schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM topic_bindings
            WHERE user_id = $1 AND topic_id = $2
            "#,
        )
        .bind(user_id)
        .bind(topic_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        let now = current_timestamp_unix_secs();

        query::<Postgres>(
            r#"
            INSERT INTO audit_stream_versions (user_id, next_version, created_at, updated_at)
            VALUES ($1, 1, $2, $2)
            ON CONFLICT (user_id) DO NOTHING
            "#,
        )
        .bind(options.user_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        let row = query::<Postgres>(
            r#"
            SELECT next_version
            FROM audit_stream_versions
            WHERE user_id = $1
            FOR UPDATE
            "#,
        )
        .bind(options.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        let next_version = i64_to_u64(row_value(&row, "next_version")?, "audit next_version")?;
        let current_version = next_version.checked_sub(1);
        let record = build_audit_event_record(
            options,
            current_version.filter(|version| *version > 0),
            now,
            Uuid::new_v4().to_string(),
        );

        query::<Postgres>(
            r#"
            INSERT INTO audit_events (
                user_id, version, event_id, topic_id, agent_id, action, payload,
                schema_version, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(record.user_id)
        .bind(u64_to_i64(record.version, "audit version")?)
        .bind(&record.event_id)
        .bind(&record.topic_id)
        .bind(&record.agent_id)
        .bind(&record.action)
        .bind(&record.payload)
        .bind(u32_to_i32(record.schema_version, "audit schema_version")?)
        .bind(record.created_at)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            UPDATE audit_stream_versions
            SET next_version = next_version + 1,
                updated_at = $2
            WHERE user_id = $1
            "#,
        )
        .bind(record.user_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?;

        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, version, event_id, topic_id, agent_id, action, payload,
                   schema_version, created_at
            FROM (
                SELECT user_id, version, event_id, topic_id, agent_id, action, payload,
                       schema_version, created_at
                FROM audit_events
                WHERE user_id = $1
                ORDER BY version DESC
                LIMIT $2
            ) recent
            ORDER BY version ASC
            "#,
        )
        .bind(user_id)
        .bind(usize_to_i64(limit, "audit limit")?)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.iter().map(row_to_audit_event).collect()
    }

    async fn list_audit_events_page(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let before_version = before_version
            .map(|version| u64_to_i64(version, "audit before_version"))
            .transpose()?;
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, version, event_id, topic_id, agent_id, action, payload,
                   schema_version, created_at
            FROM audit_events
            WHERE user_id = $1
              AND ($2::BIGINT IS NULL OR version < $2)
            ORDER BY version DESC
            LIMIT $3
            "#,
        )
        .bind(user_id)
        .bind(before_version)
        .bind(usize_to_i64(limit, "audit page limit")?)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.iter().map(row_to_audit_event).collect()
    }

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        ensure_user_row_in_tx(&mut tx, options.user_id).await?;
        let now = current_timestamp_unix_secs();
        let record = build_reminder_job_record(options, Uuid::new_v4().to_string(), now);
        insert_reminder_job_in_tx(&mut tx, &record).await?;
        tx.commit().await.map_err(db_error)?;
        Ok(record)
    }

    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let row = query::<Postgres>(
            r#"
            SELECT user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
                   thread_kind, task_prompt, schedule_kind, status, next_run_at,
                   interval_secs, cron_expression, timezone, lease_until,
                   last_run_at, last_error, run_count, version, schema_version,
                   created_at, updated_at
            FROM reminder_jobs
            WHERE user_id = $1 AND reminder_id = $2
            "#,
        )
        .bind(user_id)
        .bind(reminder_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(|row| row_to_reminder_job(&row)).transpose()
    }

    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let status_values = statuses
            .as_ref()
            .map(|statuses| enum_vec_to_sql(statuses, "reminder status"))
            .transpose()?;
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
                   thread_kind, task_prompt, schedule_kind, status, next_run_at,
                   interval_secs, cron_expression, timezone, lease_until,
                   last_run_at, last_error, run_count, version, schema_version,
                   created_at, updated_at
            FROM reminder_jobs
            WHERE user_id = $1
              AND ($2::TEXT IS NULL OR context_key = $2)
              AND ($3::TEXT[] IS NULL OR status = ANY($3))
            ORDER BY next_run_at DESC, created_at DESC
            LIMIT $4
            "#,
        )
        .bind(user_id)
        .bind(context_key)
        .bind(status_values)
        .bind(usize_to_i64(limit, "reminder list limit")?)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.iter().map(row_to_reminder_job).collect()
    }

    async fn list_due_reminder_jobs(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
                   thread_kind, task_prompt, schedule_kind, status, next_run_at,
                   interval_secs, cron_expression, timezone, lease_until,
                   last_run_at, last_error, run_count, version, schema_version,
                   created_at, updated_at
            FROM reminder_jobs
            WHERE user_id = $1
              AND status = 'scheduled'
              AND next_run_at <= $2
              AND (lease_until IS NULL OR lease_until <= $2)
            ORDER BY next_run_at ASC, created_at ASC
            LIMIT $3
            "#,
        )
        .bind(user_id)
        .bind(now)
        .bind(usize_to_i64(limit, "due reminder list limit")?)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        rows.iter().map(row_to_reminder_job).collect()
    }

    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mutation_now = current_timestamp_unix_secs();
        let row = query::<Postgres>(
            r#"
            UPDATE reminder_jobs
            SET lease_until = $3,
                version = version + 1,
                updated_at = $4
            WHERE user_id = $1
              AND reminder_id = $2
              AND status = 'scheduled'
              AND next_run_at <= $5
              AND (lease_until IS NULL OR lease_until <= $5)
            RETURNING user_id, reminder_id, context_key, flow_id, chat_id, thread_id,
                      thread_kind, task_prompt, schedule_kind, status, next_run_at,
                      interval_secs, cron_expression, timezone, lease_until,
                      last_run_at, last_error, run_count, version, schema_version,
                      created_at, updated_at
            "#,
        )
        .bind(user_id)
        .bind(reminder_id)
        .bind(lease_until)
        .bind(mutation_now)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_error)?;
        row.map(|row| row_to_reminder_job(&row)).transpose()
    }

    async fn reschedule_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        last_run_at: Option<i64>,
        last_error: Option<String>,
        increment_run_count: bool,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: last_run_at.or(record.last_run_at),
                last_error,
                run_count: if increment_run_count {
                    record.run_count.saturating_add(1)
                } else {
                    record.run_count
                },
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Completed,
                lease_until: None,
                last_run_at: Some(completed_at),
                last_error: None,
                run_count: record.run_count.saturating_add(1),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Failed,
                lease_until: None,
                last_run_at: Some(failed_at),
                last_error: Some(error),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Cancelled,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(cancelled_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Paused,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(paused_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Paused {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(resumed_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        mutate_reminder_job(self, user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Failed {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(retried_at)),
                last_error: None,
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        query::<Postgres>(
            r#"
            DELETE FROM reminder_jobs
            WHERE user_id = $1 AND reminder_id = $2
            "#,
        )
        .bind(user_id)
        .bind(reminder_id)
        .execute(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

mod helpers;
use helpers::{
    advisory_xact_lock, db_error, ensure_user_row_in_tx, enum_to_sql, enum_vec_to_sql, from_json,
    i64_to_u64, row_value, u16_to_i32, u32_to_i32, u64_to_i64, usize_to_i64,
};

mod rows;
mod wiki;

use rows::{
    row_to_agent_flow, row_to_agent_profile, row_to_audit_event, row_to_reminder_job,
    row_to_topic_agents_md, row_to_topic_binding, row_to_topic_context, row_to_topic_infra_config,
    row_to_user_context,
};
use wiki::{WIKI_SCHEMA_VERSION, parse_wiki_storage_key, validate_wiki_content_size};

mod reminder_tx;
use reminder_tx::{insert_reminder_job_in_tx, mutate_reminder_job};

mod topic_tx;
use topic_tx::{
    TopicPromptStoreKind, ensure_topic_prompt_not_duplicated_in_tx,
    get_agent_flow_record_for_update, get_agent_profile_for_update, get_topic_agents_md_for_update,
    get_topic_binding_for_update, get_topic_context_for_update, get_topic_infra_config_for_update,
};

#[cfg(test)]
mod tests;
