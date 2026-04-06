//! Postgres-backed `MemoryRepository` implementation.

use super::mapping::{
    encode_cleanup_status, encode_episode_outcome, encode_memory_type, EpisodeRow, MemoryRow,
    SessionStateRow, ThreadRow,
};
use super::migrator;
use crate::repository::{MemoryRepository, RepositoryError};
use crate::types::{
    EpisodeId, EpisodeListFilter, EpisodeRecord, MemoryListFilter, MemoryRecord,
    SessionStateRecord, ThreadId, ThreadRecord,
};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;
use sqlx::Error;
use std::future::Future;

/// Postgres-backed typed memory repository.
#[derive(Debug, Clone)]
pub struct PgMemoryRepository {
    pool: PgPool,
}

impl PgMemoryRepository {
    /// Construct a repository from an existing Postgres pool.
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Connect to Postgres and construct a repository.
    pub async fn connect(database_url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;
        Ok(Self::new(pool))
    }

    /// Return the underlying SQLx pool.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Run embedded memory-schema migrations.
    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        migrator().run(self.pool()).await
    }
}

impl MemoryRepository for PgMemoryRepository {
    fn upsert_thread(
        &self,
        record: ThreadRecord,
    ) -> impl Future<Output = Result<ThreadRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let row = sqlx::query_as::<_, ThreadRow>(
                r#"
                INSERT INTO memory_threads (
                    thread_id,
                    user_id,
                    context_key,
                    title,
                    short_summary,
                    created_at,
                    updated_at,
                    last_activity_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (thread_id) DO UPDATE
                SET
                    user_id = EXCLUDED.user_id,
                    context_key = EXCLUDED.context_key,
                    title = EXCLUDED.title,
                    short_summary = EXCLUDED.short_summary,
                    updated_at = EXCLUDED.updated_at,
                    last_activity_at = EXCLUDED.last_activity_at
                RETURNING
                    thread_id,
                    user_id,
                    context_key,
                    title,
                    short_summary,
                    created_at,
                    updated_at,
                    last_activity_at
                "#,
            )
            .bind(record.thread_id)
            .bind(record.user_id)
            .bind(record.context_key)
            .bind(record.title)
            .bind(record.short_summary)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.last_activity_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_thread", error))?;

            Ok(row.into())
        }
    }

    fn get_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl Future<Output = Result<Option<ThreadRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let thread_id = thread_id.clone();
        async move {
            let row = sqlx::query_as::<_, ThreadRow>(
                r#"
                SELECT
                    thread_id,
                    user_id,
                    context_key,
                    title,
                    short_summary,
                    created_at,
                    updated_at,
                    last_activity_at
                FROM memory_threads
                WHERE thread_id = $1
                "#,
            )
            .bind(thread_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| map_sqlx_error("get_thread", error))?;

            Ok(row.map(ThreadRecord::from))
        }
    }

    fn create_episode(
        &self,
        record: EpisodeRecord,
    ) -> impl Future<Output = Result<EpisodeRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let row = sqlx::query_as::<_, EpisodeRow>(
                r#"
                INSERT INTO memory_episodes (
                    episode_id,
                    thread_id,
                    context_key,
                    goal,
                    summary,
                    outcome,
                    tools_used,
                    artifacts,
                    failures,
                    importance,
                    created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                RETURNING
                    episode_id,
                    thread_id,
                    context_key,
                    goal,
                    summary,
                    outcome,
                    tools_used,
                    artifacts,
                    failures,
                    importance,
                    created_at
                "#,
            )
            .bind(record.episode_id)
            .bind(record.thread_id)
            .bind(record.context_key)
            .bind(record.goal)
            .bind(record.summary)
            .bind(encode_episode_outcome(record.outcome))
            .bind(record.tools_used)
            .bind(Json(record.artifacts))
            .bind(record.failures)
            .bind(record.importance)
            .bind(record.created_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_insert_error("create_episode", error))?;

            EpisodeRecord::try_from(row)
        }
    }

    fn get_episode(
        &self,
        episode_id: &EpisodeId,
    ) -> impl Future<Output = Result<Option<EpisodeRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let episode_id = episode_id.clone();
        async move {
            let row = sqlx::query_as::<_, EpisodeRow>(
                r#"
                SELECT
                    episode_id,
                    thread_id,
                    context_key,
                    goal,
                    summary,
                    outcome,
                    tools_used,
                    artifacts,
                    failures,
                    importance,
                    created_at
                FROM memory_episodes
                WHERE episode_id = $1
                "#,
            )
            .bind(episode_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| map_sqlx_error("get_episode", error))?;

            row.map(EpisodeRecord::try_from).transpose()
        }
    }

    fn list_episodes_for_thread(
        &self,
        thread_id: &ThreadId,
        filter: &EpisodeListFilter,
    ) -> impl Future<Output = Result<Vec<EpisodeRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let thread_id = thread_id.clone();
        let filter = filter.clone();
        async move {
            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let outcome = filter.outcome.map(encode_episode_outcome);
            let rows = sqlx::query_as::<_, EpisodeRow>(
                r#"
                SELECT
                    episode_id,
                    thread_id,
                    context_key,
                    goal,
                    summary,
                    outcome,
                    tools_used,
                    artifacts,
                    failures,
                    importance,
                    created_at
                FROM memory_episodes
                WHERE thread_id = $1
                  AND ($2::real IS NULL OR importance >= $2)
                  AND ($3::text IS NULL OR outcome = $3)
                ORDER BY created_at DESC, episode_id ASC
                LIMIT COALESCE($4, 100)
                "#,
            )
            .bind(thread_id)
            .bind(filter.min_importance)
            .bind(outcome)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_episodes_for_thread", error))?;

            rows.into_iter().map(EpisodeRecord::try_from).collect()
        }
    }

    fn create_memory(
        &self,
        record: MemoryRecord,
    ) -> impl Future<Output = Result<MemoryRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let row = sqlx::query_as::<_, MemoryRow>(
                r#"
                INSERT INTO memory_records (
                    memory_id,
                    context_key,
                    source_episode_id,
                    memory_type,
                    title,
                    content,
                    short_description,
                    importance,
                    confidence,
                    tags,
                    created_at,
                    updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
                RETURNING
                    memory_id,
                    context_key,
                    source_episode_id,
                    memory_type,
                    title,
                    content,
                    short_description,
                    importance,
                    confidence,
                    tags,
                    created_at,
                    updated_at
                "#,
            )
            .bind(record.memory_id)
            .bind(record.context_key)
            .bind(record.source_episode_id)
            .bind(encode_memory_type(record.memory_type))
            .bind(record.title)
            .bind(record.content)
            .bind(record.short_description)
            .bind(record.importance)
            .bind(record.confidence)
            .bind(record.tags)
            .bind(record.created_at)
            .bind(record.updated_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_insert_error("create_memory", error))?;

            MemoryRecord::try_from(row)
        }
    }

    fn get_memory(
        &self,
        memory_id: &str,
    ) -> impl Future<Output = Result<Option<MemoryRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let memory_id = memory_id.to_string();
        async move {
            let row = sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT
                    memory_id,
                    context_key,
                    source_episode_id,
                    memory_type,
                    title,
                    content,
                    short_description,
                    importance,
                    confidence,
                    tags,
                    created_at,
                    updated_at
                FROM memory_records
                WHERE memory_id = $1
                "#,
            )
            .bind(memory_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| map_sqlx_error("get_memory", error))?;

            row.map(MemoryRecord::try_from).transpose()
        }
    }

    fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> impl Future<Output = Result<Vec<MemoryRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let context_key = context_key.to_string();
        let filter = filter.clone();
        async move {
            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let memory_type = filter.memory_type.map(encode_memory_type);
            let required_tags = if filter.tags.is_empty() {
                None
            } else {
                Some(filter.tags)
            };
            let rows = sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT
                    memory_id,
                    context_key,
                    source_episode_id,
                    memory_type,
                    title,
                    content,
                    short_description,
                    importance,
                    confidence,
                    tags,
                    created_at,
                    updated_at
                FROM memory_records
                WHERE context_key = $1
                  AND ($2::text IS NULL OR memory_type = $2)
                  AND ($3::real IS NULL OR importance >= $3)
                  AND ($4::text[] IS NULL OR tags @> $4)
                ORDER BY updated_at DESC, memory_id ASC
                LIMIT COALESCE($5, 100)
                "#,
            )
            .bind(context_key)
            .bind(memory_type)
            .bind(filter.min_importance)
            .bind(required_tags)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_memories", error))?;

            rows.into_iter().map(MemoryRecord::try_from).collect()
        }
    }

    fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> impl Future<Output = Result<SessionStateRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let hot_token_estimate = i64::try_from(record.hot_token_estimate).map_err(|_| {
                RepositoryError::Storage(format!(
                    "session {} hot_token_estimate does not fit into i64",
                    record.session_id
                ))
            })?;

            let row = sqlx::query_as::<_, SessionStateRow>(
                r#"
                INSERT INTO memory_session_state (
                    session_id,
                    context_key,
                    hot_token_estimate,
                    last_compacted_at,
                    last_finalized_at,
                    cleanup_status,
                    pending_episode_id,
                    updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                ON CONFLICT (session_id) DO UPDATE
                SET
                    context_key = EXCLUDED.context_key,
                    hot_token_estimate = EXCLUDED.hot_token_estimate,
                    last_compacted_at = EXCLUDED.last_compacted_at,
                    last_finalized_at = EXCLUDED.last_finalized_at,
                    cleanup_status = EXCLUDED.cleanup_status,
                    pending_episode_id = EXCLUDED.pending_episode_id,
                    updated_at = EXCLUDED.updated_at
                RETURNING
                    session_id,
                    context_key,
                    hot_token_estimate,
                    last_compacted_at,
                    last_finalized_at,
                    cleanup_status,
                    pending_episode_id,
                    updated_at
                "#,
            )
            .bind(record.session_id)
            .bind(record.context_key)
            .bind(hot_token_estimate)
            .bind(record.last_compacted_at)
            .bind(record.last_finalized_at)
            .bind(encode_cleanup_status(record.cleanup_status))
            .bind(record.pending_episode_id)
            .bind(record.updated_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_session_state", error))?;

            SessionStateRecord::try_from(row)
        }
    }

    fn get_session_state(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<Option<SessionStateRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let session_id = session_id.to_string();
        async move {
            let row = sqlx::query_as::<_, SessionStateRow>(
                r#"
                SELECT
                    session_id,
                    context_key,
                    hot_token_estimate,
                    last_compacted_at,
                    last_finalized_at,
                    cleanup_status,
                    pending_episode_id,
                    updated_at
                FROM memory_session_state
                WHERE session_id = $1
                "#,
            )
            .bind(session_id)
            .fetch_optional(&pool)
            .await
            .map_err(|error| map_sqlx_error("get_session_state", error))?;

            row.map(SessionStateRecord::try_from).transpose()
        }
    }
}

fn map_sqlx_error(context: &str, error: Error) -> RepositoryError {
    match error {
        Error::RowNotFound => RepositoryError::NotFound(format!("{context}: row not found")),
        Error::Database(database_error) if database_error.is_unique_violation() => {
            RepositoryError::Conflict(format!("{context}: {}", database_error.message()))
        }
        other => RepositoryError::Storage(format!("{context}: {other}")),
    }
}

fn map_insert_error(context: &str, error: Error) -> RepositoryError {
    match error {
        Error::Database(database_error) if database_error.is_unique_violation() => {
            RepositoryError::Conflict(format!("{context}: {}", database_error.message()))
        }
        other => RepositoryError::Storage(format!("{context}: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::PgMemoryRepository;
    use sqlx::postgres::PgPoolOptions;

    #[tokio::test]
    async fn repository_wraps_lazy_pool() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/oxide_agent")
            .expect("lazy pool should parse url");
        let repository = PgMemoryRepository::new(pool);
        assert_eq!(repository.pool().size(), 0);
    }
}
