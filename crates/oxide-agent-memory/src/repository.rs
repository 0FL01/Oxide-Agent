//! Memory repository trait for typed long-term memory persistence.
//!
//! This trait defines the write-path and lexical-retrieval API for the persistent memory
//! subsystem. Concrete implementations may use in-memory stores, Postgres, etc.

use crate::types::{
    EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType, EmbeddingPendingUpdate,
    EmbeddingReadyUpdate, EmbeddingRecord, EpisodeEmbeddingCandidate, EpisodeId, EpisodeListFilter,
    EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit, MemoryEmbeddingCandidate,
    MemoryListFilter, MemoryRecord, MemorySearchFilter, MemorySearchHit, SessionStateListFilter,
    SessionStateRecord, ThreadId, ThreadRecord,
};

/// Error type for memory repository operations.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    /// An unexpected storage error.
    #[error("storage error: {0}")]
    Storage(String),
    /// The requested record was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// A uniqueness constraint was violated.
    #[error("conflict: {0}")]
    Conflict(String),
}

/// Abstraction over typed long-term memory persistence.
///
/// Uses RPITIS (return-position `impl Trait`) so no `async-trait` dependency needed.
#[allow(async_fn_in_trait)]
pub trait MemoryRepository: Send + Sync {
    // -- Thread operations --

    /// Create or update a thread record.
    fn upsert_thread(
        &self,
        record: ThreadRecord,
    ) -> impl std::future::Future<Output = Result<ThreadRecord, RepositoryError>> + Send;

    /// Retrieve a thread by its identifier.
    fn get_thread(
        &self,
        thread_id: &ThreadId,
    ) -> impl std::future::Future<Output = Result<Option<ThreadRecord>, RepositoryError>> + Send;

    // -- Episode operations --

    /// Persist a new episode record.
    ///
    /// Returns the stored record with any server-generated fields populated.
    fn create_episode(
        &self,
        record: EpisodeRecord,
    ) -> impl std::future::Future<Output = Result<EpisodeRecord, RepositoryError>> + Send;

    /// Retrieve a single episode by its identifier.
    fn get_episode(
        &self,
        episode_id: &EpisodeId,
    ) -> impl std::future::Future<Output = Result<Option<EpisodeRecord>, RepositoryError>> + Send;

    /// List episodes belonging to a thread, with optional filtering.
    fn list_episodes_for_thread(
        &self,
        thread_id: &ThreadId,
        filter: &EpisodeListFilter,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeRecord>, RepositoryError>> + Send;

    // -- Memory operations --

    /// Persist a reusable memory record.
    fn create_memory(
        &self,
        record: MemoryRecord,
    ) -> impl std::future::Future<Output = Result<MemoryRecord, RepositoryError>> + Send;

    /// Create or update a reusable memory record.
    fn upsert_memory(
        &self,
        record: MemoryRecord,
    ) -> impl std::future::Future<Output = Result<MemoryRecord, RepositoryError>> + Send;

    /// Retrieve a single memory record by its identifier.
    fn get_memory(
        &self,
        memory_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<MemoryRecord>, RepositoryError>> + Send;

    /// Soft-delete one reusable memory record.
    fn delete_memory(
        &self,
        memory_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<MemoryRecord>, RepositoryError>> + Send;

    /// List reusable memories with optional filtering.
    fn list_memories(
        &self,
        context_key: &str,
        filter: &MemoryListFilter,
    ) -> impl std::future::Future<Output = Result<Vec<MemoryRecord>, RepositoryError>> + Send;

    /// Execute lexical search over episode records.
    fn search_episodes_lexical(
        &self,
        query: &str,
        filter: &EpisodeSearchFilter,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeSearchHit>, RepositoryError>> + Send;

    /// Execute lexical search over reusable memory records.
    fn search_memories_lexical(
        &self,
        query: &str,
        filter: &MemorySearchFilter,
    ) -> impl std::future::Future<Output = Result<Vec<MemorySearchHit>, RepositoryError>> + Send;

    /// Retrieve embedding state for one owner.
    fn get_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<EmbeddingRecord>, RepositoryError>> + Send;

    /// Mark one owner as pending embedding generation.
    fn upsert_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> impl std::future::Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send;

    /// Persist a successful embedding vector.
    fn upsert_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> impl std::future::Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send;

    /// Persist a failed embedding indexing attempt.
    fn upsert_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> impl std::future::Future<Output = Result<EmbeddingRecord, RepositoryError>> + Send;

    /// Discover episodes that still need embeddings or reindexing for one model.
    fn list_episode_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeEmbeddingCandidate>, RepositoryError>> + Send;

    /// Discover memories that still need embeddings or reindexing for one model.
    fn list_memory_embedding_backfill_candidates(
        &self,
        request: &EmbeddingBackfillRequest,
    ) -> impl std::future::Future<Output = Result<Vec<MemoryEmbeddingCandidate>, RepositoryError>> + Send;

    /// Execute vector similarity search over episode records.
    fn search_episodes_vector(
        &self,
        query_embedding: &[f32],
        filter: &EpisodeSearchFilter,
    ) -> impl std::future::Future<Output = Result<Vec<EpisodeSearchHit>, RepositoryError>> + Send;

    /// Execute vector similarity search over reusable memory records.
    fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        filter: &MemorySearchFilter,
    ) -> impl std::future::Future<Output = Result<Vec<MemorySearchHit>, RepositoryError>> + Send;

    // -- Session state operations --

    /// Create or update session state.
    fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> impl std::future::Future<Output = Result<SessionStateRecord, RepositoryError>> + Send;

    /// Retrieve session state by session identifier.
    fn get_session_state(
        &self,
        session_id: &str,
    ) -> impl std::future::Future<Output = Result<Option<SessionStateRecord>, RepositoryError>> + Send;

    /// List session states with optional filtering.
    fn list_session_states(
        &self,
        filter: &SessionStateListFilter,
    ) -> impl std::future::Future<Output = Result<Vec<SessionStateRecord>, RepositoryError>> + Send;
}
