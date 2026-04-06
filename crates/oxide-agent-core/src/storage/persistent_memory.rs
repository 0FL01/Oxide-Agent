use super::StorageProvider;
use oxide_agent_memory::{
    EpisodeId, EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit,
    MemoryListFilter, MemoryRecord, MemoryRepository, MemorySearchFilter, MemorySearchHit,
    RepositoryError, SessionStateRecord, ThreadId, ThreadRecord,
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
        _thread_id: &ThreadId,
    ) -> Result<Option<ThreadRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "get_thread is not implemented for storage-backed memory repository".to_string(),
        ))
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
        _episode_id: &EpisodeId,
    ) -> Result<Option<EpisodeRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "get_episode is not implemented for storage-backed memory repository".to_string(),
        ))
    }

    async fn list_episodes_for_thread(
        &self,
        _thread_id: &ThreadId,
        _filter: &EpisodeListFilter,
    ) -> Result<Vec<EpisodeRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "list_episodes_for_thread is not implemented for storage-backed memory repository"
                .to_string(),
        ))
    }

    async fn create_memory(&self, record: MemoryRecord) -> Result<MemoryRecord, RepositoryError> {
        self.storage
            .create_memory_record(record)
            .await
            .map_err(map_storage_error)
    }

    async fn get_memory(&self, _memory_id: &str) -> Result<Option<MemoryRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "get_memory is not implemented for storage-backed memory repository".to_string(),
        ))
    }

    async fn list_memories(
        &self,
        _context_key: &str,
        _filter: &MemoryListFilter,
    ) -> Result<Vec<MemoryRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "list_memories is not implemented for storage-backed memory repository".to_string(),
        ))
    }

    async fn search_episodes_lexical(
        &self,
        _query: &str,
        _filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        Err(RepositoryError::Storage(
            "search_episodes_lexical is not implemented for storage-backed memory repository"
                .to_string(),
        ))
    }

    async fn search_memories_lexical(
        &self,
        _query: &str,
        _filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        Err(RepositoryError::Storage(
            "search_memories_lexical is not implemented for storage-backed memory repository"
                .to_string(),
        ))
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
        _session_id: &str,
    ) -> Result<Option<SessionStateRecord>, RepositoryError> {
        Err(RepositoryError::Storage(
            "get_session_state is not implemented for storage-backed memory repository".to_string(),
        ))
    }
}

fn map_storage_error(error: crate::storage::StorageError) -> RepositoryError {
    RepositoryError::Storage(error.to_string())
}
