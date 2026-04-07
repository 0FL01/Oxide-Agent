//! Postgres-backed `MemoryRepository` implementation.

use super::mapping::{
    encode_cleanup_status, encode_embedding_owner_type, encode_embedding_status,
    encode_episode_outcome, encode_memory_type, EmbeddingRow, EpisodeRow, EpisodeSearchRow,
    MemoryRow, MemorySearchRow, SessionStateRow, ThreadRow,
};
use super::migrator;
use crate::repository::{MemoryRepository, RepositoryError};
use crate::types::{
    ArtifactRef, EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType,
    EmbeddingPendingUpdate, EmbeddingReadyUpdate, EmbeddingRecord, EpisodeEmbeddingCandidate,
    EpisodeId, EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit,
    MemoryEmbeddingCandidate, MemoryListFilter, MemoryRecord, MemorySearchFilter, MemorySearchHit,
    SessionStateListFilter, SessionStateRecord, ThreadId, ThreadRecord,
};
use anyhow::bail;
use pgvector::Vector;
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
        Self::connect_with_max_connections(database_url, 5).await
    }

    /// Connect to Postgres with an explicit pool size.
    pub async fn connect_with_max_connections(
        database_url: &str,
        max_connections: u32,
    ) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
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

    /// Verify that Postgres, pgvector, and the current typed-memory schema are ready.
    pub async fn check_health(&self) -> anyhow::Result<()> {
        sqlx::query_scalar::<_, i64>("SELECT 1::INT8")
            .fetch_one(self.pool())
            .await?;

        ensure_extension_installed(self.pool(), "vector").await?;

        for table in [
            "memory_threads",
            "memory_episodes",
            "memory_records",
            "memory_embeddings",
            "memory_session_state",
        ] {
            ensure_table_exists(self.pool(), table).await?;
        }

        for (table, column) in [
            ("memory_records", "content_hash"),
            ("memory_records", "deleted_at"),
        ] {
            ensure_column_exists(self.pool(), table, column).await?;
        }

        Ok(())
    }
}

async fn ensure_extension_installed(pool: &PgPool, extension_name: &str) -> anyhow::Result<()> {
    let installed = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = $1)",
    )
    .bind(extension_name)
    .fetch_one(pool)
    .await?;

    if installed {
        Ok(())
    } else {
        bail!("required Postgres extension '{extension_name}' is not installed")
    }
}

async fn ensure_table_exists(pool: &PgPool, table_name: &str) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'public' AND table_name = $1)",
    )
    .bind(table_name)
    .fetch_one(pool)
    .await?;

    if exists {
        Ok(())
    } else {
        bail!("required Postgres table 'public.{table_name}' is missing")
    }
}

async fn ensure_column_exists(
    pool: &PgPool,
    table_name: &str,
    column_name: &str,
) -> anyhow::Result<()> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2)",
    )
    .bind(table_name)
    .bind(column_name)
    .fetch_one(pool)
    .await?;

    if exists {
        Ok(())
    } else {
        bail!("required Postgres column 'public.{table_name}.{column_name}' is missing")
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

    fn link_episode_artifact(
        &self,
        episode_id: &EpisodeId,
        artifact: ArtifactRef,
    ) -> impl Future<Output = Result<Option<EpisodeRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let episode_id = episode_id.clone();
        async move {
            let mut transaction = pool
                .begin()
                .await
                .map_err(|error| map_sqlx_error("link_episode_artifact.begin", error))?;

            let Some(row) = sqlx::query_as::<_, EpisodeRow>(
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
                FOR UPDATE
                "#,
            )
            .bind(&episode_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| map_sqlx_error("link_episode_artifact.select", error))?
            else {
                transaction.commit().await.map_err(|error| {
                    map_sqlx_error("link_episode_artifact.commit_missing", error)
                })?;
                return Ok(None);
            };

            let mut episode = EpisodeRecord::try_from(row)
                .map_err(|error| RepositoryError::Storage(error.to_string()))?;
            if episode
                .artifacts
                .iter()
                .all(|candidate| candidate.storage_key != artifact.storage_key)
            {
                episode.artifacts.push(artifact);
                let updated_row = sqlx::query_as::<_, EpisodeRow>(
                    r#"
                    UPDATE memory_episodes
                    SET artifacts = $2
                    WHERE episode_id = $1
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
                .bind(&episode_id)
                .bind(Json(episode.artifacts.clone()))
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| map_sqlx_error("link_episode_artifact.update", error))?;
                episode = EpisodeRecord::try_from(updated_row)
                    .map_err(|error| RepositoryError::Storage(error.to_string()))?;
            }

            transaction
                .commit()
                .await
                .map_err(|error| map_sqlx_error("link_episode_artifact.commit", error))?;

            Ok(Some(episode))
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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
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
            .bind(record.source)
            .bind(record.content_hash)
            .bind(record.reason)
            .bind(record.tags)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_insert_error("create_memory", error))?;

            MemoryRecord::try_from(row)
        }
    }

    fn upsert_memory(
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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                ON CONFLICT (memory_id) DO UPDATE
                SET
                    context_key = EXCLUDED.context_key,
                    source_episode_id = EXCLUDED.source_episode_id,
                    memory_type = EXCLUDED.memory_type,
                    title = EXCLUDED.title,
                    content = EXCLUDED.content,
                    short_description = EXCLUDED.short_description,
                    importance = EXCLUDED.importance,
                    confidence = EXCLUDED.confidence,
                    source = EXCLUDED.source,
                    content_hash = EXCLUDED.content_hash,
                    reason = EXCLUDED.reason,
                    tags = EXCLUDED.tags,
                    updated_at = EXCLUDED.updated_at,
                    deleted_at = EXCLUDED.deleted_at
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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
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
            .bind(record.source)
            .bind(record.content_hash)
            .bind(record.reason)
            .bind(record.tags)
            .bind(record.created_at)
            .bind(record.updated_at)
            .bind(record.deleted_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_memory", error))?;

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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
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

    fn delete_memory(
        &self,
        memory_id: &str,
    ) -> impl Future<Output = Result<Option<MemoryRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let memory_id = memory_id.to_string();
        async move {
            let deleted_at = chrono::Utc::now();
            let row = sqlx::query_as::<_, MemoryRow>(
                r#"
                UPDATE memory_records
                SET deleted_at = COALESCE(deleted_at, $2),
                    updated_at = CASE WHEN deleted_at IS NULL THEN $2 ELSE updated_at END
                WHERE memory_id = $1
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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
                "#,
            )
            .bind(memory_id)
            .bind(deleted_at)
            .fetch_optional(&pool)
            .await
            .map_err(|error| map_sqlx_error("delete_memory", error))?;

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
                    source,
                    content_hash,
                    reason,
                    tags,
                    created_at,
                    updated_at,
                    deleted_at
                FROM memory_records
                WHERE context_key = $1
                  AND ($2::text IS NULL OR memory_type = $2)
                  AND ($3::real IS NULL OR importance >= $3)
                  AND ($4::text[] IS NULL OR tags @> $4)
                  AND ($5::boolean OR deleted_at IS NULL)
                ORDER BY updated_at DESC, memory_id ASC
                LIMIT COALESCE($6, 100)
                "#,
            )
            .bind(context_key)
            .bind(memory_type)
            .bind(filter.min_importance)
            .bind(required_tags)
            .bind(filter.include_deleted)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_memories", error))?;

            rows.into_iter().map(MemoryRecord::try_from).collect()
        }
    }

    fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> impl Future<Output = Result<Vec<EpisodeSearchHit>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let query = query.trim().to_string();
        let filter = filter.clone();
        async move {
            if query.is_empty() {
                return Ok(Vec::new());
            }

            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let outcome = filter.outcome.map(encode_episode_outcome);
            let rows = sqlx::query_as::<_, EpisodeSearchRow>(
                r#"
                SELECT
                    episodes.episode_id,
                    episodes.thread_id,
                    episodes.context_key,
                    episodes.goal,
                    episodes.summary,
                    episodes.outcome,
                    episodes.tools_used,
                    episodes.artifacts,
                    episodes.failures,
                    episodes.importance,
                    episodes.created_at,
                    ts_rank_cd(
                        to_tsvector(
                            'simple',
                            concat_ws(
                                ' ',
                                episodes.goal,
                                episodes.summary,
                                array_to_string(episodes.tools_used, ' '),
                                array_to_string(episodes.failures, ' ')
                            )
                        ),
                        websearch_to_tsquery('simple', $1)
                    ) AS lexical_score,
                    ts_headline(
                        'simple',
                        concat_ws(E'\n', episodes.goal, episodes.summary),
                        websearch_to_tsquery('simple', $1),
                        'MaxFragments=2, MaxWords=20, MinWords=8'
                    ) AS lexical_snippet
                FROM memory_episodes AS episodes
                INNER JOIN memory_threads AS threads
                    ON threads.thread_id = episodes.thread_id
                WHERE to_tsvector(
                        'simple',
                        concat_ws(
                            ' ',
                            episodes.goal,
                            episodes.summary,
                            array_to_string(episodes.tools_used, ' '),
                            array_to_string(episodes.failures, ' ')
                        )
                    ) @@ websearch_to_tsquery('simple', $1)
                  AND ($2::text IS NULL OR episodes.context_key = $2)
                  AND ($3::bigint IS NULL OR threads.user_id = $3)
                  AND ($4::text IS NULL OR episodes.outcome = $4)
                  AND ($5::real IS NULL OR episodes.importance >= $5)
                  AND ($6::timestamptz IS NULL OR episodes.created_at >= $6)
                  AND ($7::timestamptz IS NULL OR episodes.created_at <= $7)
                ORDER BY lexical_score DESC,
                         episodes.importance DESC,
                         episodes.created_at DESC,
                         episodes.episode_id ASC
                LIMIT COALESCE($8, 20)
                "#,
            )
            .bind(query)
            .bind(filter.context_key)
            .bind(filter.user_id)
            .bind(outcome)
            .bind(filter.min_importance)
            .bind(filter.time_range.since)
            .bind(filter.time_range.until)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("search_episodes_lexical", error))?;

            rows.into_iter().map(EpisodeSearchHit::try_from).collect()
        }
    }

    fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> impl Future<Output = Result<Vec<MemorySearchHit>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let query = query.trim().to_string();
        let filter = filter.clone();
        async move {
            if query.is_empty() {
                return Ok(Vec::new());
            }

            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let memory_type = filter.memory_type.map(encode_memory_type);
            let required_tags = if filter.tags.is_empty() {
                None
            } else {
                Some(filter.tags)
            };
            let rows = sqlx::query_as::<_, MemorySearchRow>(
                r#"
                SELECT
                    memories.memory_id,
                    memories.context_key,
                    memories.source_episode_id,
                    memories.memory_type,
                    memories.title,
                    memories.content,
                    memories.short_description,
                    memories.importance,
                    memories.confidence,
                    memories.source,
                    memories.content_hash,
                    memories.reason,
                    memories.tags,
                    memories.created_at,
                    memories.updated_at,
                    memories.deleted_at,
                    ts_rank_cd(
                        to_tsvector(
                            'simple',
                            concat_ws(
                                ' ',
                                memories.title,
                                memories.short_description,
                                memories.content,
                                COALESCE(memories.source, ''),
                                COALESCE(memories.reason, ''),
                                array_to_string(memories.tags, ' ')
                            )
                        ),
                        websearch_to_tsquery('simple', $1)
                    ) AS lexical_score,
                    ts_headline(
                        'simple',
                        concat_ws(
                            E'\n',
                            memories.title,
                            memories.short_description,
                            memories.content,
                            memories.source,
                            memories.reason
                        ),
                        websearch_to_tsquery('simple', $1),
                        'MaxFragments=2, MaxWords=20, MinWords=8'
                    ) AS lexical_snippet
                FROM memory_records AS memories
                LEFT JOIN memory_episodes AS episodes
                    ON episodes.episode_id = memories.source_episode_id
                LEFT JOIN memory_threads AS threads
                    ON threads.thread_id = episodes.thread_id
                WHERE to_tsvector(
                        'simple',
                        concat_ws(
                            ' ',
                            memories.title,
                            memories.short_description,
                            memories.content,
                            COALESCE(memories.source, ''),
                            COALESCE(memories.reason, ''),
                            array_to_string(memories.tags, ' ')
                        )
                    ) @@ websearch_to_tsquery('simple', $1)
                  AND ($2::text IS NULL OR memories.context_key = $2)
                  AND ($3::bigint IS NULL OR threads.user_id = $3)
                  AND ($4::text IS NULL OR memories.memory_type = $4)
                  AND ($5::real IS NULL OR memories.importance >= $5)
                  AND ($6::text[] IS NULL OR memories.tags @> $6)
                  AND ($7::timestamptz IS NULL OR memories.updated_at >= $7)
                  AND ($8::timestamptz IS NULL OR memories.updated_at <= $8)
                  AND memories.deleted_at IS NULL
                ORDER BY lexical_score DESC,
                         memories.importance DESC,
                         memories.confidence DESC,
                         memories.updated_at DESC,
                         memories.memory_id ASC
                LIMIT COALESCE($9, 20)
                "#,
            )
            .bind(query)
            .bind(filter.context_key)
            .bind(filter.user_id)
            .bind(memory_type)
            .bind(filter.min_importance)
            .bind(required_tags)
            .bind(filter.time_range.since)
            .bind(filter.time_range.until)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("search_memories_lexical", error))?;

            rows.into_iter().map(MemorySearchHit::try_from).collect()
        }
    }

    fn get_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: &str,
    ) -> impl Future<Output = Result<Option<EmbeddingRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let owner_type = encode_embedding_owner_type(owner_type).to_string();
        let owner_id = owner_id.to_string();
        async move { fetch_embedding_row(&pool, owner_type, owner_id).await }
    }

    fn upsert_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> impl Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let row = sqlx::query_as::<_, EmbeddingRow>(
                r#"
                INSERT INTO memory_embeddings (
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                )
                VALUES ($1, $2, $3, $4, NULL, NULL, $5, NULL, 0, $6, $6, NULL)
                ON CONFLICT (owner_type, owner_id) DO UPDATE
                SET
                    model_id = EXCLUDED.model_id,
                    content_hash = EXCLUDED.content_hash,
                    embedding = NULL,
                    dimensions = NULL,
                    status = EXCLUDED.status,
                    last_error = NULL,
                    updated_at = EXCLUDED.updated_at,
                    indexed_at = NULL
                RETURNING
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                "#,
            )
            .bind(update.base.owner_id)
            .bind(encode_embedding_owner_type(update.base.owner_type))
            .bind(update.base.model_id)
            .bind(update.base.content_hash)
            .bind(encode_embedding_status(
                crate::types::EmbeddingStatus::Pending,
            ))
            .bind(update.requested_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_embedding_pending", error))?;

            EmbeddingRecord::try_from(row)
        }
    }

    fn upsert_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> impl Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let dimensions = i32::try_from(update.embedding.len()).map_err(|_| {
                RepositoryError::Storage("embedding dimensions do not fit into i32".to_string())
            })?;
            let row = sqlx::query_as::<_, EmbeddingRow>(
                r#"
                INSERT INTO memory_embeddings (
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, NULL, 0, $8, $8, $8)
                ON CONFLICT (owner_type, owner_id) DO UPDATE
                SET
                    model_id = EXCLUDED.model_id,
                    content_hash = EXCLUDED.content_hash,
                    embedding = EXCLUDED.embedding,
                    dimensions = EXCLUDED.dimensions,
                    status = EXCLUDED.status,
                    last_error = NULL,
                    updated_at = EXCLUDED.updated_at,
                    indexed_at = EXCLUDED.indexed_at
                RETURNING
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                "#,
            )
            .bind(update.base.owner_id)
            .bind(encode_embedding_owner_type(update.base.owner_type))
            .bind(update.base.model_id)
            .bind(update.base.content_hash)
            .bind(Vector::from(update.embedding))
            .bind(dimensions)
            .bind(encode_embedding_status(
                crate::types::EmbeddingStatus::Ready,
            ))
            .bind(update.indexed_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_embedding_ready", error))?;

            EmbeddingRecord::try_from(row)
        }
    }

    fn upsert_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> impl Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send {
        let pool = self.pool.clone();
        async move {
            let row = sqlx::query_as::<_, EmbeddingRow>(
                r#"
                INSERT INTO memory_embeddings (
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                )
                VALUES ($1, $2, $3, $4, NULL, NULL, $5, $6, 1, $7, $7, NULL)
                ON CONFLICT (owner_type, owner_id) DO UPDATE
                SET
                    model_id = EXCLUDED.model_id,
                    content_hash = EXCLUDED.content_hash,
                    embedding = NULL,
                    dimensions = NULL,
                    status = EXCLUDED.status,
                    last_error = EXCLUDED.last_error,
                    retry_count = memory_embeddings.retry_count + 1,
                    updated_at = EXCLUDED.updated_at
                RETURNING
                    owner_id,
                    owner_type,
                    model_id,
                    content_hash,
                    embedding,
                    dimensions,
                    status,
                    last_error,
                    retry_count,
                    created_at,
                    updated_at,
                    indexed_at
                "#,
            )
            .bind(update.base.owner_id)
            .bind(encode_embedding_owner_type(update.base.owner_type))
            .bind(update.base.model_id)
            .bind(update.base.content_hash)
            .bind(encode_embedding_status(
                crate::types::EmbeddingStatus::Failed,
            ))
            .bind(update.error)
            .bind(update.failed_at)
            .fetch_one(&pool)
            .await
            .map_err(|error| map_sqlx_error("upsert_embedding_failure", error))?;

            EmbeddingRecord::try_from(row)
        }
    }

    fn list_episode_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> impl Future<Output = Result<Vec<EpisodeEmbeddingCandidate>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let request = request.clone();
        async move {
            let limit = request.limit.and_then(|value| i64::try_from(value).ok());
            let rows = sqlx::query_as::<_, EpisodeRow>(
                r#"
                SELECT
                    episodes.episode_id,
                    episodes.thread_id,
                    episodes.context_key,
                    episodes.goal,
                    episodes.summary,
                    episodes.outcome,
                    episodes.tools_used,
                    episodes.artifacts,
                    episodes.failures,
                    episodes.importance,
                    episodes.created_at
                FROM memory_episodes AS episodes
                LEFT JOIN memory_embeddings AS embeddings
                    ON embeddings.owner_type = 'episode'
                   AND embeddings.owner_id = episodes.episode_id
                WHERE embeddings.owner_id IS NULL
                   OR embeddings.model_id <> $1
                   OR embeddings.status <> 'ready'
                   OR embeddings.embedding IS NULL
                ORDER BY episodes.created_at ASC, episodes.episode_id ASC
                LIMIT COALESCE($2, 100)
                "#,
            )
            .bind(request.model_id.clone())
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_episode_embedding_backfill_candidates", error))?;

            let mut candidates = Vec::with_capacity(rows.len());
            for row in rows {
                let record = EpisodeRecord::try_from(row)?;
                let embedding =
                    fetch_embedding_row(&pool, "episode".to_string(), record.episode_id.clone())
                        .await?;
                candidates.push(EpisodeEmbeddingCandidate { record, embedding });
            }
            Ok(candidates)
        }
    }

    fn list_memory_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> impl Future<Output = Result<Vec<MemoryEmbeddingCandidate>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let request = request.clone();
        async move {
            let limit = request.limit.and_then(|value| i64::try_from(value).ok());
            let rows = sqlx::query_as::<_, MemoryRow>(
                r#"
                SELECT
                    memories.memory_id,
                    memories.context_key,
                    memories.source_episode_id,
                    memories.memory_type,
                    memories.title,
                    memories.content,
                    memories.short_description,
                    memories.importance,
                    memories.confidence,
                    memories.source,
                    memories.reason,
                    memories.tags,
                    memories.created_at,
                    memories.updated_at
                FROM memory_records AS memories
                LEFT JOIN memory_embeddings AS embeddings
                    ON embeddings.owner_type = 'memory'
                   AND embeddings.owner_id = memories.memory_id
                WHERE embeddings.owner_id IS NULL
                   OR embeddings.model_id <> $1
                   OR embeddings.status <> 'ready'
                   OR embeddings.embedding IS NULL
                ORDER BY memories.updated_at ASC, memories.memory_id ASC
                LIMIT COALESCE($2, 100)
                "#,
            )
            .bind(request.model_id.clone())
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_memory_embedding_backfill_candidates", error))?;

            let mut candidates = Vec::with_capacity(rows.len());
            for row in rows {
                let record = MemoryRecord::try_from(row)?;
                let embedding =
                    fetch_embedding_row(&pool, "memory".to_string(), record.memory_id.clone())
                        .await?;
                candidates.push(MemoryEmbeddingCandidate { record, embedding });
            }
            Ok(candidates)
        }
    }

    fn search_episodes_vector(
        &self,
        query_embedding: &[f32],
        filter: &EpisodeSearchFilter,
    ) -> impl Future<Output = Result<Vec<EpisodeSearchHit>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let query_embedding = query_embedding.to_vec();
        let filter = filter.clone();
        async move {
            if query_embedding.is_empty() {
                return Ok(Vec::new());
            }
            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let outcome = filter.outcome.map(encode_episode_outcome);
            let rows = sqlx::query_as::<_, EpisodeSearchRow>(
                r#"
                SELECT
                    episodes.episode_id,
                    episodes.thread_id,
                    episodes.context_key,
                    episodes.goal,
                    episodes.summary,
                    episodes.outcome,
                    episodes.tools_used,
                    episodes.artifacts,
                    episodes.failures,
                    episodes.importance,
                    episodes.created_at,
                    CAST(1 - (embeddings.embedding <=> $1) AS real) AS lexical_score,
                    LEFT(concat_ws(E'\n', episodes.goal, episodes.summary), 160) AS lexical_snippet
                FROM memory_episodes AS episodes
                INNER JOIN memory_embeddings AS embeddings
                    ON embeddings.owner_type = 'episode'
                   AND embeddings.owner_id = episodes.episode_id
                   AND embeddings.status = 'ready'
                   AND embeddings.embedding IS NOT NULL
                INNER JOIN memory_threads AS threads
                    ON threads.thread_id = episodes.thread_id
                WHERE ($2::text IS NULL OR episodes.context_key = $2)
                  AND ($3::bigint IS NULL OR threads.user_id = $3)
                  AND ($4::text IS NULL OR episodes.outcome = $4)
                  AND ($5::real IS NULL OR episodes.importance >= $5)
                  AND ($6::timestamptz IS NULL OR episodes.created_at >= $6)
                  AND ($7::timestamptz IS NULL OR episodes.created_at <= $7)
                ORDER BY embeddings.embedding <=> $1 ASC,
                         episodes.importance DESC,
                         episodes.created_at DESC,
                         episodes.episode_id ASC
                LIMIT COALESCE($8, 20)
                "#,
            )
            .bind(Vector::from(query_embedding))
            .bind(filter.context_key)
            .bind(filter.user_id)
            .bind(outcome)
            .bind(filter.min_importance)
            .bind(filter.time_range.since)
            .bind(filter.time_range.until)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("search_episodes_vector", error))?;
            rows.into_iter().map(EpisodeSearchHit::try_from).collect()
        }
    }

    fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        filter: &MemorySearchFilter,
    ) -> impl Future<Output = Result<Vec<MemorySearchHit>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let query_embedding = query_embedding.to_vec();
        let filter = filter.clone();
        async move {
            if query_embedding.is_empty() {
                return Ok(Vec::new());
            }
            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let memory_type = filter.memory_type.map(encode_memory_type);
            let required_tags = if filter.tags.is_empty() {
                None
            } else {
                Some(filter.tags)
            };
            let rows = sqlx::query_as::<_, MemorySearchRow>(
                r#"
                SELECT
                    memories.memory_id,
                    memories.context_key,
                    memories.source_episode_id,
                    memories.memory_type,
                    memories.title,
                    memories.content,
                    memories.short_description,
                    memories.importance,
                    memories.confidence,
                    memories.source,
                    memories.content_hash,
                    memories.reason,
                    memories.tags,
                    memories.created_at,
                    memories.updated_at,
                    memories.deleted_at,
                    CAST(1 - (embeddings.embedding <=> $1) AS real) AS lexical_score,
                    LEFT(
                        concat_ws(E'\n', memories.title, memories.short_description, memories.content),
                        160
                    ) AS lexical_snippet
                FROM memory_records AS memories
                INNER JOIN memory_embeddings AS embeddings
                    ON embeddings.owner_type = 'memory'
                   AND embeddings.owner_id = memories.memory_id
                   AND embeddings.status = 'ready'
                   AND embeddings.embedding IS NOT NULL
                LEFT JOIN memory_episodes AS episodes
                    ON episodes.episode_id = memories.source_episode_id
                LEFT JOIN memory_threads AS threads
                    ON threads.thread_id = episodes.thread_id
                WHERE ($2::text IS NULL OR memories.context_key = $2)
                  AND ($3::bigint IS NULL OR threads.user_id = $3)
                  AND ($4::text IS NULL OR memories.memory_type = $4)
                  AND ($5::real IS NULL OR memories.importance >= $5)
                  AND ($6::text[] IS NULL OR memories.tags @> $6)
                  AND ($7::timestamptz IS NULL OR memories.updated_at >= $7)
                  AND ($8::timestamptz IS NULL OR memories.updated_at <= $8)
                  AND memories.deleted_at IS NULL
                ORDER BY embeddings.embedding <=> $1 ASC,
                         memories.importance DESC,
                         memories.confidence DESC,
                         memories.updated_at DESC,
                         memories.memory_id ASC
                LIMIT COALESCE($9, 20)
                "#,
            )
            .bind(Vector::from(query_embedding))
            .bind(filter.context_key)
            .bind(filter.user_id)
            .bind(memory_type)
            .bind(filter.min_importance)
            .bind(required_tags)
            .bind(filter.time_range.since)
            .bind(filter.time_range.until)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("search_memories_vector", error))?;
            rows.into_iter().map(MemorySearchHit::try_from).collect()
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

    fn list_session_states(
        &self,
        filter: &SessionStateListFilter,
    ) -> impl Future<Output = Result<Vec<SessionStateRecord>, RepositoryError>> + Send {
        let pool = self.pool.clone();
        let filter = filter.clone();
        async move {
            let limit = filter.limit.and_then(|value| i64::try_from(value).ok());
            let statuses = if filter.statuses.is_empty() {
                None
            } else {
                Some(
                    filter
                        .statuses
                        .into_iter()
                        .map(encode_cleanup_status)
                        .collect::<Vec<_>>(),
                )
            };
            let rows = sqlx::query_as::<_, SessionStateRow>(
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
                WHERE ($1::text IS NULL OR context_key = $1)
                  AND ($2::text[] IS NULL OR cleanup_status = ANY($2))
                  AND ($3::timestamptz IS NULL OR updated_at <= $3)
                ORDER BY updated_at ASC, session_id ASC
                LIMIT COALESCE($4, 100)
                "#,
            )
            .bind(filter.context_key)
            .bind(statuses)
            .bind(filter.updated_before)
            .bind(limit)
            .fetch_all(&pool)
            .await
            .map_err(|error| map_sqlx_error("list_session_states", error))?;

            rows.into_iter().map(SessionStateRecord::try_from).collect()
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

async fn fetch_embedding_row(
    pool: &PgPool,
    owner_type: String,
    owner_id: String,
) -> Result<Option<EmbeddingRecord>, RepositoryError> {
    let row = sqlx::query_as::<_, EmbeddingRow>(
        r#"
        SELECT
            owner_id,
            owner_type,
            model_id,
            content_hash,
            embedding,
            dimensions,
            status,
            last_error,
            retry_count,
            created_at,
            updated_at,
            indexed_at
        FROM memory_embeddings
        WHERE owner_type = $1 AND owner_id = $2
        "#,
    )
    .bind(owner_type)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| map_sqlx_error("get_embedding", error))?;
    row.map(EmbeddingRecord::try_from).transpose()
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
