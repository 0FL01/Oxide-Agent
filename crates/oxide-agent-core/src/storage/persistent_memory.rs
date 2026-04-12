use super::StorageProvider;
use oxide_agent_memory::{
    ArtifactRef, EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType,
    EmbeddingPendingUpdate, EmbeddingReadyUpdate, EmbeddingRecord, EpisodeEmbeddingCandidate,
    EpisodeId, EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit,
    MemoryEmbeddingCandidate, MemoryListFilter, MemoryRecord, MemoryRepository, MemorySearchFilter,
    MemorySearchHit, RepositoryError, SessionStateListFilter, SessionStateRecord, ThreadId,
    ThreadRecord,
};
use std::future::Future;
use std::sync::Arc;

/// `MemoryRepository` adapter backed by the transport-agnostic storage facade.
#[derive(Clone)]
pub struct StorageMemoryRepository {
    storage: Arc<dyn StorageProvider>,
}

impl StorageMemoryRepository {
    /// Create a new storage-backed memory repository wrapper.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self { storage }
    }
}

impl MemoryRepository for StorageMemoryRepository {
    fn upsert_thread(
        &self,
        record: ThreadRecord,
    ) -> impl Future<Output = Result<ThreadRecord, RepositoryError>> + Send {
        let storage = Arc::clone(&self.storage);
        async move {
            storage
                .upsert_memory_thread(record)
                .await
                .map_err(map_storage_error)
        }
    }

    async fn get_thread(
        &self,
        thread_id: &ThreadId,
    ) -> Result<Option<ThreadRecord>, RepositoryError> {
        self.storage
            .get_memory_thread(thread_id.clone())
            .await
            .map_err(map_storage_error)
    }

    fn create_episode(
        &self,
        record: EpisodeRecord,
    ) -> impl Future<Output = Result<EpisodeRecord, RepositoryError>> + Send {
        let storage = Arc::clone(&self.storage);
        async move {
            storage
                .create_memory_episode(record)
                .await
                .map_err(map_storage_error)
        }
    }

    async fn get_episode(
        &self,
        episode_id: &EpisodeId,
    ) -> Result<Option<EpisodeRecord>, RepositoryError> {
        self.storage
            .get_memory_episode(episode_id.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn list_episodes_for_thread(
        &self,
        thread_id: &ThreadId,
        filter: &EpisodeListFilter,
    ) -> Result<Vec<EpisodeRecord>, RepositoryError> {
        self.storage
            .list_memory_episodes_for_thread(thread_id.clone(), filter.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn link_episode_artifact(
        &self,
        episode_id: &EpisodeId,
        artifact: ArtifactRef,
    ) -> Result<Option<EpisodeRecord>, RepositoryError> {
        self.storage
            .link_memory_episode_artifact(episode_id.clone(), artifact)
            .await
            .map_err(map_storage_error)
    }

    async fn create_memory(&self, record: MemoryRecord) -> Result<MemoryRecord, RepositoryError> {
        self.storage
            .create_memory_record(record)
            .await
            .map_err(map_storage_error)
    }

    async fn upsert_memory(&self, record: MemoryRecord) -> Result<MemoryRecord, RepositoryError> {
        self.storage
            .upsert_memory_record(record)
            .await
            .map_err(map_storage_error)
    }

    async fn get_memory(&self, memory_id: &str) -> Result<Option<MemoryRecord>, RepositoryError> {
        self.storage
            .get_memory_record(memory_id.to_string())
            .await
            .map_err(map_storage_error)
    }

    async fn delete_memory(
        &self,
        memory_id: &str,
    ) -> Result<Option<MemoryRecord>, RepositoryError> {
        self.storage
            .delete_memory_record(memory_id.to_string())
            .await
            .map_err(map_storage_error)
    }

    async fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> Result<Vec<MemoryRecord>, RepositoryError> {
        self.storage
            .list_memory_records(context_key.to_string(), filter.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        self.storage
            .search_memory_episodes_lexical(query.to_string(), filter.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        self.storage
            .search_memory_records_lexical(query.to_string(), filter.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn get_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: &str,
    ) -> Result<Option<EmbeddingRecord>, RepositoryError> {
        self.storage
            .get_memory_embedding(owner_type, owner_id.to_string())
            .await
            .map_err(map_storage_error)
    }

    async fn upsert_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        self.storage
            .upsert_memory_embedding_pending(update)
            .await
            .map_err(map_storage_error)
    }

    async fn upsert_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        self.storage
            .upsert_memory_embedding_ready(update)
            .await
            .map_err(map_storage_error)
    }

    async fn upsert_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> Result<EmbeddingRecord, RepositoryError> {
        self.storage
            .upsert_memory_embedding_failure(update)
            .await
            .map_err(map_storage_error)
    }

    async fn list_episode_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<EpisodeEmbeddingCandidate>, RepositoryError> {
        self.storage
            .list_memory_episode_embedding_backfill_candidates(request.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn list_memory_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> Result<Vec<MemoryEmbeddingCandidate>, RepositoryError> {
        self.storage
            .list_memory_record_embedding_backfill_candidates(request.clone())
            .await
            .map_err(map_storage_error)
    }

    async fn search_episodes_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        self.storage
            .search_memory_episodes_vector(
                query_embedding.to_vec(),
                model_id.to_string(),
                filter.clone(),
            )
            .await
            .map_err(map_storage_error)
    }

    async fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        model_id: &str,
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        self.storage
            .search_memory_records_vector(
                query_embedding.to_vec(),
                model_id.to_string(),
                filter.clone(),
            )
            .await
            .map_err(map_storage_error)
    }

    fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> impl Future<Output = Result<SessionStateRecord, RepositoryError>> + Send {
        let storage = Arc::clone(&self.storage);
        async move {
            storage
                .upsert_memory_session_state(record)
                .await
                .map_err(map_storage_error)
        }
    }

    async fn get_session_state(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionStateRecord>, RepositoryError> {
        self.storage
            .get_memory_session_state(session_id.to_string())
            .await
            .map_err(map_storage_error)
    }

    async fn list_session_states(
        &self,
        filter: &SessionStateListFilter,
    ) -> Result<Vec<SessionStateRecord>, RepositoryError> {
        self.storage
            .list_memory_session_states(filter.clone())
            .await
            .map_err(map_storage_error)
    }
}

fn map_storage_error(error: crate::storage::StorageError) -> RepositoryError {
    RepositoryError::Storage(error.to_string())
}
