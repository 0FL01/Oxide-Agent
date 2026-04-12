use super::*;

const DEFAULT_MEMORY_DATABASE_MAX_CONNECTIONS: u32 = 5;
const DEFAULT_MEMORY_DATABASE_STARTUP_MAX_ATTEMPTS: u32 = 6;
const DEFAULT_MEMORY_DATABASE_STARTUP_RETRY_DELAY_MS: u64 = 2_000;
const DEFAULT_MEMORY_DATABASE_STARTUP_TIMEOUT_SECS: u64 = 10;

/// Connect the canonical Postgres-backed persistent-memory store for runtime use.
pub async fn connect_postgres_memory_store(
    settings: &crate::config::AgentSettings,
) -> Result<Arc<dyn PersistentMemoryStore>> {
    let database_url = settings
        .memory_database_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("MEMORY_DATABASE_URL is required for Postgres persistent memory")
        })?;
    let max_connections = settings
        .memory_database_max_connections
        .unwrap_or(DEFAULT_MEMORY_DATABASE_MAX_CONNECTIONS);
    let auto_migrate = settings.memory_database_auto_migrate.unwrap_or(true);
    let max_attempts = settings
        .memory_database_startup_max_attempts
        .unwrap_or(DEFAULT_MEMORY_DATABASE_STARTUP_MAX_ATTEMPTS)
        .max(1);
    let retry_delay = Duration::from_millis(
        settings
            .memory_database_startup_retry_delay_ms
            .unwrap_or(DEFAULT_MEMORY_DATABASE_STARTUP_RETRY_DELAY_MS),
    );
    let attempt_timeout = Duration::from_secs(
        settings
            .memory_database_startup_timeout_secs
            .unwrap_or(DEFAULT_MEMORY_DATABASE_STARTUP_TIMEOUT_SECS)
            .max(1),
    );
    let embedding_dimensions = settings
        .embedding_dimensions
        .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSIONS);

    let mut last_error = None;
    for attempt in 1..=max_attempts {
        let init_result = timeout(attempt_timeout, async {
            let repository = oxide_agent_memory::pg::PgMemoryRepository::connect_with_max_connections(
                database_url,
                max_connections,
            )
            .await
            .map_err(|error| anyhow::anyhow!("failed to connect Postgres persistent memory: {error}"))?;

            if auto_migrate {
                repository
                    .migrate()
                    .await
                    .map_err(|error| anyhow::anyhow!("failed to migrate Postgres persistent memory: {error}"))?;
            }

            repository
                .check_health()
                .await
                .map_err(|error| anyhow::anyhow!("Postgres persistent memory health check failed: {error}"))?;

            repository
                .ensure_vector_dimension(embedding_dimensions)
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "failed to align memory_embeddings vector dimension to {embedding_dimensions}: {error}"
                    )
                })?;

            info!(
                embedding_dimensions,
                "Persistent memory vector dimensionality confirmed."
            );

            Ok::<_, anyhow::Error>(repository)
        })
        .await;

        match init_result {
            Ok(Ok(repository)) => return Ok(Arc::new(repository)),
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => {
                last_error = Some(anyhow::anyhow!(
                    "timed out after {}s while initializing Postgres persistent memory",
                    attempt_timeout.as_secs()
                ));
            }
        }

        if attempt < max_attempts {
            if let Some(error) = last_error.as_ref() {
                warn!(
                    attempt,
                    max_attempts,
                    retry_delay_ms = retry_delay.as_millis(),
                    %error,
                    "Retrying Postgres persistent memory startup"
                );
            }
            sleep(retry_delay).await;
        }
    }

    let last_error = last_error.unwrap_or_else(|| anyhow::anyhow!("unknown startup failure"));
    Err(anyhow::anyhow!(
        "failed to initialize Postgres persistent memory after {max_attempts} attempts: {last_error}. \
         Verify MEMORY_DATABASE_URL, Postgres service health, and whether MEMORY_DATABASE_AUTO_MIGRATE \
         should bootstrap the embedded schema/pgvector extension."
    ))
}

/// Object-safe persistent-memory write surface used by the runner.
#[async_trait]
pub trait PersistentMemoryStore: Send + Sync {
    /// Create or update one scoped memory thread.
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError>;
    /// Load one scoped memory thread by identifier.
    async fn get_thread(&self, thread_id: &str) -> Result<Option<ThreadRecord>, RepositoryError>;
    /// Persist one durable episode record.
    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError>;
    /// Load one durable episode by identifier.
    async fn get_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<oxide_agent_memory::EpisodeRecord>, RepositoryError>;
    /// List durable episodes for one thread.
    async fn list_episodes_for_thread(
        &self,
        thread_id: &str,
        filter: &EpisodeListFilter,
    ) -> Result<Vec<oxide_agent_memory::EpisodeRecord>, RepositoryError>;
    /// Link one artifact to an existing durable episode.
    async fn link_episode_artifact(
        &self,
        episode_id: &str,
        artifact: ArtifactRef,
    ) -> Result<Option<oxide_agent_memory::EpisodeRecord>, RepositoryError>;
    /// Persist one reusable memory record.
    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError>;
    /// Create or update one reusable memory record.
    async fn upsert_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError>;
    /// Load one reusable memory record by identifier.
    async fn get_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<oxide_agent_memory::MemoryRecord>, RepositoryError>;
    /// Soft-delete one reusable memory record.
    async fn delete_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<oxide_agent_memory::MemoryRecord>, RepositoryError>;
    /// List reusable memory records for one context.
    async fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> Result<Vec<oxide_agent_memory::MemoryRecord>, RepositoryError>;
    /// Execute lexical search over durable episodes.
    async fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError>;
    /// Execute lexical search over reusable memories.
    async fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError>;
    /// Load embedding state for one durable-memory owner.
    async fn get_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: &str,
    ) -> Result<Option<EmbeddingRecord>, RepositoryError>;
    /// Mark one durable-memory owner as pending embedding generation.
    async fn upsert_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError>;
    /// Persist one successful embedding vector.
    async fn upsert_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError>;
    /// Persist one failed embedding generation attempt.
    async fn upsert_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError>;
    /// List episode records that still need embedding backfill.
    async fn list_episode_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<EpisodeEmbeddingCandidate>, RepositoryError>;
    /// List reusable memories that still need embedding backfill.
    async fn list_memory_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<MemoryEmbeddingCandidate>, RepositoryError>;
    /// Execute vector similarity search over durable episodes.
    async fn search_episodes_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError>;
    /// Execute vector similarity search over reusable memories.
    async fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError>;
    /// Create or update one scoped session-state record.
    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError>;
    /// List session-state records matching one filter.
    async fn list_session_states(
        &self,
        filter: &SessionStateListFilter,
    ) -> Result<Vec<SessionStateRecord>, RepositoryError>;
}

#[async_trait]
impl<T> PersistentMemoryStore for T
where
    T: MemoryRepository + Send + Sync,
{
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError> {
        MemoryRepository::upsert_thread(self, record).await
    }

    async fn get_thread(&self, thread_id: &str) -> Result<Option<ThreadRecord>, RepositoryError> {
        MemoryRepository::get_thread(self, &thread_id.to_string()).await
    }

    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError> {
        MemoryRepository::create_episode(self, record).await
    }

    async fn get_episode(
        &self,
        episode_id: &str,
    ) -> Result<Option<oxide_agent_memory::EpisodeRecord>, RepositoryError> {
        MemoryRepository::get_episode(self, &episode_id.to_string()).await
    }

    async fn list_episodes_for_thread(
        &self,
        thread_id: &str,
        filter: &EpisodeListFilter,
    ) -> Result<Vec<oxide_agent_memory::EpisodeRecord>, RepositoryError> {
        MemoryRepository::list_episodes_for_thread(self, &thread_id.to_string(), filter).await
    }

    async fn link_episode_artifact(
        &self,
        episode_id: &str,
        artifact: ArtifactRef,
    ) -> Result<Option<oxide_agent_memory::EpisodeRecord>, RepositoryError> {
        MemoryRepository::link_episode_artifact(self, &episode_id.to_string(), artifact).await
    }

    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError> {
        MemoryRepository::create_memory(self, record).await
    }

    async fn upsert_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError> {
        MemoryRepository::upsert_memory(self, record).await
    }

    async fn get_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<oxide_agent_memory::MemoryRecord>, RepositoryError> {
        MemoryRepository::get_memory(self, memory_id).await
    }

    async fn delete_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<oxide_agent_memory::MemoryRecord>, RepositoryError> {
        MemoryRepository::delete_memory(self, memory_id).await
    }

    async fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> Result<Vec<oxide_agent_memory::MemoryRecord>, RepositoryError> {
        MemoryRepository::list_memories(self, context_key, filter).await
    }

    async fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        MemoryRepository::search_episodes_lexical(self, query, filter).await
    }

    async fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        MemoryRepository::search_memories_lexical(self, query, filter).await
    }

    async fn get_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: &str,
    ) -> Result<Option<EmbeddingRecord>, RepositoryError> {
        MemoryRepository::get_embedding(self, owner_type, owner_id).await
    }

    async fn upsert_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        MemoryRepository::upsert_embedding_pending(self, update).await
    }

    async fn upsert_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        MemoryRepository::upsert_embedding_ready(self, update).await
    }

    async fn upsert_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        MemoryRepository::upsert_embedding_failure(self, update).await
    }

    async fn list_episode_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<EpisodeEmbeddingCandidate>, RepositoryError> {
        MemoryRepository::list_episode_embedding_backfill_candidates(self, request).await
    }

    async fn list_memory_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<MemoryEmbeddingCandidate>, RepositoryError> {
        MemoryRepository::list_memory_embedding_backfill_candidates(self, request).await
    }

    async fn search_episodes_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        MemoryRepository::search_episodes_vector(self, query_embedding, model_id, filter).await
    }

    async fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        MemoryRepository::search_memories_vector(self, query_embedding, model_id, filter).await
    }

    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError> {
        MemoryRepository::upsert_session_state(self, record).await
    }

    async fn list_session_states(
        &self,
        filter: &SessionStateListFilter,
    ) -> Result<Vec<SessionStateRecord>, RepositoryError> {
        MemoryRepository::list_session_states(self, filter).await
    }
}
