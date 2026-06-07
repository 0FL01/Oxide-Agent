//! SQLx/Postgres web console persistence.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use oxide_agent_core::storage::SqlxStorage;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, SessionSummary, TaskEventsResponse, WebSessionRecord, WebTaskRecord,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use sqlx_core::query::query;
use sqlx_core::row::Row;
use sqlx_postgres::{PgPool, PgRow, Postgres};
use uuid::Uuid;

use super::{
    LoginIndexRecord, ValidateWebRecord, WebAuthSessionRecord, WebSessionContextKeys,
    WebTaskEventState, WebTaskFileBlob, WebTaskFileRecord, WebUiStore, WebUiStoreError,
    WebUiStoreResult, WebUserRecord, WEB_AUTH_SCHEMA_VERSION,
};

const DEFAULT_TASK_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const TASK_FILE_MAX_BYTES_ENV: &str = "OXIDE_WEB_TASK_FILE_MAX_BYTES";
const WEB_LATENCY_TARGET: &str = "oxide_agent_transport_web::web_latency";

fn log_store_query(
    operation: &'static str,
    started_at: Instant,
    user_id: Option<i64>,
    session_id: Option<&str>,
    task_id: Option<&str>,
    rows_affected: Option<u64>,
    row_count: Option<usize>,
    found: Option<bool>,
) {
    tracing::info!(
        target: WEB_LATENCY_TARGET,
        operation,
        user_id = ?user_id,
        session_id = ?session_id,
        task_id = ?task_id,
        rows_affected = ?rows_affected,
        row_count = ?row_count,
        found = ?found,
        elapsed_ms = started_at.elapsed().as_millis(),
        "web sqlx store latency"
    );
}

/// SQLx-backed implementation of [`WebUiStore`].
#[derive(Clone)]
pub struct SqlxWebUiStore {
    storage: Arc<SqlxStorage>,
    max_task_file_bytes: u64,
}

/// Row counts removed by one bounded SQLx web retention cleanup pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpiredWebRecordsCleanup {
    pub auth_sessions: u64,
    pub task_events: u64,
    pub task_files: u64,
}

impl SqlxWebUiStore {
    /// Creates a web store from the shared SQLx storage handle.
    #[must_use]
    pub fn new(storage: Arc<SqlxStorage>) -> Self {
        Self {
            storage,
            max_task_file_bytes: max_task_file_bytes_from_env(),
        }
    }

    /// Creates a web store with an explicit task-file size limit.
    #[must_use]
    pub fn with_max_task_file_bytes(storage: Arc<SqlxStorage>, max_task_file_bytes: u64) -> Self {
        Self {
            storage,
            max_task_file_bytes,
        }
    }

    /// Deletes expired web records in bounded per-table batches.
    ///
    /// A zero `limit_per_table` is a no-op. Task file cleanup removes blob rows
    /// through the existing foreign-key cascade.
    pub async fn cleanup_expired_records(
        &self,
        now: DateTime<Utc>,
        limit_per_table: usize,
    ) -> WebUiStoreResult<ExpiredWebRecordsCleanup> {
        if limit_per_table == 0 {
            return Ok(ExpiredWebRecordsCleanup::default());
        }
        let limit = usize_to_i64(limit_per_table, "web retention cleanup limit")?;
        let auth_sessions = query::<Postgres>(
            r#"
            DELETE FROM auth_sessions
            WHERE ctid IN (
                SELECT ctid
                FROM auth_sessions
                WHERE expires_at <= $1 OR (revoked_at IS NOT NULL AND revoked_at <= $1)
                ORDER BY COALESCE(revoked_at, expires_at) ASC, session_token_hash ASC
                LIMIT $2
            )
            "#,
        )
        .bind(now)
        .bind(limit)
        .execute(self.pool())
        .await
        .map_err(db_error)?
        .rows_affected();
        let task_events = query::<Postgres>(
            r#"
            DELETE FROM web_task_events
            WHERE ctid IN (
                SELECT ctid
                FROM web_task_events
                WHERE retention_expires_at IS NOT NULL
                  AND retention_expires_at <= $1
                ORDER BY retention_expires_at ASC, id ASC
                LIMIT $2
            )
            "#,
        )
        .bind(now)
        .bind(limit)
        .execute(self.pool())
        .await
        .map_err(db_error)?
        .rows_affected();
        let task_files = query::<Postgres>(
            r#"
            DELETE FROM web_task_files
            WHERE ctid IN (
                SELECT ctid
                FROM web_task_files
                WHERE retention_expires_at IS NOT NULL
                  AND retention_expires_at <= $1
                ORDER BY retention_expires_at ASC,
                         created_at ASC,
                         user_id ASC,
                         session_id ASC,
                         task_id ASC,
                         file_id ASC
                LIMIT $2
            )
            "#,
        )
        .bind(now)
        .bind(limit)
        .execute(self.pool())
        .await
        .map_err(db_error)?
        .rows_affected();

        Ok(ExpiredWebRecordsCleanup {
            auth_sessions,
            task_events,
            task_files,
        })
    }

    fn pool(&self) -> &PgPool {
        self.storage.pool()
    }

    async fn ensure_user_row(
        &self,
        user_id: i64,
        timestamp: DateTime<Utc>,
    ) -> WebUiStoreResult<()> {
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO users (user_id, created_at, updated_at)
            VALUES ($1, $2, $2)
            ON CONFLICT (user_id) DO UPDATE
            SET updated_at = GREATEST(users.updated_at, EXCLUDED.updated_at)
            "#,
        )
        .bind(user_id)
        .bind(timestamp)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "ensure_user_row",
            started_at,
            Some(user_id),
            None,
            None,
            Some(result.rows_affected()),
            None,
            None,
        );
        Ok(())
    }

    async fn upsert_user_record(&self, record: &WebUserRecord) -> WebUiStoreResult<()> {
        let role = enum_to_sql(&record.role, "web user role")?;
        let status = enum_to_sql(&record.status, "web user status")?;
        let model_selection = optional_json(&record.default_model_selection, "model selection")?;
        let effort = optional_enum_to_sql(&record.default_effort, "agent effort")?;
        let schema_version = u32_to_i32(record.schema_version, "web user schema_version")?;

        query::<Postgres>(
            r#"
            INSERT INTO web_users (
                user_id, login, normalized_login, password_hash, role, status,
                default_model_selection, default_agent_profile_id, default_effort,
                last_login_at, schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (user_id) DO UPDATE SET
                login = EXCLUDED.login,
                normalized_login = EXCLUDED.normalized_login,
                password_hash = EXCLUDED.password_hash,
                role = EXCLUDED.role,
                status = EXCLUDED.status,
                default_model_selection = EXCLUDED.default_model_selection,
                default_agent_profile_id = EXCLUDED.default_agent_profile_id,
                default_effort = EXCLUDED.default_effort,
                last_login_at = EXCLUDED.last_login_at,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.login)
        .bind(&record.normalized_login)
        .bind(&record.password_hash)
        .bind(role)
        .bind(status)
        .bind(model_selection)
        .bind(&record.default_agent_profile_id)
        .bind(effort)
        .bind(record.last_login_at)
        .bind(schema_version)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(self.pool())
        .await
        .map_err(|error| login_conflict_error(error, &record.normalized_login))?;
        Ok(())
    }

    async fn upsert_password_identity(&self, record: &WebUserRecord) -> WebUiStoreResult<()> {
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            DELETE FROM login_identities
            WHERE user_id = $1
              AND provider = 'password'
              AND normalized_login IS DISTINCT FROM $2
            "#,
        )
        .bind(record.user_id)
        .bind(&record.normalized_login)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "upsert_password_identity.delete_stale",
            started_at,
            Some(record.user_id),
            None,
            None,
            Some(result.rows_affected()),
            None,
            None,
        );

        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO login_identities (
                identity_id, user_id, provider, provider_subject,
                normalized_login, password_hash, created_at, updated_at
            )
            VALUES ($1, $2, 'password', $3, $3, $4, $5, $6)
            ON CONFLICT (provider, provider_subject) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                normalized_login = EXCLUDED.normalized_login,
                password_hash = EXCLUDED.password_hash,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(record.user_id)
        .bind(&record.normalized_login)
        .bind(&record.password_hash)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(self.pool())
        .await
        .map_err(|error| login_conflict_error(error, &record.normalized_login))?;
        log_store_query(
            "upsert_password_identity.upsert",
            started_at,
            Some(record.user_id),
            None,
            None,
            Some(result.rows_affected()),
            None,
            None,
        );
        Ok(())
    }

    async fn save_task_progress(&self, record: &WebTaskRecord) -> WebUiStoreResult<()> {
        let Some(progress) = &record.last_progress else {
            let started_at = Instant::now();
            let result = query::<Postgres>(
                r#"
                DELETE FROM web_task_progress
                WHERE user_id = $1 AND session_id = $2 AND task_id = $3
                "#,
            )
            .bind(record.user_id)
            .bind(&record.session_id)
            .bind(&record.task_id)
            .execute(self.pool())
            .await
            .map_err(db_error)?;
            log_store_query(
                "save_task_progress.delete_empty",
                started_at,
                Some(record.user_id),
                Some(&record.session_id),
                Some(&record.task_id),
                Some(result.rows_affected()),
                None,
                None,
            );
            return Ok(());
        };

        let progress_payload = json_value(progress, "task progress")?;
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO web_task_progress (
                user_id, session_id, task_id, current_iteration, max_iterations,
                is_finished, error, current_thought, progress_payload, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (user_id, session_id, task_id) DO UPDATE SET
                current_iteration = EXCLUDED.current_iteration,
                max_iterations = EXCLUDED.max_iterations,
                is_finished = EXCLUDED.is_finished,
                error = EXCLUDED.error,
                current_thought = EXCLUDED.current_thought,
                progress_payload = EXCLUDED.progress_payload,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.task_id)
        .bind(usize_to_i32(
            progress.current_iteration,
            "current_iteration",
        )?)
        .bind(usize_to_i32(progress.max_iterations, "max_iterations")?)
        .bind(progress.is_finished)
        .bind(&progress.error)
        .bind(&progress.current_thought)
        .bind(progress_payload)
        .bind(record.updated_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "save_task_progress.upsert",
            started_at,
            Some(record.user_id),
            Some(&record.session_id),
            Some(&record.task_id),
            Some(result.rows_affected()),
            None,
            None,
        );
        Ok(())
    }
}

#[async_trait]
impl WebUiStore for SqlxWebUiStore {
    async fn users_count(&self) -> WebUiStoreResult<u64> {
        let row = query::<Postgres>("SELECT COUNT(*) AS count FROM web_users")
            .fetch_one(self.pool())
            .await
            .map_err(db_error)?;
        i64_to_u64(row_value(&row, "count")?, "web user count")
    }

    async fn save_user(&self, record: WebUserRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.ensure_user_row(record.user_id, record.created_at)
            .await?;
        self.upsert_user_record(&record).await?;
        self.upsert_password_identity(&record).await
    }

    async fn load_user(&self, user_id: i64) -> WebUiStoreResult<Option<WebUserRecord>> {
        let started_at = Instant::now();
        let row = query::<Postgres>(
            r#"
            SELECT user_id, login, normalized_login, password_hash, role, status,
                   default_model_selection, default_agent_profile_id, default_effort,
                   last_login_at, schema_version, created_at, updated_at
            FROM web_users
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "load_user",
            started_at,
            Some(user_id),
            None,
            None,
            None,
            None,
            Some(row.is_some()),
        );

        row.as_ref().map(row_to_user).transpose()
    }

    async fn load_login_index(
        &self,
        normalized_login: &str,
    ) -> WebUiStoreResult<Option<LoginIndexRecord>> {
        let row = query::<Postgres>(
            r#"
            SELECT normalized_login, user_id
            FROM web_users
            WHERE normalized_login = $1
            "#,
        )
        .bind(normalized_login)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;

        row.as_ref().map(row_to_login_index).transpose()
    }

    async fn save_auth_session(&self, record: WebAuthSessionRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.ensure_user_row(record.user_id, record.created_at)
            .await?;
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO auth_sessions (
                session_token_hash, user_id, csrf_token, created_at,
                last_seen_at, expires_at, revoked_at, schema_version
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (session_token_hash) DO UPDATE SET
                user_id = EXCLUDED.user_id,
                csrf_token = EXCLUDED.csrf_token,
                created_at = EXCLUDED.created_at,
                last_seen_at = EXCLUDED.last_seen_at,
                expires_at = EXCLUDED.expires_at,
                revoked_at = EXCLUDED.revoked_at,
                schema_version = EXCLUDED.schema_version
            "#,
        )
        .bind(&record.session_token_hash)
        .bind(record.user_id)
        .bind(&record.csrf_token)
        .bind(record.created_at)
        .bind(record.last_seen_at)
        .bind(record.expires_at)
        .bind(record.revoked_at)
        .bind(u32_to_i32(
            record.schema_version,
            "auth session schema_version",
        )?)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "save_auth_session",
            started_at,
            Some(record.user_id),
            None,
            None,
            Some(result.rows_affected()),
            None,
            None,
        );
        Ok(())
    }

    async fn load_auth_session(
        &self,
        session_token_hash: &str,
    ) -> WebUiStoreResult<Option<WebAuthSessionRecord>> {
        let started_at = Instant::now();
        let row = query::<Postgres>(
            r#"
            SELECT session_token_hash, user_id, csrf_token, created_at,
                   last_seen_at, expires_at, revoked_at, schema_version
            FROM auth_sessions
            WHERE session_token_hash = $1
            "#,
        )
        .bind(session_token_hash)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;
        let user_id = row
            .as_ref()
            .and_then(|row| row.try_get::<i64, _>("user_id").ok());
        log_store_query(
            "load_auth_session",
            started_at,
            user_id,
            None,
            None,
            None,
            None,
            Some(row.is_some()),
        );

        row.as_ref().map(row_to_auth_session).transpose()
    }

    async fn revoke_auth_session(
        &self,
        session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<bool> {
        let result = query::<Postgres>(
            r#"
            UPDATE auth_sessions
            SET revoked_at = $2
            WHERE session_token_hash = $1
            "#,
        )
        .bind(session_token_hash)
        .bind(revoked_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    async fn revoke_auth_sessions_for_user_except(
        &self,
        user_id: i64,
        keep_session_token_hash: &str,
        revoked_at: DateTime<Utc>,
    ) -> WebUiStoreResult<u64> {
        let result = query::<Postgres>(
            r#"
            UPDATE auth_sessions
            SET revoked_at = $3
            WHERE user_id = $1
              AND session_token_hash <> $2
              AND revoked_at IS NULL
            "#,
        )
        .bind(user_id)
        .bind(keep_session_token_hash)
        .bind(revoked_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected())
    }

    async fn save_session(&self, record: WebSessionRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.ensure_user_row(record.user_id, record.created_at)
            .await?;
        let model_selection = optional_json(&record.model_selection, "model selection")?;
        let last_task_status = optional_enum_to_sql(&record.last_task_status, "task status")?;
        let auto_title_attempts = u32_to_i32(record.auto_title_attempts, "auto title attempts")?;
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO web_sessions (
                user_id, session_id, title, context_key, context_keys, agent_flow_id,
                model_selection, agent_profile_id, active_task_id, last_task_status,
                last_preview, manually_renamed, auto_title_source_message,
                auto_title_replaceable_title, auto_title_attempts,
                auto_title_next_attempt_at, auto_title_last_error,
                schema_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
            ON CONFLICT (user_id, session_id) DO UPDATE SET
                title = EXCLUDED.title,
                context_key = EXCLUDED.context_key,
                context_keys = EXCLUDED.context_keys,
                agent_flow_id = EXCLUDED.agent_flow_id,
                model_selection = EXCLUDED.model_selection,
                agent_profile_id = EXCLUDED.agent_profile_id,
                active_task_id = EXCLUDED.active_task_id,
                last_task_status = EXCLUDED.last_task_status,
                last_preview = EXCLUDED.last_preview,
                manually_renamed = EXCLUDED.manually_renamed,
                auto_title_source_message = EXCLUDED.auto_title_source_message,
                auto_title_replaceable_title = EXCLUDED.auto_title_replaceable_title,
                auto_title_attempts = EXCLUDED.auto_title_attempts,
                auto_title_next_attempt_at = EXCLUDED.auto_title_next_attempt_at,
                auto_title_last_error = EXCLUDED.auto_title_last_error,
                schema_version = EXCLUDED.schema_version,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.title)
        .bind(&record.context_key)
        .bind(&record.context_keys)
        .bind(&record.agent_flow_id)
        .bind(model_selection)
        .bind(&record.agent_profile_id)
        .bind(&record.active_task_id)
        .bind(last_task_status)
        .bind(&record.last_preview)
        .bind(record.manually_renamed)
        .bind(&record.auto_title_source_message)
        .bind(&record.auto_title_replaceable_title)
        .bind(auto_title_attempts)
        .bind(record.auto_title_next_attempt_at)
        .bind(&record.auto_title_last_error)
        .bind(u32_to_i32(
            record.schema_version,
            "web session schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "save_session",
            started_at,
            Some(record.user_id),
            Some(&record.session_id),
            record.active_task_id.as_deref(),
            Some(result.rows_affected()),
            None,
            None,
        );
        Ok(())
    }

    async fn load_session(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Option<WebSessionRecord>> {
        let started_at = Instant::now();
        let row = query::<Postgres>(
            r#"
            SELECT user_id, session_id, title, context_key, context_keys, agent_flow_id,
                   model_selection, agent_profile_id, active_task_id, last_task_status,
                   last_preview, manually_renamed, auto_title_source_message,
                   auto_title_replaceable_title, auto_title_attempts,
                   auto_title_next_attempt_at, auto_title_last_error,
                   schema_version, created_at, updated_at
            FROM web_sessions
            WHERE user_id = $1 AND session_id = $2
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "load_session",
            started_at,
            Some(user_id),
            Some(session_id),
            None,
            None,
            None,
            Some(row.is_some()),
        );

        row.as_ref().map(row_to_session).transpose()
    }

    async fn list_sessions(&self, user_id: i64) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, session_id, title, context_key, context_keys, agent_flow_id,
                   model_selection, agent_profile_id, active_task_id, last_task_status,
                   last_preview, manually_renamed, auto_title_source_message,
                   auto_title_replaceable_title, auto_title_attempts,
                   auto_title_next_attempt_at, auto_title_last_error,
                   schema_version, created_at, updated_at
            FROM web_sessions
            WHERE user_id = $1
            ORDER BY updated_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        rows.iter().map(row_to_session).collect()
    }

    async fn list_session_summaries(&self, user_id: i64) -> WebUiStoreResult<Vec<SessionSummary>> {
        let rows = query::<Postgres>(
            r#"
            SELECT session_id, title, model_selection, agent_profile_id, active_task_id,
                   last_task_status, last_preview, created_at, updated_at
            FROM web_sessions
            WHERE user_id = $1
            ORDER BY updated_at DESC
            "#,
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        rows.iter().map(row_to_session_summary).collect()
    }

    async fn list_session_context_keys(
        &self,
        user_id: i64,
    ) -> WebUiStoreResult<Vec<WebSessionContextKeys>> {
        let rows = query::<Postgres>(
            r#"
            SELECT context_key, context_keys
            FROM web_sessions
            WHERE user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        rows.iter().map(row_to_session_context_keys).collect()
    }

    async fn list_due_auto_title_sessions(
        &self,
        now: DateTime<Utc>,
        limit: usize,
    ) -> WebUiStoreResult<Vec<WebSessionRecord>> {
        let limit = usize_to_i64(limit, "auto title due session limit")?;
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, session_id, title, context_key, context_keys, agent_flow_id,
                   model_selection, agent_profile_id, active_task_id, last_task_status,
                   last_preview, manually_renamed, auto_title_source_message,
                   auto_title_replaceable_title, auto_title_attempts,
                   auto_title_next_attempt_at, auto_title_last_error,
                   schema_version, created_at, updated_at
            FROM web_sessions
            WHERE auto_title_source_message IS NOT NULL
              AND manually_renamed = FALSE
              AND (auto_title_next_attempt_at IS NULL OR auto_title_next_attempt_at <= $1)
            ORDER BY auto_title_next_attempt_at ASC NULLS FIRST, updated_at ASC
            LIMIT $2
            "#,
        )
        .bind(now)
        .bind(limit)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        rows.iter().map(row_to_session).collect()
    }

    async fn delete_session(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        let result = query::<Postgres>(
            r#"
            DELETE FROM web_sessions
            WHERE user_id = $1 AND session_id = $2
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        Ok(result.rows_affected() > 0)
    }

    async fn save_task(&self, record: WebTaskRecord) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        self.ensure_user_row(record.user_id, record.created_at)
            .await?;
        let status = enum_to_sql(&record.status, "task status")?;
        let attachments = json_value(&record.attachments, "task attachments")?;
        let pending_user_input = optional_json(&record.pending_user_input, "pending user input")?;
        let started_at = Instant::now();
        let result = query::<Postgres>(
            r#"
            INSERT INTO web_tasks (
                user_id, session_id, task_id, version_group_id, version_index,
                parent_task_id, status, input_markdown, attachments, input_edited_at,
                final_response_markdown, error_message, pending_user_input, last_event_seq,
                schema_version, created_at, started_at, updated_at, finished_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                    $11, $12, $13, $14, $15, $16, $17, $18, $19)
            ON CONFLICT (user_id, session_id, task_id) DO UPDATE SET
                version_group_id = EXCLUDED.version_group_id,
                version_index = EXCLUDED.version_index,
                parent_task_id = EXCLUDED.parent_task_id,
                status = EXCLUDED.status,
                input_markdown = EXCLUDED.input_markdown,
                attachments = EXCLUDED.attachments,
                input_edited_at = EXCLUDED.input_edited_at,
                final_response_markdown = EXCLUDED.final_response_markdown,
                error_message = EXCLUDED.error_message,
                pending_user_input = EXCLUDED.pending_user_input,
                last_event_seq = GREATEST(web_tasks.last_event_seq, EXCLUDED.last_event_seq),
                schema_version = EXCLUDED.schema_version,
                started_at = EXCLUDED.started_at,
                updated_at = EXCLUDED.updated_at,
                finished_at = EXCLUDED.finished_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.task_id)
        .bind(&record.version_group_id)
        .bind(u32_to_i32(record.version_index, "task version_index")?)
        .bind(&record.parent_task_id)
        .bind(status)
        .bind(&record.input_markdown)
        .bind(attachments)
        .bind(record.input_edited_at)
        .bind(&record.final_response_markdown)
        .bind(&record.error_message)
        .bind(pending_user_input)
        .bind(u64_to_i64(record.last_event_seq, "last_event_seq")?)
        .bind(u32_to_i32(
            record.schema_version,
            "web task schema_version",
        )?)
        .bind(record.created_at)
        .bind(record.started_at)
        .bind(record.updated_at)
        .bind(record.finished_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "save_task",
            started_at,
            Some(record.user_id),
            Some(&record.session_id),
            Some(&record.task_id),
            Some(result.rows_affected()),
            None,
            None,
        );

        self.save_task_progress(&record).await
    }

    async fn load_task(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskRecord>> {
        let sql = task_select_sql(
            "WHERE t.user_id = $1 AND t.session_id = $2 AND t.task_id = $3",
            "",
        );
        let row = query::<Postgres>(&sql)
            .bind(user_id)
            .bind(session_id)
            .bind(task_id)
            .fetch_optional(self.pool())
            .await
            .map_err(db_error)?;

        row.as_ref().map(row_to_task).transpose()
    }

    async fn task_exists(&self, user_id: i64, session_id: &str) -> WebUiStoreResult<bool> {
        let started_at = Instant::now();
        let row = query::<Postgres>(
            r#"
            SELECT 1
            FROM web_tasks
            WHERE user_id = $1 AND session_id = $2
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "task_exists",
            started_at,
            Some(user_id),
            Some(session_id),
            None,
            None,
            None,
            Some(row.is_some()),
        );

        Ok(row.is_some())
    }

    async fn load_task_event_state(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskEventState>> {
        let started_at = Instant::now();
        let row = query::<Postgres>(
            r#"
            SELECT status, last_event_seq
            FROM web_tasks
            WHERE user_id = $1 AND session_id = $2 AND task_id = $3
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .bind(task_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;
        log_store_query(
            "load_task_event_state",
            started_at,
            Some(user_id),
            Some(session_id),
            Some(task_id),
            None,
            None,
            Some(row.is_some()),
        );

        row.as_ref().map(row_to_task_event_state).transpose()
    }

    async fn list_tasks(
        &self,
        user_id: i64,
        session_id: &str,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let sql = task_list_select_sql(
            "WHERE t.user_id = $1 AND t.session_id = $2",
            "ORDER BY t.created_at ASC",
        );
        let rows = query::<Postgres>(&sql)
            .bind(user_id)
            .bind(session_id)
            .fetch_all(self.pool())
            .await
            .map_err(db_error)?;

        rows.iter().map(row_to_task).collect()
    }

    async fn list_recent_tasks_page(
        &self,
        user_id: i64,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let sql = task_list_select_sql(
            "WHERE t.user_id = $1 AND t.session_id = $2",
            "ORDER BY t.created_at DESC, t.task_id DESC LIMIT $3 OFFSET $4",
        );
        let rows = query::<Postgres>(&sql)
            .bind(user_id)
            .bind(session_id)
            .bind(usize_to_i64(limit, "task list page limit")?)
            .bind(usize_to_i64(offset, "task list page offset")?)
            .fetch_all(self.pool())
            .await
            .map_err(db_error)?;

        let mut tasks = rows
            .iter()
            .map(row_to_task)
            .collect::<WebUiStoreResult<Vec<_>>>()?;
        tasks.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        Ok(tasks)
    }

    async fn append_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        mut events: Vec<PersistedTaskEvent>,
    ) -> WebUiStoreResult<()> {
        events.sort_by_key(|event| event.seq);
        for event in events {
            event.validate_web_record()?;
            let kind = enum_to_sql(&event.kind, "task event kind")?;
            query::<Postgres>(
                r#"
                INSERT INTO web_task_events (
                    user_id, session_id, task_id, seq, kind, summary, payload,
                    redacted, truncated, schema_version, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (user_id, session_id, task_id, seq) DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(session_id)
            .bind(task_id)
            .bind(u64_to_i64(event.seq, "task event seq")?)
            .bind(kind)
            .bind(&event.summary)
            .bind(event.payload)
            .bind(event.redacted)
            .bind(event.truncated)
            .bind(u32_to_i32(
                event.schema_version,
                "task event schema_version",
            )?)
            .bind(event.created_at)
            .execute(self.pool())
            .await
            .map_err(db_error)?;
        }
        Ok(())
    }

    async fn list_task_events(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        after_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse> {
        let fetch_limit = limit.saturating_add(1);
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, session_id, task_id, seq, kind, summary, payload,
                   redacted, truncated, schema_version, created_at
            FROM web_task_events
            WHERE user_id = $1 AND session_id = $2 AND task_id = $3 AND seq > $4
            ORDER BY seq ASC
            LIMIT $5
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .bind(task_id)
        .bind(u64_to_i64(after_seq, "after_seq")?)
        .bind(usize_to_i64(fetch_limit, "task event page limit")?)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        let has_more = rows.len() > limit;
        let events = rows
            .iter()
            .take(limit)
            .map(row_to_event)
            .collect::<WebUiStoreResult<Vec<_>>>()?;
        let first_seq = events.first().map_or(after_seq, |event| event.seq);
        let last_seq = events.last().map_or(after_seq, |event| event.seq);
        Ok(TaskEventsResponse {
            events,
            first_seq,
            last_seq,
            has_more,
        })
    }

    async fn list_task_events_before(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        before_seq: u64,
        limit: usize,
    ) -> WebUiStoreResult<TaskEventsResponse> {
        let fetch_limit = limit.saturating_add(1);
        let rows = query::<Postgres>(
            r#"
            SELECT user_id, session_id, task_id, seq, kind, summary, payload,
                   redacted, truncated, schema_version, created_at
            FROM web_task_events
            WHERE user_id = $1 AND session_id = $2 AND task_id = $3 AND seq < $4
            ORDER BY seq DESC
            LIMIT $5
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .bind(task_id)
        .bind(u64_to_i64(before_seq, "before_seq")?)
        .bind(usize_to_i64(fetch_limit, "task event page limit")?)
        .fetch_all(self.pool())
        .await
        .map_err(db_error)?;

        let has_more = rows.len() > limit;
        let mut events = rows
            .iter()
            .take(limit)
            .map(row_to_event)
            .collect::<WebUiStoreResult<Vec<_>>>()?;
        events.sort_by_key(|event| event.seq);
        let first_seq = events.first().map_or(before_seq, |event| event.seq);
        let last_seq = events.last().map_or(0, |event| event.seq);
        Ok(TaskEventsResponse {
            events,
            first_seq,
            last_seq,
            has_more,
        })
    }

    async fn save_task_file(
        &self,
        record: WebTaskFileRecord,
        content: Vec<u8>,
    ) -> WebUiStoreResult<()> {
        record.validate_web_record()?;
        if record.size_bytes != content.len() as u64 {
            return Err(WebUiStoreError::Unavailable(format!(
                "task file size mismatch for {}: metadata={}, content={}",
                record.file_id,
                record.size_bytes,
                content.len()
            )));
        }
        if record.size_bytes > self.max_task_file_bytes {
            return Err(WebUiStoreError::Unavailable(format!(
                "task file {} exceeds Postgres storage limit: {} > {} bytes",
                record.file_id, record.size_bytes, self.max_task_file_bytes
            )));
        }

        let delivery_kind = enum_to_sql(&record.delivery_kind, "file delivery kind")?;
        query::<Postgres>(
            r#"
            INSERT INTO web_task_files (
                user_id, session_id, task_id, file_id, file_name, content_type,
                size_bytes, delivery_kind, schema_version, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (user_id, session_id, task_id, file_id) DO UPDATE SET
                file_name = EXCLUDED.file_name,
                content_type = EXCLUDED.content_type,
                size_bytes = EXCLUDED.size_bytes,
                delivery_kind = EXCLUDED.delivery_kind,
                schema_version = EXCLUDED.schema_version,
                created_at = EXCLUDED.created_at
            "#,
        )
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.task_id)
        .bind(&record.file_id)
        .bind(&record.file_name)
        .bind(&record.content_type)
        .bind(u64_to_i64(record.size_bytes, "task file size_bytes")?)
        .bind(delivery_kind)
        .bind(u32_to_i32(
            record.schema_version,
            "task file schema_version",
        )?)
        .bind(record.created_at)
        .execute(self.pool())
        .await
        .map_err(db_error)?;

        query::<Postgres>(
            r#"
            INSERT INTO web_task_file_blobs (user_id, session_id, task_id, file_id, content)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id, session_id, task_id, file_id) DO UPDATE SET
                content = EXCLUDED.content,
                created_at = NOW()
            "#,
        )
        .bind(record.user_id)
        .bind(&record.session_id)
        .bind(&record.task_id)
        .bind(&record.file_id)
        .bind(content)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn load_task_file(
        &self,
        user_id: i64,
        session_id: &str,
        task_id: &str,
        file_id: &str,
    ) -> WebUiStoreResult<Option<WebTaskFileBlob>> {
        let row = query::<Postgres>(
            r#"
            SELECT f.user_id, f.session_id, f.task_id, f.file_id, f.file_name,
                   f.content_type, f.size_bytes, f.delivery_kind, f.schema_version,
                   f.created_at, b.content
            FROM web_task_files f
            LEFT JOIN web_task_file_blobs b
              ON b.user_id = f.user_id
             AND b.session_id = f.session_id
             AND b.task_id = f.task_id
             AND b.file_id = f.file_id
            WHERE f.user_id = $1 AND f.session_id = $2 AND f.task_id = $3 AND f.file_id = $4
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .bind(task_id)
        .bind(file_id)
        .fetch_optional(self.pool())
        .await
        .map_err(db_error)?;

        row.as_ref().map(row_to_task_file).transpose()
    }

    async fn mark_unfinished_tasks_interrupted(
        &self,
        message: &str,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<Vec<WebTaskRecord>> {
        let select_sql = task_select_sql(
            r#"
            JOIN updated_tasks u
              ON u.user_id = t.user_id
             AND u.session_id = t.session_id
             AND u.task_id = t.task_id
            "#,
            "ORDER BY t.user_id ASC, t.session_id ASC, t.task_id ASC",
        );
        let sql = format!(
            r#"
            WITH updated_tasks AS (
                UPDATE web_tasks
                SET status = 'interrupted', error_message = $1, updated_at = $2, finished_at = $2
                WHERE status IN ('queued', 'running')
                RETURNING user_id, session_id, task_id
            )
            {select_sql}
            "#,
        );
        let rows = query::<Postgres>(&sql)
            .bind(message)
            .bind(now)
            .fetch_all(self.pool())
            .await
            .map_err(db_error)?;

        let interrupted = rows
            .iter()
            .map(row_to_task)
            .collect::<WebUiStoreResult<Vec<_>>>()?;
        for task in &interrupted {
            self.clear_interrupted_session_task(task, now).await?;
        }
        Ok(interrupted)
    }
}

impl SqlxWebUiStore {
    async fn clear_interrupted_session_task(
        &self,
        task: &WebTaskRecord,
        now: DateTime<Utc>,
    ) -> WebUiStoreResult<()> {
        query::<Postgres>(
            r#"
            UPDATE web_sessions
            SET active_task_id = NULL,
                last_task_status = 'interrupted',
                updated_at = $4
            WHERE user_id = $1
              AND session_id = $2
              AND active_task_id = $3
            "#,
        )
        .bind(task.user_id)
        .bind(&task.session_id)
        .bind(&task.task_id)
        .bind(now)
        .execute(self.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }
}

fn task_select_sql(join_or_where: &str, order_by: &str) -> String {
    format!(
        r#"
            SELECT t.user_id, t.session_id, t.task_id, t.version_group_id, t.version_index,
                   t.parent_task_id, t.status, t.input_markdown, t.attachments,
                   t.input_edited_at, t.final_response_markdown, t.error_message,
                   t.pending_user_input, t.last_event_seq, t.schema_version, t.created_at,
                   t.started_at, t.updated_at, t.finished_at,
                   p.progress_payload AS last_progress_payload
            FROM web_tasks t
            LEFT JOIN web_task_progress p
              ON p.user_id = t.user_id
             AND p.session_id = t.session_id
             AND p.task_id = t.task_id
            {join_or_where}
            {order_by}
            "#
    )
}

fn task_list_select_sql(join_or_where: &str, order_by: &str) -> String {
    format!(
        r#"
            SELECT t.user_id, t.session_id, t.task_id, t.version_group_id, t.version_index,
                   t.parent_task_id, t.status, t.input_markdown, t.attachments,
                   t.input_edited_at, t.final_response_markdown, t.error_message,
                   t.pending_user_input, t.last_event_seq, t.schema_version, t.created_at,
                   t.started_at, t.updated_at, t.finished_at,
                   NULL::jsonb AS last_progress_payload
            FROM web_tasks t
            {join_or_where}
            {order_by}
            "#
    )
}

fn row_to_user(row: &PgRow) -> WebUiStoreResult<WebUserRecord> {
    let role = enum_from_sql(row_value::<String>(row, "role")?.as_str(), "web user role")?;
    let status = enum_from_sql(
        row_value::<String>(row, "status")?.as_str(),
        "web user status",
    )?;
    let default_model_selection = optional_from_json(
        row_value::<Option<Value>>(row, "default_model_selection")?,
        "model selection",
    )?;
    let default_effort = optional_enum_from_sql(
        row_value::<Option<String>>(row, "default_effort")?,
        "agent effort",
    )?;
    Ok(WebUserRecord {
        schema_version: i32_to_u32(row_value(row, "schema_version")?, "web user schema_version")?,
        user_id: row_value(row, "user_id")?,
        login: row_value(row, "login")?,
        normalized_login: row_value(row, "normalized_login")?,
        password_hash: row_value(row, "password_hash")?,
        role,
        status,
        default_model_selection,
        default_agent_profile_id: row_value(row, "default_agent_profile_id")?,
        default_effort,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
        last_login_at: row_value(row, "last_login_at")?,
    })
}

fn row_to_login_index(row: &PgRow) -> WebUiStoreResult<LoginIndexRecord> {
    Ok(LoginIndexRecord {
        schema_version: WEB_AUTH_SCHEMA_VERSION,
        normalized_login: row_value(row, "normalized_login")?,
        user_id: row_value(row, "user_id")?,
    })
}

fn row_to_auth_session(row: &PgRow) -> WebUiStoreResult<WebAuthSessionRecord> {
    Ok(WebAuthSessionRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "auth session schema_version",
        )?,
        session_token_hash: row_value(row, "session_token_hash")?,
        user_id: row_value(row, "user_id")?,
        csrf_token: row_value(row, "csrf_token")?,
        created_at: row_value(row, "created_at")?,
        last_seen_at: row_value(row, "last_seen_at")?,
        expires_at: row_value(row, "expires_at")?,
        revoked_at: row_value(row, "revoked_at")?,
    })
}

fn row_to_session(row: &PgRow) -> WebUiStoreResult<WebSessionRecord> {
    let model_selection = optional_from_json(
        row_value::<Option<Value>>(row, "model_selection")?,
        "model selection",
    )?;
    let last_task_status = optional_enum_from_sql(
        row_value::<Option<String>>(row, "last_task_status")?,
        "task status",
    )?;
    Ok(WebSessionRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "web session schema_version",
        )?,
        session_id: row_value(row, "session_id")?,
        user_id: row_value(row, "user_id")?,
        title: row_value(row, "title")?,
        context_key: row_value(row, "context_key")?,
        context_keys: row_value(row, "context_keys")?,
        agent_flow_id: row_value(row, "agent_flow_id")?,
        model_selection,
        agent_profile_id: row_value(row, "agent_profile_id")?,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
        active_task_id: row_value(row, "active_task_id")?,
        last_task_status,
        last_preview: row_value(row, "last_preview")?,
        manually_renamed: row_value(row, "manually_renamed")?,
        auto_title_source_message: row_value(row, "auto_title_source_message")?,
        auto_title_replaceable_title: row_value(row, "auto_title_replaceable_title")?,
        auto_title_attempts: i32_to_u32(
            row_value(row, "auto_title_attempts")?,
            "auto title attempts",
        )?,
        auto_title_next_attempt_at: row_value(row, "auto_title_next_attempt_at")?,
        auto_title_last_error: row_value(row, "auto_title_last_error")?,
    })
}

fn row_to_session_summary(row: &PgRow) -> WebUiStoreResult<SessionSummary> {
    let model_selection = optional_from_json(
        row_value::<Option<Value>>(row, "model_selection")?,
        "model selection",
    )?;
    let last_task_status = optional_enum_from_sql(
        row_value::<Option<String>>(row, "last_task_status")?,
        "task status",
    )?;
    Ok(SessionSummary {
        session_id: row_value(row, "session_id")?,
        title: row_value(row, "title")?,
        model_selection,
        agent_profile_id: row_value(row, "agent_profile_id")?,
        last_preview: row_value(row, "last_preview")?,
        active_task_id: row_value(row, "active_task_id")?,
        last_task_status,
        created_at: row_value(row, "created_at")?,
        updated_at: row_value(row, "updated_at")?,
    })
}

fn row_to_session_context_keys(row: &PgRow) -> WebUiStoreResult<WebSessionContextKeys> {
    Ok(WebSessionContextKeys {
        context_key: row_value(row, "context_key")?,
        context_keys: row_value(row, "context_keys")?,
    })
}

fn row_to_task(row: &PgRow) -> WebUiStoreResult<WebTaskRecord> {
    let status = enum_from_sql(row_value::<String>(row, "status")?.as_str(), "task status")?;
    let attachments = from_json(row_value(row, "attachments")?, "task attachments")?;
    let pending_user_input = optional_from_json(
        row_value::<Option<Value>>(row, "pending_user_input")?,
        "pending user input",
    )?;
    let last_progress = optional_from_json(
        row_value::<Option<Value>>(row, "last_progress_payload")?,
        "task progress",
    )?;
    Ok(WebTaskRecord {
        schema_version: i32_to_u32(row_value(row, "schema_version")?, "web task schema_version")?,
        task_id: row_value(row, "task_id")?,
        session_id: row_value(row, "session_id")?,
        user_id: row_value(row, "user_id")?,
        version_group_id: row_value(row, "version_group_id")?,
        version_index: i32_to_u32(row_value(row, "version_index")?, "task version_index")?,
        parent_task_id: row_value(row, "parent_task_id")?,
        status,
        input_markdown: row_value(row, "input_markdown")?,
        attachments,
        input_edited_at: row_value(row, "input_edited_at")?,
        final_response_markdown: row_value(row, "final_response_markdown")?,
        error_message: row_value(row, "error_message")?,
        pending_user_input,
        last_progress,
        last_event_seq: i64_to_u64(row_value(row, "last_event_seq")?, "last_event_seq")?,
        created_at: row_value(row, "created_at")?,
        started_at: row_value(row, "started_at")?,
        updated_at: row_value(row, "updated_at")?,
        finished_at: row_value(row, "finished_at")?,
    })
}

fn row_to_task_event_state(row: &PgRow) -> WebUiStoreResult<WebTaskEventState> {
    let status = enum_from_sql(row_value::<String>(row, "status")?.as_str(), "task status")?;
    Ok(WebTaskEventState {
        status,
        last_event_seq: i64_to_u64(row_value(row, "last_event_seq")?, "last_event_seq")?,
    })
}

fn row_to_event(row: &PgRow) -> WebUiStoreResult<PersistedTaskEvent> {
    let kind = enum_from_sql(
        row_value::<String>(row, "kind")?.as_str(),
        "task event kind",
    )?;
    let event = PersistedTaskEvent {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "task event schema_version",
        )?,
        task_id: row_value(row, "task_id")?,
        session_id: row_value(row, "session_id")?,
        user_id: row_value(row, "user_id")?,
        seq: i64_to_u64(row_value(row, "seq")?, "task event seq")?,
        created_at: row_value(row, "created_at")?,
        kind,
        summary: row_value(row, "summary")?,
        payload: row_value(row, "payload")?,
        redacted: row_value(row, "redacted")?,
        truncated: row_value(row, "truncated")?,
    };
    event.validate_web_record()?;
    Ok(event)
}

fn row_to_task_file(row: &PgRow) -> WebUiStoreResult<WebTaskFileBlob> {
    let content: Option<Vec<u8>> = row_value(row, "content")?;
    let Some(content) = content else {
        let file_id: String = row_value(row, "file_id")?;
        return Err(WebUiStoreError::Unavailable(format!(
            "task file blob missing for {file_id}"
        )));
    };
    let delivery_kind = enum_from_sql(
        row_value::<String>(row, "delivery_kind")?.as_str(),
        "file delivery kind",
    )?;
    let record = WebTaskFileRecord {
        schema_version: i32_to_u32(
            row_value(row, "schema_version")?,
            "task file schema_version",
        )?,
        user_id: row_value(row, "user_id")?,
        session_id: row_value(row, "session_id")?,
        task_id: row_value(row, "task_id")?,
        file_id: row_value(row, "file_id")?,
        file_name: row_value(row, "file_name")?,
        content_type: row_value(row, "content_type")?,
        size_bytes: i64_to_u64(row_value(row, "size_bytes")?, "task file size_bytes")?,
        delivery_kind,
        created_at: row_value(row, "created_at")?,
    };
    record.validate_web_record()?;
    Ok(WebTaskFileBlob { record, content })
}

fn row_value<T>(row: &PgRow, column: &str) -> WebUiStoreResult<T>
where
    for<'r> T: sqlx_core::decode::Decode<'r, Postgres> + sqlx_core::types::Type<Postgres>,
{
    row.try_get(column).map_err(db_error)
}

fn json_value<T: Serialize>(value: &T, name: &str) -> WebUiStoreResult<Value> {
    serde_json::to_value(value).map_err(|error| json_error(name, error))
}

fn optional_json<T: Serialize>(value: &Option<T>, name: &str) -> WebUiStoreResult<Option<Value>> {
    value
        .as_ref()
        .map(|value| json_value(value, name))
        .transpose()
}

fn from_json<T: DeserializeOwned>(value: Value, name: &str) -> WebUiStoreResult<T> {
    serde_json::from_value(value).map_err(|error| json_error(name, error))
}

fn optional_from_json<T: DeserializeOwned>(
    value: Option<Value>,
    name: &str,
) -> WebUiStoreResult<Option<T>> {
    value.map(|value| from_json(value, name)).transpose()
}

fn enum_to_sql<T: Serialize>(value: &T, name: &str) -> WebUiStoreResult<String> {
    match json_value(value, name)? {
        Value::String(value) => Ok(value),
        other => Err(WebUiStoreError::Unavailable(format!(
            "{name} serialized to non-string JSON value: {other}"
        ))),
    }
}

fn optional_enum_to_sql<T: Serialize>(
    value: &Option<T>,
    name: &str,
) -> WebUiStoreResult<Option<String>> {
    value
        .as_ref()
        .map(|value| enum_to_sql(value, name))
        .transpose()
}

fn enum_from_sql<T: DeserializeOwned>(value: &str, name: &str) -> WebUiStoreResult<T> {
    from_json(Value::String(value.to_string()), name)
}

fn optional_enum_from_sql<T: DeserializeOwned>(
    value: Option<String>,
    name: &str,
) -> WebUiStoreResult<Option<T>> {
    value
        .as_deref()
        .map(|value| enum_from_sql(value, name))
        .transpose()
}

fn u32_to_i32(value: u32, name: &str) -> WebUiStoreResult<i32> {
    i32::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} exceeds i32 range: {value}")))
}

fn i32_to_u32(value: i32, name: &str) -> WebUiStoreResult<u32> {
    u32::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} is negative: {value}")))
}

fn u64_to_i64(value: u64, name: &str) -> WebUiStoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} exceeds i64 range: {value}")))
}

fn i64_to_u64(value: i64, name: &str) -> WebUiStoreResult<u64> {
    u64::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} is negative: {value}")))
}

fn usize_to_i32(value: usize, name: &str) -> WebUiStoreResult<i32> {
    i32::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} exceeds i32 range: {value}")))
}

fn usize_to_i64(value: usize, name: &str) -> WebUiStoreResult<i64> {
    i64::try_from(value)
        .map_err(|_| WebUiStoreError::Unavailable(format!("{name} exceeds i64 range: {value}")))
}

fn db_error(error: sqlx_core::error::Error) -> WebUiStoreError {
    WebUiStoreError::Unavailable(error.to_string())
}

fn login_conflict_error(error: sqlx_core::error::Error, normalized_login: &str) -> WebUiStoreError {
    if let sqlx_core::error::Error::Database(database_error) = &error {
        if database_error.is_unique_violation() {
            return WebUiStoreError::Conflict(format!(
                "login {normalized_login} already belongs to another user"
            ));
        }
    }
    db_error(error)
}

fn json_error(name: &str, error: serde_json::Error) -> WebUiStoreError {
    WebUiStoreError::Unavailable(format!("failed to map {name} JSON: {error}"))
}

fn max_task_file_bytes_from_env() -> u64 {
    std::env::var(TASK_FILE_MAX_BYTES_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TASK_FILE_MAX_BYTES)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::time::Duration as StdDuration;

    use chrono::Duration;
    use oxide_agent_core::agent::progress::FileDeliveryKind;
    use oxide_agent_core::storage::{SqlxStorage, SqlxStorageConfig};
    use oxide_agent_web_contracts::{
        AgentEffort, ProgressSnapshot, TaskEventKind, TaskStatus, UserRole,
    };

    use crate::persistence::WebUserStatus;

    use super::*;

    static SQL_USER_ID: AtomicI64 = AtomicI64::new(0);

    async fn test_store() -> Option<SqlxWebUiStore> {
        let database_url = std::env::var("OXIDE_DATABASE_TEST_URL").ok()?;
        let migrations_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("migrations");
        let storage = SqlxStorage::connect(SqlxStorageConfig {
            database_url,
            max_connections: 5,
            connect_timeout: StdDuration::from_secs(10),
            migrate_on_startup: true,
            migrations_dir,
        })
        .await
        .ok()?;
        Some(SqlxWebUiStore::with_max_task_file_bytes(
            Arc::new(storage),
            1024 * 1024,
        ))
    }

    fn next_user_id() -> i64 {
        let sequence = SQL_USER_ID.fetch_add(10, Ordering::SeqCst);
        let timestamp_component = Utc::now().timestamp_micros().rem_euclid(90_000_000_000) * 10;
        9_000_000_000_000 + timestamp_component + sequence
    }

    fn postgres_timestamp_now() -> DateTime<Utc> {
        let now = Utc::now();
        let micros = now.timestamp_subsec_micros();
        DateTime::<Utc>::from_timestamp(now.timestamp(), micros * 1_000)
            .expect("current timestamp should be representable")
    }

    fn user_record(user_id: i64, login: &str, now: DateTime<Utc>) -> WebUserRecord {
        WebUserRecord {
            schema_version: 1,
            user_id,
            login: login.to_string(),
            normalized_login: login.to_ascii_lowercase(),
            password_hash: "argon2id$hash".to_string(),
            role: UserRole::User,
            status: WebUserStatus::Active,
            default_model_selection: None,
            default_agent_profile_id: None,
            default_effort: Some(AgentEffort::Standard),
            created_at: now,
            updated_at: now,
            last_login_at: None,
        }
    }

    fn auth_session(
        user_id: i64,
        session_token_hash: &str,
        now: DateTime<Utc>,
    ) -> WebAuthSessionRecord {
        WebAuthSessionRecord {
            schema_version: 1,
            session_token_hash: session_token_hash.to_string(),
            user_id,
            csrf_token: "csrf".to_string(),
            created_at: now,
            last_seen_at: now,
            expires_at: now + Duration::days(1),
            revoked_at: None,
        }
    }

    fn session_record(user_id: i64, session_id: &str, now: DateTime<Utc>) -> WebSessionRecord {
        WebSessionRecord {
            schema_version: 1,
            session_id: session_id.to_string(),
            user_id,
            title: format!("Session {session_id}"),
            context_key: format!("web-session-{session_id}"),
            context_keys: vec![format!("web-session-{session_id}")],
            agent_flow_id: "main".to_string(),
            model_selection: None,
            agent_profile_id: None,
            created_at: now,
            updated_at: now,
            active_task_id: None,
            last_task_status: None,
            last_preview: None,
            manually_renamed: false,
            auto_title_source_message: None,
            auto_title_replaceable_title: None,
            auto_title_attempts: 0,
            auto_title_next_attempt_at: None,
            auto_title_last_error: None,
        }
    }

    fn task_record(
        user_id: i64,
        session_id: &str,
        task_id: &str,
        status: TaskStatus,
        now: DateTime<Utc>,
    ) -> WebTaskRecord {
        WebTaskRecord {
            schema_version: 1,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            version_group_id: task_id.to_string(),
            version_index: 1,
            parent_task_id: None,
            status,
            input_markdown: "Prompt".to_string(),
            attachments: Vec::new(),
            input_edited_at: None,
            final_response_markdown: status.is_terminal().then(|| "Done".to_string()),
            error_message: None,
            pending_user_input: None,
            last_progress: Some(ProgressSnapshot {
                current_iteration: 1,
                max_iterations: 3,
                is_finished: status.is_terminal(),
                error: None,
                current_thought: Some("working".to_string()),
                current_todos: Some(serde_json::json!([{ "content": "test", "status": "done" }])),
                last_compaction_status: None,
                repeated_compaction_warning: None,
                last_history_repair_status: None,
                latest_token_snapshot: None,
                llm_retry: None,
                provider_failover_notice: None,
            }),
            last_event_seq: 0,
            created_at: now,
            started_at: Some(now),
            updated_at: now,
            finished_at: status.is_terminal().then_some(now),
        }
    }

    fn event(user_id: i64, session_id: &str, task_id: &str, seq: u64) -> PersistedTaskEvent {
        PersistedTaskEvent {
            schema_version: 1,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            user_id,
            seq,
            created_at: Utc::now(),
            kind: TaskEventKind::ToolResult,
            summary: format!("event-{seq}"),
            payload: serde_json::json!({ "seq": seq }),
            redacted: false,
            truncated: false,
        }
    }

    async fn save_user_and_session(
        store: &SqlxWebUiStore,
        user_id: i64,
        session_id: &str,
        now: DateTime<Utc>,
    ) {
        store
            .save_user(user_record(user_id, &format!("user-{user_id}"), now))
            .await
            .expect("save SQL user");
        store
            .save_session(session_record(user_id, session_id, now))
            .await
            .expect("save SQL session");
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_round_trips_records_events_progress_and_files() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let now = postgres_timestamp_now();
        let user_id = next_user_id();
        let users_before = store.users_count().await.expect("count users before");

        let user = user_record(user_id, &format!("alice-{user_id}"), now);
        store.save_user(user.clone()).await.expect("save user");
        assert_eq!(
            store.load_user(user_id).await.expect("load user"),
            Some(user)
        );
        assert!(
            store.users_count().await.expect("count users") >= users_before + 1,
            "users_count should include the saved user"
        );
        assert!(store
            .save_user(user_record(user_id + 1, &format!("alice-{user_id}"), now))
            .await
            .is_err());

        store
            .save_auth_session(auth_session(user_id, "keep", now))
            .await
            .expect("save kept auth session");
        store
            .save_auth_session(auth_session(user_id, "revoke", now))
            .await
            .expect("save revoked auth session");
        assert_eq!(
            store
                .revoke_auth_sessions_for_user_except(user_id, "keep", now + Duration::seconds(1))
                .await
                .expect("revoke other sessions"),
            1
        );

        store
            .save_session(session_record(user_id, "session-1", now))
            .await
            .expect("save session");
        let task = task_record(user_id, "session-1", "task-1", TaskStatus::Completed, now);
        store.save_task(task).await.expect("save task");
        assert!(store
            .load_task(user_id, "session-1", "task-1")
            .await
            .expect("load task")
            .and_then(|task| task.last_progress)
            .is_some());
        assert!(store
            .task_exists(user_id, "session-1")
            .await
            .expect("task exists"));
        let task_state = store
            .load_task_event_state(user_id, "session-1", "task-1")
            .await
            .expect("load task event state")
            .expect("task event state exists");
        assert_eq!(task_state.status, TaskStatus::Completed);
        assert_eq!(task_state.last_event_seq, 0);

        let events = (1..=250)
            .map(|seq| event(user_id, "session-1", "task-1", seq))
            .collect::<Vec<_>>();
        store
            .append_task_events(user_id, "session-1", "task-1", events)
            .await
            .expect("append events");
        let page = store
            .list_task_events(user_id, "session-1", "task-1", 0, 200)
            .await
            .expect("list first event page");
        assert_eq!(page.events.len(), 200);
        assert_eq!(page.first_seq, 1);
        assert_eq!(page.last_seq, 200);
        assert!(page.has_more);

        let tail = store
            .list_task_events_before(user_id, "session-1", "task-1", 251, 25)
            .await
            .expect("list tail event page");
        assert_eq!(tail.events.len(), 25);
        assert_eq!(tail.first_seq, 226);
        assert_eq!(tail.last_seq, 250);
        assert!(tail.has_more);

        store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: 1,
                    user_id,
                    session_id: "session-1".to_string(),
                    task_id: "task-1".to_string(),
                    file_id: "file-1".to_string(),
                    file_name: "report.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    size_bytes: 5,
                    delivery_kind: FileDeliveryKind::Document,
                    created_at: now,
                },
                b"hello".to_vec(),
            )
            .await
            .expect("save file");
        let file = store
            .load_task_file(user_id, "session-1", "task-1", "file-1")
            .await
            .expect("load file")
            .expect("file exists");
        assert_eq!(file.content, b"hello");

        assert!(store
            .delete_session(user_id, "session-1")
            .await
            .expect("delete session"));
        assert!(store
            .list_task_events(user_id, "session-1", "task-1", 0, 10)
            .await
            .expect("list deleted events")
            .events
            .is_empty());
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_reconciles_unfinished_tasks_with_sql_updates() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let now = postgres_timestamp_now();
        let reconcile_at = now + Duration::seconds(5);
        let user_id = next_user_id();
        save_user_and_session(&store, user_id, "session-1", now).await;
        let mut session = store
            .load_session(user_id, "session-1")
            .await
            .expect("load session")
            .expect("session exists");
        session.active_task_id = Some("running".to_string());
        store
            .save_session(session)
            .await
            .expect("save active session");
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "queued",
                TaskStatus::Queued,
                now,
            ))
            .await
            .expect("save queued task");
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "running",
                TaskStatus::Running,
                now,
            ))
            .await
            .expect("save running task");
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "done",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save completed task");

        let interrupted = store
            .mark_unfinished_tasks_interrupted("backend restarted", reconcile_at)
            .await
            .expect("reconcile tasks");
        let mut interrupted_task_ids = interrupted
            .iter()
            .filter(|task| task.user_id == user_id)
            .map(|task| task.task_id.as_str())
            .collect::<Vec<_>>();
        interrupted_task_ids.sort_unstable();
        assert_eq!(interrupted_task_ids, ["queued", "running"]);
        assert_eq!(
            store
                .load_task(user_id, "session-1", "running")
                .await
                .expect("load running")
                .map(|task| task.status),
            Some(TaskStatus::Interrupted)
        );
        assert_eq!(
            store
                .load_session(user_id, "session-1")
                .await
                .expect("load reconciled session")
                .and_then(|session| session.active_task_id),
            None
        );
        assert_eq!(
            store
                .load_task(user_id, "session-1", "done")
                .await
                .expect("load completed")
                .map(|task| task.status),
            Some(TaskStatus::Completed)
        );
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_persists_records_across_store_recreation() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let now = postgres_timestamp_now();
        let user_id = next_user_id();
        save_user_and_session(&store, user_id, "session-1", now).await;
        store
            .save_auth_session(auth_session(user_id, "restart-token", now))
            .await
            .expect("save auth session");
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "task-1",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save task");
        store
            .append_task_events(
                user_id,
                "session-1",
                "task-1",
                vec![event(user_id, "session-1", "task-1", 1)],
            )
            .await
            .expect("append event");
        store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: 1,
                    user_id,
                    session_id: "session-1".to_string(),
                    task_id: "task-1".to_string(),
                    file_id: "file-1".to_string(),
                    file_name: "restart.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    size_bytes: 7,
                    delivery_kind: FileDeliveryKind::Document,
                    created_at: now,
                },
                b"restart".to_vec(),
            )
            .await
            .expect("save task file");

        let restarted_storage = SqlxStorage::connect(store.storage.config().clone())
            .await
            .expect("restarted SQLx storage should connect");
        let restarted =
            SqlxWebUiStore::with_max_task_file_bytes(Arc::new(restarted_storage), 1024 * 1024);

        assert!(restarted
            .load_auth_session("restart-token")
            .await
            .expect("auth session should load after restart")
            .is_some());
        assert!(restarted
            .load_session(user_id, "session-1")
            .await
            .expect("session should load after restart")
            .is_some());
        assert_eq!(
            restarted
                .list_task_events(user_id, "session-1", "task-1", 0, 10)
                .await
                .expect("events should load after restart")
                .events
                .len(),
            1
        );
        assert_eq!(
            restarted
                .load_task_file(user_id, "session-1", "task-1", "file-1")
                .await
                .expect("file should load after restart")
                .expect("file exists")
                .content,
            b"restart"
        );
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_rejects_oversized_task_files() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let store = SqlxWebUiStore::with_max_task_file_bytes(store.storage.clone(), 4);
        let now = postgres_timestamp_now();
        let user_id = next_user_id();
        save_user_and_session(&store, user_id, "session-1", now).await;
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "task-1",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save task");

        let error = store
            .save_task_file(
                WebTaskFileRecord {
                    schema_version: 1,
                    user_id,
                    session_id: "session-1".to_string(),
                    task_id: "task-1".to_string(),
                    file_id: "too-large".to_string(),
                    file_name: "large.bin".to_string(),
                    content_type: "application/octet-stream".to_string(),
                    size_bytes: 5,
                    delivery_kind: FileDeliveryKind::Document,
                    created_at: now,
                },
                b"hello".to_vec(),
            )
            .await
            .expect_err("oversized file should fail");
        assert!(error.to_string().contains("Postgres storage limit"));
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_pages_large_event_stream_and_ignores_duplicate_seq() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let now = postgres_timestamp_now();
        let user_id = next_user_id();
        save_user_and_session(&store, user_id, "session-1", now).await;
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "task-1",
                TaskStatus::Running,
                now,
            ))
            .await
            .expect("save task");

        let events = (1..=1_200)
            .map(|seq| event(user_id, "session-1", "task-1", seq))
            .collect::<Vec<_>>();
        store
            .append_task_events(user_id, "session-1", "task-1", events)
            .await
            .expect("append large event stream");
        let mut duplicate = event(user_id, "session-1", "task-1", 1_200);
        duplicate.summary = "duplicate-should-be-ignored".to_string();
        store
            .append_task_events(
                user_id,
                "session-1",
                "task-1",
                vec![duplicate, event(user_id, "session-1", "task-1", 1_201)],
            )
            .await
            .expect("append duplicate and next event");

        let tail = store
            .list_task_events(user_id, "session-1", "task-1", 1_198, 10)
            .await
            .expect("tail event page should load");
        assert_eq!(tail.events.len(), 3);
        assert_eq!(tail.events[0].seq, 1_199);
        assert_eq!(tail.events[1].seq, 1_200);
        assert_ne!(tail.events[1].summary, "duplicate-should-be-ignored");
        assert_eq!(tail.events[2].seq, 1_201);
        assert!(!tail.has_more);

        let row = query::<Postgres>(
            r#"
            SELECT COUNT(*) AS event_count
            FROM web_task_events
            WHERE user_id = $1 AND session_id = $2 AND task_id = $3
            "#,
        )
        .bind(user_id)
        .bind("session-1")
        .bind("task-1")
        .fetch_one(store.pool())
        .await
        .expect("event count should load");
        assert_eq!(row_value::<i64>(&row, "event_count").unwrap(), 1_201);
    }

    #[tokio::test]
    async fn sqlx_web_ui_store_retention_cleanup_is_bounded_and_idempotent() {
        let Some(store) = test_store().await else {
            eprintln!("skipping SQLx web store test: OXIDE_DATABASE_TEST_URL is not set");
            return;
        };
        let now = postgres_timestamp_now();
        let expired_at = now - Duration::seconds(5);
        let fresh_until = now + Duration::days(1);
        let user_id = next_user_id();
        save_user_and_session(&store, user_id, "session-1", now).await;
        store
            .save_task(task_record(
                user_id,
                "session-1",
                "task-1",
                TaskStatus::Completed,
                now,
            ))
            .await
            .expect("save task");

        let mut expired_session = auth_session(user_id, "expired", now);
        expired_session.expires_at = expired_at;
        store
            .save_auth_session(expired_session)
            .await
            .expect("save expired auth session");
        let mut revoked_session = auth_session(user_id, "revoked", now);
        revoked_session.expires_at = fresh_until;
        revoked_session.revoked_at = Some(expired_at);
        store
            .save_auth_session(revoked_session)
            .await
            .expect("save revoked auth session");
        store
            .save_auth_session(auth_session(user_id, "active", now))
            .await
            .expect("save active auth session");

        store
            .append_task_events(
                user_id,
                "session-1",
                "task-1",
                (1..=3)
                    .map(|seq| event(user_id, "session-1", "task-1", seq))
                    .collect(),
            )
            .await
            .expect("append events");
        query::<Postgres>(
            r#"
            UPDATE web_task_events
            SET retention_expires_at = CASE WHEN seq = 3 THEN $2 ELSE $1 END
            WHERE user_id = $3 AND session_id = $4 AND task_id = $5
            "#,
        )
        .bind(expired_at)
        .bind(fresh_until)
        .bind(user_id)
        .bind("session-1")
        .bind("task-1")
        .execute(store.pool())
        .await
        .expect("event retention timestamps should update");

        for file_id in ["expired-file", "fresh-file"] {
            store
                .save_task_file(
                    WebTaskFileRecord {
                        schema_version: 1,
                        user_id,
                        session_id: "session-1".to_string(),
                        task_id: "task-1".to_string(),
                        file_id: file_id.to_string(),
                        file_name: format!("{file_id}.txt"),
                        content_type: "text/plain".to_string(),
                        size_bytes: 5,
                        delivery_kind: FileDeliveryKind::Document,
                        created_at: now,
                    },
                    b"hello".to_vec(),
                )
                .await
                .expect("save task file");
        }
        query::<Postgres>(
            r#"
            UPDATE web_task_files
            SET retention_expires_at = CASE WHEN file_id = 'fresh-file' THEN $1 ELSE $2 END
            WHERE user_id = $3 AND session_id = $4 AND task_id = $5
            "#,
        )
        .bind(fresh_until)
        .bind(expired_at)
        .bind(user_id)
        .bind("session-1")
        .bind("task-1")
        .execute(store.pool())
        .await
        .expect("file retention timestamps should update");

        assert_eq!(
            store
                .cleanup_expired_records(now, 1)
                .await
                .expect("first bounded cleanup should execute"),
            ExpiredWebRecordsCleanup {
                auth_sessions: 1,
                task_events: 1,
                task_files: 1,
            }
        );
        assert_eq!(
            store
                .cleanup_expired_records(now, 10)
                .await
                .expect("second cleanup should execute"),
            ExpiredWebRecordsCleanup {
                auth_sessions: 1,
                task_events: 1,
                task_files: 0,
            }
        );
        assert_eq!(
            store
                .cleanup_expired_records(now, 10)
                .await
                .expect("idempotent cleanup should execute"),
            ExpiredWebRecordsCleanup::default()
        );
        assert_eq!(
            store
                .cleanup_expired_records(fresh_until + Duration::seconds(1), 0)
                .await
                .expect("zero-limit cleanup should no-op"),
            ExpiredWebRecordsCleanup::default()
        );

        assert!(store
            .load_auth_session("active")
            .await
            .expect("active auth session lookup should execute")
            .is_some());
        let remaining_events = store
            .list_task_events(user_id, "session-1", "task-1", 0, 10)
            .await
            .expect("remaining events should load")
            .events;
        assert_eq!(remaining_events.len(), 1);
        assert_eq!(remaining_events[0].seq, 3);
        assert!(store
            .load_task_file(user_id, "session-1", "task-1", "fresh-file")
            .await
            .expect("fresh file lookup should execute")
            .is_some());
        assert!(store
            .load_task_file(user_id, "session-1", "task-1", "expired-file")
            .await
            .expect("expired file lookup should execute")
            .is_none());
    }
}
