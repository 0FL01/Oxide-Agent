use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use crate::config::ModelInfo;
use crate::llm::{EmbeddingTaskType, LlmClient, LlmError};
use crate::storage::{StorageMemoryRepository, StorageProvider};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use lazy_regex::lazy_regex;
use oxide_agent_memory::{
    stable_memory_content_hash, ArtifactRef, ConsolidationPolicy, ContextConsolidator,
    EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType, EmbeddingPendingUpdate,
    EmbeddingReadyUpdate, EmbeddingRecord, EmbeddingUpdateBase, EpisodeEmbeddingCandidate,
    EpisodeFinalizationInput, EpisodeFinalizer, EpisodeListFilter, EpisodeOutcome, EpisodeRecord,
    EpisodeSearchFilter, EpisodeSearchHit, MemoryEmbeddingCandidate, MemoryListFilter,
    MemoryRecord, MemoryRepository, MemorySearchFilter, MemorySearchHit, MemoryType,
    RepositoryError, SessionStateListFilter, SessionStateRecord, ThreadRecord, TimeRange,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, warn};

const EMBEDDING_BACKFILL_LIMIT: usize = 8;
const HYBRID_RETRIEVAL_CANDIDATE_LIMIT: usize = 8;
const HYBRID_RETRIEVAL_TOP_K: usize = 5;
const HYBRID_RETRIEVAL_MIN_SCORE: f32 = 0.45;
const MEMORY_BEHAVIOR_MAX_DRAFTS: usize = 8;
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
            let repository =
                oxide_agent_memory::pg::PgMemoryRepository::connect_with_max_connections(
                    database_url,
                    max_connections,
                )
                .await
                .map_err(|error| {
                    anyhow::anyhow!("failed to connect Postgres persistent memory: {error}")
                })?;

            if auto_migrate {
                repository.migrate().await.map_err(|error| {
                    anyhow::anyhow!("failed to migrate Postgres persistent memory: {error}")
                })?;
            }

            repository.check_health().await.map_err(|error| {
                anyhow::anyhow!("Postgres persistent memory health check failed: {error}")
            })?;

            // Align PG vector column dimensionality with the configured
            // embedding output dimensionality. Migrations default to
            // VECTOR(768); ALTER TABLE is a no-op when dimensions match.
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

/// Scope-aware policy for topic-native memory behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicMemoryPolicy {
    /// Human-readable label used in advisory cards.
    pub context_label: String,
    /// Whether procedural memories may be extracted from tool activity.
    pub allow_procedure_capture: bool,
    /// Whether failure memories may be extracted from tool activity.
    pub allow_failure_capture: bool,
    /// Whether preference extraction from repeated patterns is allowed.
    pub allow_preference_capture: bool,
    /// Whether a retrieval advisor may suggest durable-memory reads.
    pub allow_manual_read_advice: bool,
    /// Whether history-card guidance should be shown.
    pub allow_history_cards: bool,
}

impl TopicMemoryPolicy {
    #[must_use]
    pub fn from_scope(scope: Option<&AgentMemoryScope>) -> Self {
        let synthetic = scope
            .map(|scope| scope.context_key.starts_with("session:"))
            .unwrap_or(true);
        let context_label = scope
            .map(|scope| {
                if synthetic {
                    "this conversation".to_string()
                } else {
                    format!("topic '{}'", scope.context_key)
                }
            })
            .unwrap_or_else(|| "this conversation".to_string());

        Self {
            context_label,
            allow_procedure_capture: true,
            allow_failure_capture: true,
            allow_preference_capture: !synthetic,
            allow_manual_read_advice: true,
            allow_history_cards: !synthetic,
        }
    }
}

/// Tool-derived reusable-memory draft captured during the live agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDerivedMemoryDraft {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    pub source: String,
    pub reason: String,
    pub tags: Vec<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
struct MemoryBehaviorState {
    drafts: Vec<ToolDerivedMemoryDraft>,
    pattern_counts: HashMap<String, usize>,
    emitted_patterns: HashSet<String>,
}

/// Task-local runtime used by Stage-14 hooks to capture memory behavior signals.
#[derive(Debug, Default)]
pub struct MemoryBehaviorRuntime {
    state: Mutex<MemoryBehaviorState>,
}

impl MemoryBehaviorRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = MemoryBehaviorState::default();
        }
    }

    pub fn record_draft(&self, draft: ToolDerivedMemoryDraft) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.drafts.iter().any(|existing| {
            existing.memory_type == draft.memory_type && existing.content == draft.content
        }) {
            return;
        }
        if state.drafts.len() >= MEMORY_BEHAVIOR_MAX_DRAFTS {
            return;
        }
        state.drafts.push(draft);
    }

    #[must_use]
    pub fn observe_pattern(&self, pattern: &str, threshold: usize) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let count = state.pattern_counts.entry(pattern.to_string()).or_insert(0);
        *count = count.saturating_add(1);
        *count >= threshold && state.emitted_patterns.insert(pattern.to_string())
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<ToolDerivedMemoryDraft> {
        self.state
            .lock()
            .map(|state| state.drafts.clone())
            .unwrap_or_default()
    }
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
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError>;
    /// Execute vector similarity search over reusable memories.
    async fn search_memories_vector(
        &self,
        query_embedding: &[f32],
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
        filter: &EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, RepositoryError> {
        MemoryRepository::search_episodes_vector(self, query_embedding, filter).await
    }

    async fn search_memories_vector(
        &self,
        query_embedding: &[f32],
        filter: &MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, RepositoryError> {
        MemoryRepository::search_memories_vector(self, query_embedding, filter).await
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

#[derive(Debug, Clone, Copy)]
pub enum PersistentRunPhase<'a> {
    Completed { final_answer: &'a str },
    WaitingForUserInput,
}

pub struct PersistentRunContext<'a> {
    pub session_id: &'a str,
    pub task_id: &'a str,
    pub scope: &'a AgentMemoryScope,
    pub task: &'a str,
    pub messages: &'a [AgentMessage],
    pub explicit_remember_intent: bool,
    pub hot_token_estimate: usize,
    pub tool_memory_drafts: Vec<ToolDerivedMemoryDraft>,
    pub phase: PersistentRunPhase<'a>,
}

const DEFAULT_POST_RUN_MEMORY_WRITER_TIMEOUT_SECS: u64 = 8;
const DEFAULT_POST_RUN_MEMORY_WRITER_MAX_ATTEMPTS: usize = 3;
const DEFAULT_POST_RUN_MEMORY_WRITER_INITIAL_BACKOFF_MS: u64 = 1_000;
const DEFAULT_POST_RUN_MEMORY_WRITER_MAX_BACKOFF_MS: u64 = 8_000;
const POST_RUN_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES: usize = 32;
const POST_RUN_MEMORY_WRITER_MAX_MESSAGE_CHARS: usize = 1_200;
const POST_RUN_MEMORY_WRITER_MAX_MEMORIES: usize = 8;
const THREAD_SHORT_SUMMARY_MAX_CHARS: usize = 220;
const MEMORY_TITLE_MAX_CHARS: usize = 96;
const MEMORY_CONTENT_MAX_CHARS: usize = 320;
const MEMORY_SHORT_DESCRIPTION_MAX_CHARS: usize = 160;

#[derive(Debug, Clone)]
pub(crate) struct PostRunMemoryWriterConfig {
    pub model_routes: Vec<ModelInfo>,
    pub timeout_secs: u64,
    pub max_attempts: usize,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl Default for PostRunMemoryWriterConfig {
    fn default() -> Self {
        Self {
            model_routes: Vec::new(),
            timeout_secs: DEFAULT_POST_RUN_MEMORY_WRITER_TIMEOUT_SECS,
            max_attempts: DEFAULT_POST_RUN_MEMORY_WRITER_MAX_ATTEMPTS,
            initial_backoff_ms: DEFAULT_POST_RUN_MEMORY_WRITER_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_POST_RUN_MEMORY_WRITER_MAX_BACKOFF_MS,
        }
    }
}

pub(crate) struct PostRunMemoryWriterInput<'a> {
    pub task_id: &'a str,
    pub scope: &'a AgentMemoryScope,
    pub task: &'a str,
    pub final_answer: &'a str,
    pub messages: &'a [AgentMessage],
    pub explicit_remember_intent: bool,
    pub tools_used: &'a [String],
    pub artifacts: &'a [ArtifactRef],
    pub compaction_summary: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunMemoryWriterResponse {
    pub thread_short_summary: Option<String>,
    pub episode: PostRunEpisodeDraft,
    #[serde(default)]
    pub memories: Vec<PostRunMemoryDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunEpisodeDraft {
    pub summary: String,
    pub outcome: EpisodeOutcome,
    #[serde(default)]
    pub failures: Vec<String>,
    pub importance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct PostRunMemoryDraft {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub short_description: String,
    pub importance: f32,
    pub confidence: f32,
    #[serde(default)]
    pub tags: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPostRunMemoryWrite {
    thread_short_summary: Option<String>,
    episode: ValidatedPostRunEpisode,
    memories: Vec<MemoryRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ValidatedPostRunEpisode {
    summary: String,
    outcome: EpisodeOutcome,
    failures: Vec<String>,
    importance: f32,
}

#[async_trait]
pub(crate) trait PostRunMemoryWriter: Send + Sync {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> Result<ValidatedPostRunMemoryWrite>;
}

#[derive(Clone)]
pub(crate) struct LlmPostRunMemoryWriter {
    llm_client: Arc<LlmClient>,
    config: PostRunMemoryWriterConfig,
}

impl LlmPostRunMemoryWriter {
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>, config: PostRunMemoryWriterConfig) -> Self {
        Self { llm_client, config }
    }

    async fn call_llm(&self, user_message: &str, route: &ModelInfo) -> Result<String, LlmError> {
        let max_attempts = self.config.max_attempts.max(1);
        for attempt in 1..=max_attempts {
            let llm_call = self.llm_client.chat_completion_for_model_info(
                post_run_memory_writer_system_prompt(),
                &[],
                user_message,
                route,
            );

            match timeout(Duration::from_secs(self.config.timeout_secs), llm_call).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) => {
                    let Some(backoff) = self.retry_backoff_for_error(&error, attempt) else {
                        return Err(error);
                    };
                    warn!(
                        model = %route.id,
                        provider = %route.provider,
                        attempt,
                        max_attempts,
                        backoff_ms = backoff.as_millis(),
                        error = %error,
                        "PostRun memory writer attempt failed, retrying route"
                    );
                    sleep(backoff).await;
                }
                Err(_) => {
                    let Some(backoff) = self.retry_backoff_for_timeout(attempt) else {
                        return Err(LlmError::Unknown(format!(
                            "PostRun memory writer timed out after {}s",
                            self.config.timeout_secs
                        )));
                    };
                    warn!(
                        model = %route.id,
                        provider = %route.provider,
                        attempt,
                        max_attempts,
                        timeout_secs = self.config.timeout_secs,
                        backoff_ms = backoff.as_millis(),
                        "PostRun memory writer attempt timed out, retrying route"
                    );
                    sleep(backoff).await;
                }
            }
        }

        Err(LlmError::ApiError(
            "PostRun memory writer retry attempts exhausted".to_string(),
        ))
    }

    fn retry_backoff_for_error(&self, error: &LlmError, attempt: usize) -> Option<Duration> {
        if attempt >= self.config.max_attempts.max(1) || !LlmClient::is_retryable_error(error) {
            return None;
        }

        match error {
            LlmError::RateLimit {
                wait_secs: Some(wait_secs),
                ..
            } => {
                let wait_with_buffer = wait_secs.saturating_add(1);
                let backoff = Duration::from_secs(wait_with_buffer);
                (backoff <= self.max_backoff()).then_some(backoff)
            }
            _ => Some(self.exponential_backoff(attempt)),
        }
    }

    fn retry_backoff_for_timeout(&self, attempt: usize) -> Option<Duration> {
        (attempt < self.config.max_attempts.max(1)).then(|| self.exponential_backoff(attempt))
    }

    fn exponential_backoff(&self, attempt: usize) -> Duration {
        let exponent = (attempt.saturating_sub(1)).min(31) as u32;
        let multiplier = 2u64.pow(exponent);
        let backoff_ms = self
            .config
            .initial_backoff_ms
            .saturating_mul(multiplier)
            .min(self.config.max_backoff_ms);
        Duration::from_millis(backoff_ms)
    }

    fn max_backoff(&self) -> Duration {
        Duration::from_millis(self.config.max_backoff_ms)
    }
}

#[async_trait]
impl PostRunMemoryWriter for LlmPostRunMemoryWriter {
    async fn write(
        &self,
        input: &PostRunMemoryWriterInput<'_>,
    ) -> Result<ValidatedPostRunMemoryWrite> {
        let user_message = build_post_run_memory_writer_user_message(input);
        let mut last_error = None;

        for route in self
            .config
            .model_routes
            .iter()
            .filter(|route| !route.id.trim().is_empty() && !route.provider.trim().is_empty())
        {
            if !self.llm_client.is_provider_available(&route.provider) {
                continue;
            }

            match self.call_llm(&user_message, route).await {
                Ok(response) => match parse_post_run_memory_writer_response(&response) {
                    Some(parsed) => {
                        return validate_post_run_memory_writer_response(input, parsed);
                    }
                    None => {
                        last_error = Some(anyhow::anyhow!(
                            "PostRun memory writer returned invalid JSON"
                        ));
                    }
                },
                Err(error) => {
                    last_error = Some(anyhow::anyhow!(error.to_string()));
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| anyhow::anyhow!("No available PostRun memory writer routes")))
    }
}

fn post_run_memory_writer_system_prompt() -> &'static str {
    r#"You write durable memory records for an autonomous software agent.

Return JSON only. No markdown. No prose outside JSON.

Your job:
1. Produce one compact episode summary for the completed task.
2. Produce only reusable long-term memories worth retrieving later.
3. Ignore transient noise, raw tool payloads, progress chatter, and duplicated facts.

Rules:
- Only write memories that would still matter in a future session.
- Prefer project facts, user preferences, procedures, decisions, and constraints.
- If `explicit_remember_intent` is true, preserve the explicitly requested durable information when it is grounded in the transcript and reusable later.
- Do not invent facts not grounded in the provided conversation.
- Keep summaries concise and specific.
- `importance` and `confidence` must be floats between 0.0 and 1.0.
- `outcome` must be one of: success, partial, failure, cancelled.
- `memory_type` must be one of: fact, preference, procedure, decision, constraint.
- Output at most 8 memories.

Schema:
{
  "thread_short_summary": "optional short thread summary",
  "episode": {
    "summary": "what happened and what matters",
    "outcome": "success|partial|failure|cancelled",
    "failures": ["notable failures if any"],
    "importance": 0.0
  },
  "memories": [
    {
      "memory_type": "fact|preference|procedure|decision|constraint",
      "title": "short title",
      "content": "full reusable memory text",
      "short_description": "compact preview",
      "importance": 0.0,
      "confidence": 0.0,
      "tags": ["tag"],
      "reason": "why this should be remembered"
    }
  ]
}"#
}

fn build_post_run_memory_writer_user_message(input: &PostRunMemoryWriterInput<'_>) -> String {
    let mut sections = vec![
        format!("Task ID: {}", input.task_id),
        format!("Context key: {}", input.scope.context_key),
        format!(
            "Explicit remember intent: {}",
            if input.explicit_remember_intent {
                "true"
            } else {
                "false"
            }
        ),
        format!("User task:\n{}", input.task.trim()),
        format!("Final answer:\n{}", input.final_answer.trim()),
    ];

    if let Some(summary) = input
        .compaction_summary
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sections.push(format!("Compaction summary:\n{summary}"));
    }

    if !input.tools_used.is_empty() {
        sections.push(format!("Tools used:\n- {}", input.tools_used.join("\n- ")));
    }

    if !input.artifacts.is_empty() {
        let artifacts = input
            .artifacts
            .iter()
            .map(|artifact| {
                format!(
                    "- {} | {} | {}",
                    artifact.storage_key,
                    artifact.description,
                    artifact.content_type.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("Artifacts:\n{artifacts}"));
    }

    let transcript = input
        .messages
        .iter()
        .rev()
        .take(POST_RUN_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(render_post_run_message)
        .collect::<Vec<_>>()
        .join("\n\n");
    sections.push(format!("Transcript excerpt:\n{transcript}"));
    sections.join("\n\n")
}

fn render_post_run_message(message: &AgentMessage) -> String {
    let role = match message.role {
        crate::agent::memory::MessageRole::System => "system",
        crate::agent::memory::MessageRole::User => "user",
        crate::agent::memory::MessageRole::Assistant => "assistant",
        crate::agent::memory::MessageRole::Tool => "tool",
    };
    let kind = format!("{:?}", message.resolved_kind());
    let content = truncate_chars(
        message.content.trim(),
        POST_RUN_MEMORY_WRITER_MAX_MESSAGE_CHARS,
    );
    if let Some(tool_name) = message.tool_name.as_deref() {
        format!("[{role}/{kind}/{tool_name}]\n{content}")
    } else {
        format!("[{role}/{kind}]\n{content}")
    }
}

fn parse_post_run_memory_writer_response(response: &str) -> Option<PostRunMemoryWriterResponse> {
    serde_json::from_str(extract_json_payload(response)).ok()
}

fn extract_json_payload(response: &str) -> &str {
    lazy_regex!(r"(?s)```(?:json)?\s*(\{.*\})\s*```")
        .captures(response)
        .and_then(|captures| captures.get(1))
        .map_or_else(|| response.trim(), |json| json.as_str().trim())
}

fn validate_post_run_memory_writer_response(
    input: &PostRunMemoryWriterInput<'_>,
    parsed: PostRunMemoryWriterResponse,
) -> Result<ValidatedPostRunMemoryWrite> {
    let now = Utc::now();
    let episode_summary = truncate_chars(parsed.episode.summary.trim(), 2_000);
    if episode_summary.is_empty() {
        return Err(anyhow::anyhow!(
            "PostRun memory writer produced empty episode summary"
        ));
    }

    let failures = parsed
        .episode
        .failures
        .into_iter()
        .map(|item| truncate_chars(item.trim(), 240))
        .filter(|item| !item.is_empty())
        .take(6)
        .collect::<Vec<_>>();

    let mut seen_hashes = HashSet::new();
    let mut memories = Vec::new();
    for draft in parsed
        .memories
        .into_iter()
        .take(POST_RUN_MEMORY_WRITER_MAX_MEMORIES)
    {
        let content = truncate_chars(draft.content.trim(), MEMORY_CONTENT_MAX_CHARS);
        if content.is_empty() {
            continue;
        }
        let content_hash = stable_memory_content_hash(draft.memory_type, &content);
        if !seen_hashes.insert(content_hash.clone()) {
            continue;
        }

        let mut tags = draft
            .tags
            .into_iter()
            .map(|tag| truncate_chars(tag.trim(), 32))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.push("llm_post_run".to_string());
        tags.push(memory_type_label(draft.memory_type).to_string());
        tags.sort();
        tags.dedup();

        memories.push(MemoryRecord {
            memory_id: format!(
                "llm-post-run:{}:{}:{}",
                input.task_id,
                memory_type_label(draft.memory_type),
                &content_hash[..12.min(content_hash.len())]
            ),
            context_key: input.scope.context_key.clone(),
            source_episode_id: Some(input.task_id.to_string()),
            memory_type: draft.memory_type,
            title: truncate_chars(draft.title.trim(), MEMORY_TITLE_MAX_CHARS),
            content,
            short_description: truncate_chars(
                draft.short_description.trim(),
                MEMORY_SHORT_DESCRIPTION_MAX_CHARS,
            ),
            importance: draft.importance.clamp(0.0, 1.0),
            confidence: draft.confidence.clamp(0.0, 1.0),
            source: Some("llm_post_run_writer".to_string()),
            content_hash: Some(content_hash),
            reason: Some(truncate_chars(draft.reason.trim(), 240)),
            tags,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        });
    }

    Ok(ValidatedPostRunMemoryWrite {
        thread_short_summary: parsed
            .thread_short_summary
            .map(|value| truncate_chars(value.trim(), THREAD_SHORT_SUMMARY_MAX_CHARS))
            .filter(|value| !value.is_empty()),
        episode: ValidatedPostRunEpisode {
            summary: episode_summary,
            outcome: parsed.episode.outcome,
            failures,
            importance: parsed.episode.importance.clamp(0.0, 1.0),
        },
        memories,
    })
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[derive(Clone)]
pub struct PersistentMemoryCoordinator {
    store: Arc<dyn PersistentMemoryStore>,
    finalizer: EpisodeFinalizer,
    consolidator: ContextConsolidator,
    embedding_indexer: Option<PersistentMemoryEmbeddingIndexer>,
    memory_writer: Option<Arc<dyn PostRunMemoryWriter>>,
}

impl PersistentMemoryCoordinator {
    #[must_use]
    pub fn new(store: Arc<dyn PersistentMemoryStore>) -> Self {
        Self {
            store,
            finalizer: EpisodeFinalizer,
            consolidator: ContextConsolidator::new(ConsolidationPolicy::default()),
            embedding_indexer: None,
            memory_writer: None,
        }
    }

    #[must_use]
    pub fn with_embedding_indexer(
        mut self,
        embedding_indexer: PersistentMemoryEmbeddingIndexer,
    ) -> Self {
        self.embedding_indexer = Some(embedding_indexer);
        self
    }

    #[must_use]
    pub(crate) fn with_memory_writer(
        mut self,
        memory_writer: Arc<dyn PostRunMemoryWriter>,
    ) -> Self {
        self.memory_writer = Some(memory_writer);
        self
    }

    pub async fn persist_post_run(&self, ctx: PersistentRunContext<'_>) -> Result<()> {
        let summary_signal = latest_summary_signal(ctx.messages);
        let artifacts = collect_artifacts(ctx.messages);
        let tools_used = collect_tools_used(ctx.messages);
        let final_answer = match ctx.phase {
            PersistentRunPhase::Completed { final_answer } => Some(final_answer.to_string()),
            PersistentRunPhase::WaitingForUserInput => None,
        };

        let llm_write = if let Some(final_answer) = final_answer.as_deref() {
            let Some(memory_writer) = self.memory_writer.as_ref() else {
                return Err(anyhow::anyhow!(
                    "PostRun memory writer is required for completed durable memory writes"
                ));
            };
            Some(
                memory_writer
                    .write(&PostRunMemoryWriterInput {
                        task_id: ctx.task_id,
                        scope: ctx.scope,
                        task: ctx.task,
                        final_answer,
                        messages: ctx.messages,
                        explicit_remember_intent: ctx.explicit_remember_intent,
                        tools_used: &tools_used,
                        artifacts: &artifacts,
                        compaction_summary: summary_signal
                            .as_ref()
                            .map(|signal| signal.summary_text.as_str()),
                    })
                    .await?,
            )
        } else {
            None
        };

        let plan = self.finalizer.build_plan(EpisodeFinalizationInput {
            user_id: ctx.scope.user_id,
            context_key: ctx.scope.context_key.clone(),
            flow_id: ctx.scope.flow_id.clone(),
            session_id: ctx.session_id.to_string(),
            episode_id: ctx.task_id.to_string(),
            goal: ctx.task.to_string(),
            thread_short_summary: llm_write
                .as_ref()
                .and_then(|write| write.thread_short_summary.clone()),
            episode_summary: llm_write
                .as_ref()
                .map(|write| write.episode.summary.clone()),
            episode_outcome: llm_write.as_ref().map(|write| write.episode.outcome),
            episode_importance: llm_write.as_ref().map(|write| write.episode.importance),
            final_answer,
            compaction_summary: summary_signal
                .as_ref()
                .map(|signal| signal.summary_text.clone()),
            tools_used,
            artifacts,
            failures: llm_write
                .as_ref()
                .map(|write| write.episode.failures.clone())
                .unwrap_or_else(|| {
                    summary_signal
                        .as_ref()
                        .map_or_else(Vec::new, |signal| signal.failures.clone())
                }),
            hot_token_estimate: ctx.hot_token_estimate,
            finalized_at: Utc::now(),
        });

        self.store.upsert_thread(plan.thread).await?;
        let episode = if let Some(episode) = plan.episode {
            Some(self.store.create_episode(episode).await?)
        } else {
            None
        };
        if let Some(episode) = episode.as_ref() {
            info!(
                episode_id = %episode.episode_id,
                context_key = %episode.context_key,
                outcome = outcome_label(episode.outcome),
                artifact_count = episode.artifacts.len(),
                tool_count = episode.tools_used.len(),
                "Persistent episode finalized"
            );
        }
        if let (Some(indexer), Some(episode)) = (self.embedding_indexer.as_ref(), episode.as_ref())
        {
            if let Err(error) = indexer.index_episode(episode).await {
                warn!(error = %error, episode_id = %episode.episode_id, "episode embedding write failed");
            }
        }
        if let (Some(episode), Some(llm_write)) = (episode.as_ref(), llm_write.as_ref()) {
            self.persist_llm_memories(episode, &llm_write.memories)
                .await;
        }
        if let Some(indexer) = self.embedding_indexer.as_ref() {
            if let Err(error) = indexer.backfill().await {
                warn!(error = %error, "persistent memory embedding backfill failed");
            }
        }
        self.store.upsert_session_state(plan.session_state).await?;
        self.run_context_maintenance(&ctx.scope.context_key, Utc::now())
            .await;
        self.run_watchdog_pass(Utc::now()).await;
        Ok(())
    }

    async fn persist_llm_memories(
        &self,
        episode: &oxide_agent_memory::EpisodeRecord,
        memories: &[MemoryRecord],
    ) {
        let mut fact_writes = 0usize;
        let mut preference_writes = 0usize;
        let mut procedure_writes = 0usize;
        let mut decision_writes = 0usize;
        let mut constraint_writes = 0usize;
        let mut failed_writes = 0usize;
        let mut stored_memory_ids = Vec::new();

        for memory in memories.iter().cloned() {
            match self.store.upsert_memory(memory).await {
                Ok(memory) => {
                    match memory.memory_type {
                        MemoryType::Fact => fact_writes += 1,
                        MemoryType::Preference => preference_writes += 1,
                        MemoryType::Procedure => procedure_writes += 1,
                        MemoryType::Decision => decision_writes += 1,
                        MemoryType::Constraint => constraint_writes += 1,
                    }
                    stored_memory_ids.push(memory.memory_id.clone());
                    info!(
                        memory_write_source = "llm_post_run",
                        context_key = %memory.context_key,
                        episode_id = %episode.episode_id,
                        memory_id = %memory.memory_id,
                        memory_type = memory_type_label(memory.memory_type),
                        "Persistent LLM memory write"
                    );
                    if let Some(indexer) = self.embedding_indexer.as_ref() {
                        if let Err(error) = indexer.index_memory(&memory).await {
                            warn!(error = %error, memory_id = %memory.memory_id, "reusable memory embedding write failed");
                        }
                    }
                }
                Err(error) => {
                    failed_writes += 1;
                    warn!(error = %error, episode_id = %episode.episode_id, "LLM memory write failed");
                }
            }
        }

        if !stored_memory_ids.is_empty() || failed_writes > 0 {
            info!(
                memory_write_source = "llm_post_run",
                episode_id = %episode.episode_id,
                context_key = %episode.context_key,
                stored_memory_count = stored_memory_ids.len(),
                failed_memory_writes = failed_writes,
                fact_writes,
                preference_writes,
                procedure_writes,
                decision_writes,
                constraint_writes,
                stored_memory_ids = ?stored_memory_ids,
                "Post-run memory write telemetry"
            );
        }
    }

    async fn run_context_maintenance(&self, context_key: &str, now: chrono::DateTime<Utc>) {
        let memories = match self
            .store
            .list_memories(
                context_key,
                &MemoryListFilter {
                    include_deleted: true,
                    limit: Some(256),
                    ..MemoryListFilter::default()
                },
            )
            .await
        {
            Ok(memories) => memories,
            Err(error) => {
                warn!(error = %error, context_key, "persistent memory maintenance list failed");
                return;
            }
        };

        let plan = self.consolidator.consolidate(&memories, now);
        if !plan.upserts.is_empty() || !plan.deletions.is_empty() {
            let upserted_memory_ids = plan
                .upserts
                .iter()
                .map(|memory| memory.memory_id.clone())
                .collect::<Vec<_>>();
            info!(
                context_key,
                upsert_count = plan.upserts.len(),
                deletion_count = plan.deletions.len(),
                exact_merge_deletion_count = plan.diagnostics.exact_merge_deletions.len(),
                similarity_merge_deletion_count = plan.diagnostics.similarity_merge_deletions.len(),
                expiration_deletion_count = plan.diagnostics.expired_deletions.len(),
                upserted_memory_ids = ?upserted_memory_ids,
                deleted_memory_ids = ?plan.deletions,
                "Persistent memory consolidation telemetry"
            );
        }
        let oxide_agent_memory::ConsolidatedContext {
            upserts, deletions, ..
        } = plan;
        for memory in upserts {
            match self.store.upsert_memory(memory.clone()).await {
                Ok(memory) => {
                    if let Some(indexer) = self.embedding_indexer.as_ref() {
                        if let Err(error) = indexer.index_memory(&memory).await {
                            warn!(error = %error, memory_id = %memory.memory_id, "persistent memory maintenance reindex failed");
                        }
                    }
                }
                Err(error) => {
                    warn!(error = %error, context_key, "persistent memory maintenance upsert failed");
                }
            }
        }
        for memory_id in deletions {
            if let Err(error) = self.store.delete_memory(&memory_id).await {
                warn!(error = %error, %memory_id, context_key, "persistent memory maintenance delete failed");
            }
        }
    }

    async fn run_watchdog_pass(&self, now: chrono::DateTime<Utc>) {
        let states = match self
            .store
            .list_session_states(&SessionStateListFilter {
                statuses: vec![
                    oxide_agent_memory::CleanupStatus::Idle,
                    oxide_agent_memory::CleanupStatus::Cleaning,
                ],
                limit: Some(32),
                ..SessionStateListFilter::default()
            })
            .await
        {
            Ok(states) => states,
            Err(error) => {
                warn!(error = %error, "persistent memory watchdog list failed");
                return;
            }
        };
        let stale = self.consolidator.stale_sessions(&states, now);
        let mut seen_contexts = HashSet::new();
        for state in stale {
            if seen_contexts.insert(state.context_key.clone()) {
                self.run_context_maintenance(&state.context_key, now).await;
            }
        }
    }
}

#[async_trait]
pub trait MemoryEmbeddingGenerator: Send + Sync {
    async fn embed_document(
        &self,
        text: &str,
        title: Option<&str>,
    ) -> Result<Vec<f32>, anyhow::Error>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, anyhow::Error>;
}

#[derive(Clone)]
pub struct LlmMemoryEmbeddingGenerator {
    llm_client: Arc<LlmClient>,
}

impl LlmMemoryEmbeddingGenerator {
    #[must_use]
    pub fn new(llm_client: Arc<LlmClient>) -> Self {
        Self { llm_client }
    }
}

#[async_trait]
impl MemoryEmbeddingGenerator for LlmMemoryEmbeddingGenerator {
    async fn embed_document(
        &self,
        text: &str,
        title: Option<&str>,
    ) -> Result<Vec<f32>, anyhow::Error> {
        self.llm_client
            .generate_embedding_for_task(text, Some(EmbeddingTaskType::RetrievalDocument), title)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, anyhow::Error> {
        self.llm_client
            .generate_embedding_for_task(text, Some(EmbeddingTaskType::RetrievalQuery), None)
            .await
            .map_err(anyhow::Error::from)
    }
}

#[derive(Clone)]
pub struct PersistentMemoryEmbeddingIndexer {
    store: Arc<dyn PersistentMemoryStore>,
    generator: Arc<dyn MemoryEmbeddingGenerator>,
    model_id: String,
    backfill_limit: usize,
}

impl PersistentMemoryEmbeddingIndexer {
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        generator: Arc<dyn MemoryEmbeddingGenerator>,
        model_id: impl Into<String>,
    ) -> Self {
        let store: Arc<dyn PersistentMemoryStore> = Arc::new(StorageMemoryRepository::new(storage));
        Self::new_with_store(store, generator, model_id)
    }

    #[must_use]
    pub fn new_with_store(
        store: Arc<dyn PersistentMemoryStore>,
        generator: Arc<dyn MemoryEmbeddingGenerator>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            generator,
            model_id: model_id.into(),
            backfill_limit: EMBEDDING_BACKFILL_LIMIT,
        }
    }

    pub async fn index_episode(&self, episode: &EpisodeRecord) -> Result<()> {
        let text = episode_embedding_text(episode);
        let base = EmbeddingUpdateBase {
            owner_id: episode.episode_id.clone(),
            owner_type: EmbeddingOwnerType::Episode,
            model_id: self.model_id.clone(),
            content_hash: embedding_content_hash(&text),
        };
        self.store
            .upsert_embedding_pending(EmbeddingPendingUpdate {
                base: base.clone(),
                requested_at: Utc::now(),
            })
            .await?;
        match self
            .generator
            .embed_document(&text, Some(&episode.goal))
            .await
        {
            Ok(embedding) => {
                self.store
                    .upsert_embedding_ready(EmbeddingReadyUpdate {
                        base,
                        embedding,
                        indexed_at: Utc::now(),
                    })
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.store
                    .upsert_embedding_failure(EmbeddingFailureUpdate {
                        base,
                        error: error.to_string(),
                        failed_at: Utc::now(),
                    })
                    .await?;
                Err(error)
            }
        }
    }

    pub async fn index_memory(&self, memory: &MemoryRecord) -> Result<()> {
        let text = memory_embedding_text(memory);
        let base = EmbeddingUpdateBase {
            owner_id: memory.memory_id.clone(),
            owner_type: EmbeddingOwnerType::Memory,
            model_id: self.model_id.clone(),
            content_hash: embedding_content_hash(&text),
        };
        self.store
            .upsert_embedding_pending(EmbeddingPendingUpdate {
                base: base.clone(),
                requested_at: Utc::now(),
            })
            .await?;
        match self
            .generator
            .embed_document(&text, Some(&memory.title))
            .await
        {
            Ok(embedding) => {
                self.store
                    .upsert_embedding_ready(EmbeddingReadyUpdate {
                        base,
                        embedding,
                        indexed_at: Utc::now(),
                    })
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.store
                    .upsert_embedding_failure(EmbeddingFailureUpdate {
                        base,
                        error: error.to_string(),
                        failed_at: Utc::now(),
                    })
                    .await?;
                Err(error)
            }
        }
    }

    pub async fn backfill(&self) -> Result<()> {
        let request = EmbeddingBackfillRequest {
            model_id: self.model_id.clone(),
            limit: Some(self.backfill_limit),
        };
        let episode_candidates = self
            .store
            .list_episode_embedding_backfill_candidates(&request)
            .await?;
        let memory_candidates = self
            .store
            .list_memory_embedding_backfill_candidates(&request)
            .await?;
        let episode_candidate_count = episode_candidates.len();
        let memory_candidate_count = memory_candidates.len();
        let summarize_candidates = |statuses: Vec<Option<EmbeddingRecord>>| {
            statuses.into_iter().fold(
                (0usize, 0usize, 0usize),
                |(pending, failed, missing), embedding| match embedding
                    .map(|embedding| embedding.status)
                {
                    Some(oxide_agent_memory::EmbeddingStatus::Pending) => {
                        (pending + 1, failed, missing)
                    }
                    Some(oxide_agent_memory::EmbeddingStatus::Failed) => {
                        (pending, failed + 1, missing)
                    }
                    Some(oxide_agent_memory::EmbeddingStatus::Ready) => (pending, failed, missing),
                    None => (pending, failed, missing + 1),
                },
            )
        };
        let (episode_pending_before, episode_failed_before, episode_missing_before) =
            summarize_candidates(
                episode_candidates
                    .iter()
                    .map(|candidate| candidate.embedding.clone())
                    .collect(),
            );
        let (memory_pending_before, memory_failed_before, memory_missing_before) =
            summarize_candidates(
                memory_candidates
                    .iter()
                    .map(|candidate| candidate.embedding.clone())
                    .collect(),
            );

        let mut episode_failures = 0usize;
        let mut memory_failures = 0usize;
        let mut first_error = None;
        for candidate in episode_candidates {
            if let Err(error) = self.index_episode(&candidate.record).await {
                episode_failures += 1;
                warn!(error = %error, episode_id = %candidate.record.episode_id, "persistent memory backfill episode indexing failed");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        for candidate in memory_candidates {
            if let Err(error) = self.index_memory(&candidate.record).await {
                memory_failures += 1;
                warn!(error = %error, memory_id = %candidate.record.memory_id, "persistent memory backfill memory indexing failed");
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }

        info!(
            model_id = %self.model_id,
            episode_candidate_count,
            episode_pending_before,
            episode_failed_before,
            episode_missing_before,
            episode_backfill_failures = episode_failures,
            memory_candidate_count,
            memory_pending_before,
            memory_failed_before,
            memory_missing_before,
            memory_backfill_failures = memory_failures,
            "Persistent memory embedding backfill telemetry"
        );

        if let Some(error) = first_error {
            return Err(error);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DurableMemoryRetrievalOptions {
    pub top_k: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct DurableMemorySearchRequest {
    pub query: String,
    pub search_episodes: bool,
    pub search_memories: bool,
    pub memory_type: Option<MemoryType>,
    pub time_range: TimeRange,
    pub min_importance: Option<f32>,
    pub limit: usize,
    pub candidate_limit: Option<usize>,
    pub allow_full_thread_read: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum DurableMemorySearchItem {
    Episode {
        record: EpisodeRecord,
        snippet: String,
        score: f32,
        lexical_score: Option<f32>,
        vector_score: Option<f32>,
    },
    Memory {
        record: MemoryRecord,
        snippet: String,
        score: f32,
        lexical_score: Option<f32>,
        vector_score: Option<f32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetrievalVectorStatus {
    Disabled,
    Miss,
    Hit,
    EmbeddingFailed,
    SearchFailed,
}

impl RetrievalVectorStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Miss => "miss",
            Self::Hit => "hit",
            Self::EmbeddingFailed => "embedding_failed",
            Self::SearchFailed => "search_failed",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DurableMemoryRetrievalDiagnostics {
    pub query: String,
    pub search_episodes: bool,
    pub search_memories: bool,
    pub candidate_limit: usize,
    pub episode_lexical_hits: usize,
    pub episode_vector_hits: usize,
    pub memory_lexical_hits: usize,
    pub memory_vector_hits: usize,
    pub fused_candidate_count: usize,
    pub injected_item_count: usize,
    pub lexical_only_items: usize,
    pub vector_only_items: usize,
    pub hybrid_items: usize,
    pub filtered_low_score: usize,
    pub filtered_duplicate_snippet: usize,
    pub filtered_covered_episode: usize,
    pub empty_reason: Option<&'static str>,
    pub episode_vector_status: RetrievalVectorStatus,
    pub memory_vector_status: RetrievalVectorStatus,
}

impl DurableMemoryRetrievalDiagnostics {
    fn skipped(query: impl Into<String>, empty_reason: &'static str) -> Self {
        Self {
            query: query.into(),
            search_episodes: false,
            search_memories: false,
            candidate_limit: 0,
            episode_lexical_hits: 0,
            episode_vector_hits: 0,
            memory_lexical_hits: 0,
            memory_vector_hits: 0,
            fused_candidate_count: 0,
            injected_item_count: 0,
            lexical_only_items: 0,
            vector_only_items: 0,
            hybrid_items: 0,
            filtered_low_score: 0,
            filtered_duplicate_snippet: 0,
            filtered_covered_episode: 0,
            empty_reason: Some(empty_reason),
            episode_vector_status: RetrievalVectorStatus::Disabled,
            memory_vector_status: RetrievalVectorStatus::Disabled,
        }
    }

    fn with_plan(plan: &RetrievalPlan, candidate_limit: usize) -> Self {
        Self {
            query: plan.query.clone(),
            search_episodes: plan.search_episodes,
            search_memories: plan.search_memories,
            candidate_limit,
            episode_lexical_hits: 0,
            episode_vector_hits: 0,
            memory_lexical_hits: 0,
            memory_vector_hits: 0,
            fused_candidate_count: 0,
            injected_item_count: 0,
            lexical_only_items: 0,
            vector_only_items: 0,
            hybrid_items: 0,
            filtered_low_score: 0,
            filtered_duplicate_snippet: 0,
            filtered_covered_episode: 0,
            empty_reason: None,
            episode_vector_status: RetrievalVectorStatus::Disabled,
            memory_vector_status: RetrievalVectorStatus::Disabled,
        }
    }

    pub const fn hit(&self) -> bool {
        self.injected_item_count > 0
    }
}

#[derive(Debug, Clone)]
struct DurableMemoryRetrievalOutcome {
    retrieval: Option<DurableMemoryRetrieval>,
    diagnostics: DurableMemoryRetrievalDiagnostics,
}

#[derive(Debug, Clone)]
pub(crate) struct DurableMemorySearchOutcome {
    pub items: Vec<DurableMemorySearchItem>,
    pub diagnostics: DurableMemoryRetrievalDiagnostics,
}

#[derive(Debug, Clone)]
struct VectorSearchOutcome<T> {
    hits: Vec<T>,
    status: RetrievalVectorStatus,
}

#[derive(Clone)]
pub struct DurableMemoryRetriever {
    store: Arc<dyn PersistentMemoryStore>,
    generator: Option<Arc<dyn MemoryEmbeddingGenerator>>,
}

impl DurableMemoryRetriever {
    #[cfg(test)]
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        let store: Arc<dyn PersistentMemoryStore> = Arc::new(StorageMemoryRepository::new(storage));
        Self::new_with_store(store)
    }

    #[must_use]
    pub fn new_with_store(store: Arc<dyn PersistentMemoryStore>) -> Self {
        Self {
            store,
            generator: None,
        }
    }

    #[must_use]
    pub fn with_query_embedding_generator(
        mut self,
        generator: Arc<dyn MemoryEmbeddingGenerator>,
    ) -> Self {
        self.generator = Some(generator);
        self
    }

    fn log_retrieval_telemetry(
        channel: &'static str,
        diagnostics: &DurableMemoryRetrievalDiagnostics,
    ) {
        info!(
            retrieval_channel = channel,
            query = %diagnostics.query,
            retrieval_hit = diagnostics.hit(),
            search_episodes = diagnostics.search_episodes,
            search_memories = diagnostics.search_memories,
            candidate_limit = diagnostics.candidate_limit,
            episode_lexical_hits = diagnostics.episode_lexical_hits,
            episode_vector_hits = diagnostics.episode_vector_hits,
            memory_lexical_hits = diagnostics.memory_lexical_hits,
            memory_vector_hits = diagnostics.memory_vector_hits,
            episode_vector_status = diagnostics.episode_vector_status.as_str(),
            memory_vector_status = diagnostics.memory_vector_status.as_str(),
            fused_candidate_count = diagnostics.fused_candidate_count,
            injected_item_count = diagnostics.injected_item_count,
            lexical_only_items = diagnostics.lexical_only_items,
            vector_only_items = diagnostics.vector_only_items,
            hybrid_items = diagnostics.hybrid_items,
            filtered_low_score = diagnostics.filtered_low_score,
            filtered_duplicate_snippet = diagnostics.filtered_duplicate_snippet,
            filtered_covered_episode = diagnostics.filtered_covered_episode,
            empty_reason = diagnostics.empty_reason.unwrap_or("none"),
            "Durable memory retrieval telemetry"
        );
    }

    pub async fn render_prompt_context(
        &self,
        task: &str,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<String>> {
        let outcome = self.retrieve_outcome_for_task(task, scope, options).await?;
        Self::log_retrieval_telemetry("prompt", &outcome.diagnostics);
        Ok(outcome
            .retrieval
            .map(|retrieval| retrieval.render_for_prompt()))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn search(
        &self,
        scope: &AgentMemoryScope,
        request: DurableMemorySearchRequest,
    ) -> Result<Vec<DurableMemorySearchItem>> {
        Ok(self.search_with_diagnostics(scope, request).await?.items)
    }

    pub(crate) async fn search_with_diagnostics(
        &self,
        scope: &AgentMemoryScope,
        request: DurableMemorySearchRequest,
    ) -> Result<DurableMemorySearchOutcome> {
        let query = request.query.trim();
        if query.is_empty() || (!request.search_episodes && !request.search_memories) {
            let diagnostics = DurableMemoryRetrievalDiagnostics::skipped(
                query,
                if query.is_empty() {
                    "empty_query"
                } else {
                    "no_sources_requested"
                },
            );
            Self::log_retrieval_telemetry("tool", &diagnostics);
            return Ok(DurableMemorySearchOutcome {
                items: Vec::new(),
                diagnostics,
            });
        }

        let plan = RetrievalPlan {
            query: query.to_string(),
            search_episodes: request.search_episodes,
            search_memories: request.search_memories,
            memory_type: request.memory_type,
            min_importance: request.min_importance.unwrap_or(0.0),
            top_k: request.limit.max(1),
            allow_full_thread_read: request.allow_full_thread_read,
        };
        let candidate_limit = request
            .candidate_limit
            .unwrap_or_else(|| request.limit.max(HYBRID_RETRIEVAL_CANDIDATE_LIMIT));

        let outcome = self
            .retrieve_with_plan(scope, plan, request.time_range, candidate_limit)
            .await?;
        Self::log_retrieval_telemetry("tool", &outcome.diagnostics);
        Ok(DurableMemorySearchOutcome {
            items: outcome
                .retrieval
                .map(DurableMemoryRetrieval::into_search_items)
                .unwrap_or_default(),
            diagnostics: outcome.diagnostics,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    async fn retrieve(
        &self,
        task: &str,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<DurableMemoryRetrieval>> {
        Ok(self
            .retrieve_outcome_for_task(task, scope, options)
            .await?
            .retrieval)
    }

    async fn retrieve_outcome_for_task(
        &self,
        task: &str,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<DurableMemoryRetrievalOutcome> {
        let Some(plan) = query_retrieval_plan(task, options) else {
            return Ok(DurableMemoryRetrievalOutcome {
                retrieval: None,
                diagnostics: DurableMemoryRetrievalDiagnostics::skipped(
                    task,
                    "query_filtered_as_smalltalk",
                ),
            });
        };

        self.retrieve_with_plan(
            scope,
            plan,
            TimeRange::default(),
            HYBRID_RETRIEVAL_CANDIDATE_LIMIT,
        )
        .await
    }

    async fn retrieve_with_plan(
        &self,
        scope: &AgentMemoryScope,
        plan: RetrievalPlan,
        time_range: TimeRange,
        candidate_limit: usize,
    ) -> Result<DurableMemoryRetrievalOutcome> {
        let candidate_limit = candidate_limit.max(1);
        let mut diagnostics = DurableMemoryRetrievalDiagnostics::with_plan(&plan, candidate_limit);

        let mut candidates = Vec::new();

        if plan.search_episodes {
            let filter = episode_search_filter(scope, &plan, time_range.clone(), candidate_limit);
            let lexical_hits = self
                .store
                .search_episodes_lexical(&plan.query, &filter)
                .await?;
            diagnostics.episode_lexical_hits = lexical_hits.len();
            let vector_hits = self.search_episode_vectors(&plan, &filter).await;
            diagnostics.episode_vector_status = vector_hits.status;
            diagnostics.episode_vector_hits = vector_hits.hits.len();
            candidates.extend(fuse_episode_hits(lexical_hits, vector_hits.hits));
        }

        if plan.search_memories {
            let filter = memory_search_filter(scope, &plan, time_range, candidate_limit);
            let lexical_hits = self
                .store
                .search_memories_lexical(&plan.query, &filter)
                .await?;
            diagnostics.memory_lexical_hits = lexical_hits.len();
            let vector_hits = self.search_memory_vectors(&plan, &filter).await;
            diagnostics.memory_vector_status = vector_hits.status;
            diagnostics.memory_vector_hits = vector_hits.hits.len();
            candidates.extend(fuse_memory_hits(lexical_hits, vector_hits.hits));
        }

        diagnostics.fused_candidate_count = candidates.len();

        candidates.sort_by(|left, right| {
            right
                .score()
                .total_cmp(&left.score())
                .then_with(|| right.rank_priority().total_cmp(&left.rank_priority()))
                .then_with(|| left.stable_id().cmp(right.stable_id()))
        });

        let mut items = Vec::new();
        let mut covered_episode_ids = HashSet::new();
        let mut seen_snippets = HashSet::new();
        for candidate in candidates {
            if candidate.score() < HYBRID_RETRIEVAL_MIN_SCORE {
                diagnostics.filtered_low_score += 1;
                continue;
            }

            if !seen_snippets.insert(normalized_snippet_key(candidate.snippet())) {
                diagnostics.filtered_duplicate_snippet += 1;
                continue;
            }

            if let Some(source_episode_id) = candidate.source_episode_id() {
                if covered_episode_ids.contains(source_episode_id) {
                    diagnostics.filtered_covered_episode += 1;
                    continue;
                }
            }

            if let Some(episode_id) = candidate.primary_episode_id() {
                covered_episode_ids.insert(episode_id.to_string());
            }

            items.push(candidate);
            if items.len() >= plan.top_k {
                break;
            }
        }

        diagnostics.injected_item_count = items.len();
        for candidate in &items {
            match candidate {
                HybridCandidate::Episode {
                    lexical_score,
                    vector_score,
                    ..
                }
                | HybridCandidate::Memory {
                    lexical_score,
                    vector_score,
                    ..
                } => match (lexical_score.is_some(), vector_score.is_some()) {
                    (true, true) => diagnostics.hybrid_items += 1,
                    (true, false) => diagnostics.lexical_only_items += 1,
                    (false, true) => diagnostics.vector_only_items += 1,
                    (false, false) => {}
                },
            }
        }

        if items.is_empty() {
            diagnostics.empty_reason = Some(if diagnostics.fused_candidate_count == 0 {
                "no_search_hits"
            } else if diagnostics.filtered_low_score == diagnostics.fused_candidate_count {
                "all_candidates_below_score_threshold"
            } else {
                "all_candidates_deduplicated_or_covered"
            });
            return Ok(DurableMemoryRetrievalOutcome {
                retrieval: None,
                diagnostics,
            });
        }

        Ok(DurableMemoryRetrievalOutcome {
            retrieval: Some(DurableMemoryRetrieval { plan, items }),
            diagnostics,
        })
    }

    async fn search_episode_vectors(
        &self,
        plan: &RetrievalPlan,
        filter: &EpisodeSearchFilter,
    ) -> VectorSearchOutcome<EpisodeSearchHit> {
        let Some(generator) = self.generator.as_ref() else {
            return VectorSearchOutcome {
                hits: Vec::new(),
                status: RetrievalVectorStatus::Disabled,
            };
        };

        let query_embedding = match generator.embed_query(&plan.query).await {
            Ok(query_embedding) => query_embedding,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory query embedding failed");
                return VectorSearchOutcome {
                    hits: Vec::new(),
                    status: RetrievalVectorStatus::EmbeddingFailed,
                };
            }
        };

        match self
            .store
            .search_episodes_vector(&query_embedding, filter)
            .await
        {
            Ok(hits) => VectorSearchOutcome {
                status: if hits.is_empty() {
                    RetrievalVectorStatus::Miss
                } else {
                    RetrievalVectorStatus::Hit
                },
                hits,
            },
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory episode vector search failed");
                VectorSearchOutcome {
                    hits: Vec::new(),
                    status: RetrievalVectorStatus::SearchFailed,
                }
            }
        }
    }

    async fn search_memory_vectors(
        &self,
        plan: &RetrievalPlan,
        filter: &MemorySearchFilter,
    ) -> VectorSearchOutcome<MemorySearchHit> {
        let Some(generator) = self.generator.as_ref() else {
            return VectorSearchOutcome {
                hits: Vec::new(),
                status: RetrievalVectorStatus::Disabled,
            };
        };

        let query_embedding = match generator.embed_query(&plan.query).await {
            Ok(query_embedding) => query_embedding,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory query embedding failed");
                return VectorSearchOutcome {
                    hits: Vec::new(),
                    status: RetrievalVectorStatus::EmbeddingFailed,
                };
            }
        };

        match self
            .store
            .search_memories_vector(&query_embedding, filter)
            .await
        {
            Ok(hits) => VectorSearchOutcome {
                status: if hits.is_empty() {
                    RetrievalVectorStatus::Miss
                } else {
                    RetrievalVectorStatus::Hit
                },
                hits,
            },
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory record vector search failed");
                VectorSearchOutcome {
                    hits: Vec::new(),
                    status: RetrievalVectorStatus::SearchFailed,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct RetrievalPlan {
    query: String,
    search_episodes: bool,
    search_memories: bool,
    memory_type: Option<MemoryType>,
    min_importance: f32,
    top_k: usize,
    allow_full_thread_read: bool,
}

#[derive(Debug, Clone)]
struct DurableMemoryRetrieval {
    plan: RetrievalPlan,
    items: Vec<HybridCandidate>,
}

impl DurableMemoryRetrieval {
    fn into_search_items(self) -> Vec<DurableMemorySearchItem> {
        self.items
            .into_iter()
            .map(DurableMemorySearchItem::from)
            .collect()
    }

    fn render_for_prompt(&self) -> String {
        let mut lines = vec![
            "Scoped durable memory context (use as evidence, not source of truth):".to_string(),
            format!("- query: {}", self.plan.query),
            format!(
                "- sources: {}{}{}",
                if self.plan.search_memories {
                    "memories"
                } else {
                    ""
                },
                if self.plan.search_memories && self.plan.search_episodes {
                    ", "
                } else {
                    ""
                },
                if self.plan.search_episodes {
                    "episodes"
                } else {
                    ""
                }
            ),
        ];

        for (index, item) in self.items.iter().enumerate() {
            lines.extend(item.render(index + 1));
        }

        lines.push(
            "Open full thread only if needed via memory_read_episode, memory_read_thread_summary, or memory_read_thread_window."
                .to_string(),
        );
        if self.plan.allow_full_thread_read {
            lines.push(
                "Prefer a single targeted read using the refs below instead of loading full history eagerly."
                    .to_string(),
            );
        }
        lines.join("\n")
    }
}

impl From<HybridCandidate> for DurableMemorySearchItem {
    fn from(value: HybridCandidate) -> Self {
        match value {
            HybridCandidate::Episode {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            } => Self::Episode {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            },
            HybridCandidate::Memory {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            } => Self::Memory {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            },
        }
    }
}

#[derive(Debug, Clone)]
enum HybridCandidate {
    Episode {
        record: EpisodeRecord,
        snippet: String,
        score: f32,
        lexical_score: Option<f32>,
        vector_score: Option<f32>,
    },
    Memory {
        record: MemoryRecord,
        snippet: String,
        score: f32,
        lexical_score: Option<f32>,
        vector_score: Option<f32>,
    },
}

impl HybridCandidate {
    fn stable_id(&self) -> &str {
        match self {
            Self::Episode { record, .. } => &record.episode_id,
            Self::Memory { record, .. } => &record.memory_id,
        }
    }

    fn score(&self) -> f32 {
        match self {
            Self::Episode { score, .. } | Self::Memory { score, .. } => *score,
        }
    }

    fn snippet(&self) -> &str {
        match self {
            Self::Episode { snippet, .. } | Self::Memory { snippet, .. } => snippet,
        }
    }

    fn primary_episode_id(&self) -> Option<&str> {
        match self {
            Self::Episode { record, .. } => Some(&record.episode_id),
            Self::Memory { record, .. } => record.source_episode_id.as_deref(),
        }
    }

    fn source_episode_id(&self) -> Option<&str> {
        match self {
            Self::Episode { .. } => None,
            Self::Memory { record, .. } => record.source_episode_id.as_deref(),
        }
    }

    fn rank_priority(&self) -> f32 {
        match self {
            Self::Episode { record, .. } => record.importance,
            Self::Memory { record, .. } => (record.importance + record.confidence) / 2.0,
        }
    }

    fn render(&self, index: usize) -> Vec<String> {
        match self {
            Self::Episode {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            } => vec![
                format!(
                    "{}. episode {} [score {:.2}] outcome={} importance={:.2}",
                    index,
                    record.episode_id,
                    score,
                    outcome_label(record.outcome),
                    record.importance,
                ),
                format!("   evidence: {}", snippet),
                format!(
                    "   refs: thread_id={} episode_id={} lexical={:.2} vector={:.2}",
                    record.thread_id,
                    record.episode_id,
                    lexical_score.unwrap_or_default(),
                    vector_score.unwrap_or_default(),
                ),
            ],
            Self::Memory {
                record,
                snippet,
                score,
                lexical_score,
                vector_score,
            } => vec![
                format!(
                    "{}. memory {} [score {:.2}] type={} importance={:.2} confidence={:.2}",
                    index,
                    record.memory_id,
                    score,
                    memory_type_label(record.memory_type),
                    record.importance,
                    record.confidence,
                ),
                format!(
                    "   title: {}{}",
                    record.title,
                    if record.short_description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", record.short_description)
                    }
                ),
                format!("   evidence: {}", snippet),
                format!(
                    "   refs: source_episode_id={} lexical={:.2} vector={:.2}",
                    record.source_episode_id.as_deref().unwrap_or("none"),
                    lexical_score.unwrap_or_default(),
                    vector_score.unwrap_or_default(),
                ),
            ],
        }
    }
}

fn query_retrieval_plan(
    task: &str,
    options: DurableMemoryRetrievalOptions,
) -> Option<RetrievalPlan> {
    let query = task.trim();
    if query.is_empty() {
        return None;
    }

    let normalized = query.to_ascii_lowercase();
    let token_count = normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .count();
    let is_smalltalk = token_count <= 6
        && [
            "thanks",
            "thank you",
            "hello",
            "hi",
            "ok",
            "okay",
            "got it",
            "sounds good",
        ]
        .iter()
        .any(|phrase| normalized == *phrase || normalized.starts_with(&format!("{phrase} ")));
    if is_smalltalk {
        return None;
    }

    let has_history_cue = contains_any(
        &normalized,
        &[
            "previous",
            "earlier",
            "before",
            "again",
            "history",
            "thread",
            "episode",
            "what happened",
            "why did",
            "why was",
            "incident",
            "regression",
            "error",
            "issue",
            "debug",
            "resolved",
        ],
    );
    let has_procedure_cue = contains_any(
        &normalized,
        &[
            "how",
            "steps",
            "procedure",
            "workflow",
            "run ",
            "deploy",
            "setup",
            "configure",
            "install",
        ],
    );
    let has_constraint_cue = contains_any(
        &normalized,
        &[
            "constraint",
            "must",
            "never",
            "required",
            "policy",
            "guardrail",
            "forbid",
        ],
    );
    let has_preference_cue = contains_any(
        &normalized,
        &["prefer", "preference", "style", "guideline", "convention"],
    );
    let has_decision_cue = contains_any(&normalized, &["decision", "decided"]);

    let search_episodes =
        has_history_cue || normalized.contains("why") || normalized.contains("debug");
    let search_memories = has_procedure_cue
        || has_constraint_cue
        || has_preference_cue
        || has_decision_cue
        || !search_episodes
        || token_count >= 5;

    let memory_type = if has_procedure_cue {
        Some(MemoryType::Procedure)
    } else if has_constraint_cue {
        Some(MemoryType::Constraint)
    } else if has_preference_cue {
        Some(MemoryType::Preference)
    } else if has_decision_cue {
        Some(MemoryType::Decision)
    } else {
        None
    };

    Some(RetrievalPlan {
        query: query.to_string(),
        search_episodes,
        search_memories,
        memory_type,
        min_importance: if has_history_cue { 0.45 } else { 0.55 },
        top_k: options
            .top_k
            .unwrap_or(HYBRID_RETRIEVAL_TOP_K)
            .clamp(1, HYBRID_RETRIEVAL_TOP_K),
        allow_full_thread_read: has_history_cue
            || normalized.contains("thread")
            || normalized.contains("transcript"),
    })
}

fn episode_search_filter(
    scope: &AgentMemoryScope,
    plan: &RetrievalPlan,
    time_range: TimeRange,
    candidate_limit: usize,
) -> EpisodeSearchFilter {
    EpisodeSearchFilter {
        context_key: Some(scope.context_key.clone()),
        user_id: Some(scope.user_id),
        outcome: None,
        min_importance: Some(plan.min_importance),
        time_range,
        limit: Some(candidate_limit),
    }
}

fn memory_search_filter(
    scope: &AgentMemoryScope,
    plan: &RetrievalPlan,
    time_range: TimeRange,
    candidate_limit: usize,
) -> MemorySearchFilter {
    MemorySearchFilter {
        context_key: Some(scope.context_key.clone()),
        user_id: Some(scope.user_id),
        memory_type: plan.memory_type,
        min_importance: Some(plan.min_importance),
        tags: Vec::new(),
        time_range,
        limit: Some(candidate_limit),
    }
}

fn fuse_episode_hits(
    lexical_hits: Vec<EpisodeSearchHit>,
    vector_hits: Vec<EpisodeSearchHit>,
) -> Vec<HybridCandidate> {
    let lexical = normalize_scores(
        lexical_hits
            .iter()
            .map(|hit| (hit.record.episode_id.clone(), hit.score))
            .collect(),
    );
    let vector = normalize_scores(
        vector_hits
            .iter()
            .map(|hit| (hit.record.episode_id.clone(), hit.score))
            .collect(),
    );

    let mut by_id = HashMap::new();
    for hit in lexical_hits {
        by_id.insert(hit.record.episode_id.clone(), (Some(hit), None));
    }
    for hit in vector_hits {
        by_id
            .entry(hit.record.episode_id.clone())
            .and_modify(
                |entry: &mut (Option<EpisodeSearchHit>, Option<EpisodeSearchHit>)| {
                    entry.1 = Some(hit.clone());
                },
            )
            .or_insert((None, Some(hit)));
    }

    by_id
        .into_iter()
        .filter_map(|(episode_id, (lexical_hit, vector_hit))| {
            let record = lexical_hit
                .as_ref()
                .map(|hit| hit.record.clone())
                .or_else(|| vector_hit.as_ref().map(|hit| hit.record.clone()))?;
            let snippet = lexical_hit
                .as_ref()
                .map(|hit| hit.snippet.clone())
                .or_else(|| vector_hit.as_ref().map(|hit| hit.snippet.clone()))
                .unwrap_or_default();
            let lexical_score = lexical.get(&episode_id).copied();
            let vector_score = vector.get(&episode_id).copied();
            Some(HybridCandidate::Episode {
                score: fused_score(lexical_score, vector_score, record.importance, None),
                record,
                snippet,
                lexical_score,
                vector_score,
            })
        })
        .collect()
}

fn fuse_memory_hits(
    lexical_hits: Vec<MemorySearchHit>,
    vector_hits: Vec<MemorySearchHit>,
) -> Vec<HybridCandidate> {
    let lexical = normalize_scores(
        lexical_hits
            .iter()
            .map(|hit| (hit.record.memory_id.clone(), hit.score))
            .collect(),
    );
    let vector = normalize_scores(
        vector_hits
            .iter()
            .map(|hit| (hit.record.memory_id.clone(), hit.score))
            .collect(),
    );

    let mut by_id = HashMap::new();
    for hit in lexical_hits {
        by_id.insert(hit.record.memory_id.clone(), (Some(hit), None));
    }
    for hit in vector_hits {
        by_id
            .entry(hit.record.memory_id.clone())
            .and_modify(
                |entry: &mut (Option<MemorySearchHit>, Option<MemorySearchHit>)| {
                    entry.1 = Some(hit.clone());
                },
            )
            .or_insert((None, Some(hit)));
    }

    by_id
        .into_iter()
        .filter_map(|(memory_id, (lexical_hit, vector_hit))| {
            let record = lexical_hit
                .as_ref()
                .map(|hit| hit.record.clone())
                .or_else(|| vector_hit.as_ref().map(|hit| hit.record.clone()))?;
            let snippet = lexical_hit
                .as_ref()
                .map(|hit| hit.snippet.clone())
                .or_else(|| vector_hit.as_ref().map(|hit| hit.snippet.clone()))
                .unwrap_or_default();
            let lexical_score = lexical.get(&memory_id).copied();
            let vector_score = vector.get(&memory_id).copied();
            Some(HybridCandidate::Memory {
                score: fused_score(
                    lexical_score,
                    vector_score,
                    record.importance,
                    Some(record.confidence),
                ),
                record,
                snippet,
                lexical_score,
                vector_score,
            })
        })
        .collect()
}

fn normalize_scores(scores: Vec<(String, f32)>) -> HashMap<String, f32> {
    if scores.is_empty() {
        return HashMap::new();
    }

    let (min_score, max_score) = scores.iter().fold(
        (f32::INFINITY, f32::NEG_INFINITY),
        |(min_score, max_score), (_, score)| (min_score.min(*score), max_score.max(*score)),
    );

    scores
        .into_iter()
        .map(|(id, score)| {
            let normalized = if (max_score - min_score).abs() < f32::EPSILON {
                if score > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                (score - min_score) / (max_score - min_score)
            };
            (id, normalized.clamp(0.0, 1.0))
        })
        .collect()
}

fn fused_score(
    lexical_score: Option<f32>,
    vector_score: Option<f32>,
    importance: f32,
    confidence: Option<f32>,
) -> f32 {
    let lexical = lexical_score.unwrap_or_default() * 0.45;
    let vector = vector_score.unwrap_or_default() * 0.45;
    let importance = importance.clamp(0.0, 1.0) * 0.07;
    let confidence = confidence.unwrap_or(1.0).clamp(0.0, 1.0) * 0.03;
    lexical + vector + importance + confidence
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn normalized_snippet_key(snippet: &str) -> String {
    snippet
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn outcome_label(outcome: EpisodeOutcome) -> &'static str {
    match outcome {
        EpisodeOutcome::Success => "success",
        EpisodeOutcome::Failure => "failure",
        EpisodeOutcome::Partial => "partial",
        EpisodeOutcome::Cancelled => "cancelled",
    }
}

fn memory_type_label(memory_type: MemoryType) -> &'static str {
    match memory_type {
        MemoryType::Fact => "fact",
        MemoryType::Preference => "preference",
        MemoryType::Procedure => "procedure",
        MemoryType::Decision => "decision",
        MemoryType::Constraint => "constraint",
    }
}

fn embedding_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn episode_embedding_text(episode: &EpisodeRecord) -> String {
    format!(
        "goal: {}\nsummary: {}\ntools: {}\nfailures: {}",
        episode.goal,
        episode.summary,
        episode.tools_used.join(", "),
        episode.failures.join(" | ")
    )
}

fn memory_embedding_text(memory: &MemoryRecord) -> String {
    format!(
        "title: {}\ndescription: {}\ncontent: {}\nsource: {}\nreason: {}\ntags: {}",
        memory.title,
        memory.short_description,
        memory.content,
        memory.source.clone().unwrap_or_default(),
        memory.reason.clone().unwrap_or_default(),
        memory.tags.join(", ")
    )
}

#[derive(Debug, Clone)]
struct PersistentSummarySignal {
    summary_text: String,
    failures: Vec<String>,
}

fn latest_summary_signal(messages: &[AgentMessage]) -> Option<PersistentSummarySignal> {
    let mut latest_summary = None;

    for message in messages.iter().rev() {
        let Some(summary) = message.summary_payload() else {
            continue;
        };

        latest_summary = Some(PersistentSummarySignal {
            summary_text: message.content.trim().to_string(),
            failures: summary
                .risks
                .iter()
                .map(|risk| risk.trim())
                .filter(|risk| !risk.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        });
        break;
    }

    latest_summary
}

fn collect_artifacts(messages: &[AgentMessage]) -> Vec<ArtifactRef> {
    let mut seen = HashSet::new();
    let mut artifacts = Vec::new();

    for message in messages {
        if let Some(archive_ref) = message.archive_ref_payload() {
            push_artifact(
                &mut artifacts,
                &mut seen,
                archive_ref.storage_key.clone(),
                archive_ref.title.clone(),
                Some("application/json".to_string()),
                archive_ref.created_at,
            );
        }
        if let Some(payload) = &message.externalized_payload {
            push_artifact(
                &mut artifacts,
                &mut seen,
                payload.archive_ref.storage_key.clone(),
                payload.archive_ref.title.clone(),
                Some("text/plain".to_string()),
                payload.archive_ref.created_at,
            );
        }
        if let Some(artifact) = &message.pruned_artifact {
            if let Some(archive_ref) = &artifact.archive_ref {
                push_artifact(
                    &mut artifacts,
                    &mut seen,
                    archive_ref.storage_key.clone(),
                    archive_ref.title.clone(),
                    Some("text/plain".to_string()),
                    archive_ref.created_at,
                );
            }
        }
    }

    artifacts
}

fn push_artifact(
    artifacts: &mut Vec<ArtifactRef>,
    seen: &mut HashSet<String>,
    storage_key: String,
    description: String,
    content_type: Option<String>,
    created_at: i64,
) {
    if !seen.insert(storage_key.clone()) {
        return;
    }

    let Some(created_at) = chrono::DateTime::<Utc>::from_timestamp(created_at, 0) else {
        return;
    };

    artifacts.push(ArtifactRef {
        storage_key,
        description,
        content_type,
        source: Some("post_run_extract".to_string()),
        reason: None,
        tags: vec!["archive".to_string()],
        created_at,
    });
}

fn collect_tools_used(messages: &[AgentMessage]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut tools = Vec::new();

    for message in messages {
        let Some(tool_name) = message.tool_name.as_deref() else {
            continue;
        };
        let tool_name = tool_name.trim();
        if tool_name.is_empty() || !seen.insert(tool_name.to_string()) {
            continue;
        }
        tools.push(tool_name.to_string());
    }

    tools
}

#[cfg(test)]
mod tests {
    use super::{
        DurableMemoryRetrievalOptions, DurableMemoryRetriever, DurableMemorySearchItem,
        DurableMemorySearchRequest, HybridCandidate, MemoryEmbeddingGenerator,
        PersistentMemoryCoordinator, PersistentMemoryEmbeddingIndexer, PersistentMemoryStore,
        PersistentRunContext, PersistentRunPhase, PostRunMemoryWriter, PostRunMemoryWriterInput,
        ValidatedPostRunEpisode, ValidatedPostRunMemoryWrite,
    };
    use crate::agent::compaction::ArchiveRef;
    use crate::agent::memory::AgentMessage;
    use crate::agent::session::AgentMemoryScope;
    use crate::storage::MockStorageProvider;
    use chrono::TimeZone;
    use oxide_agent_memory::{
        CleanupStatus, EmbeddingPendingUpdate, EmbeddingReadyUpdate, EpisodeEmbeddingCandidate,
        EpisodeOutcome, EpisodeRecord, EpisodeSearchHit, InMemoryMemoryRepository,
        MemoryListFilter, MemoryRecord, MemoryRepository, MemorySearchHit, MemoryType,
        SessionStateRecord,
    };
    use std::sync::Arc;

    struct FakeEmbeddingGenerator;

    struct FakePostRunMemoryWriter;

    #[async_trait::async_trait]
    impl PostRunMemoryWriter for FakePostRunMemoryWriter {
        async fn write(
            &self,
            input: &PostRunMemoryWriterInput<'_>,
        ) -> anyhow::Result<ValidatedPostRunMemoryWrite> {
            let context_label = input.scope.context_key.replace('-', " ");
            let now = chrono::Utc::now();
            let memory_specs = if input.task.contains("hygiene") {
                vec![
                    (
                        MemoryType::Decision,
                        "Memory hygiene decision".to_string(),
                        "Use cargo check before build".to_string(),
                    ),
                    (
                        MemoryType::Constraint,
                        "Memory hygiene constraint".to_string(),
                        "Keep duplicate durable memories merged by content hash".to_string(),
                    ),
                    (
                        MemoryType::Fact,
                        "Memory hygiene fact".to_string(),
                        "Persistent memory consolidation marks superseded duplicates as deleted"
                            .to_string(),
                    ),
                ]
            } else {
                vec![
                    (
                        MemoryType::Decision,
                        format!("{} decision", context_label),
                        format!(
                            "Use durable memory isolation for {}",
                            input.scope.context_key
                        ),
                    ),
                    (
                        MemoryType::Constraint,
                        format!("{} constraint", context_label),
                        format!(
                            "{} durable memory records must stay isolated",
                            input.scope.context_key
                        ),
                    ),
                    (
                        MemoryType::Fact,
                        format!("{} fact", context_label),
                        format!(
                            "{} records are stored under their own context key",
                            input.scope.context_key
                        ),
                    ),
                ]
            };

            let memories = memory_specs
                .into_iter()
                .map(|(memory_type, title, content)| {
                    let content_hash =
                        oxide_agent_memory::stable_memory_content_hash(memory_type, &content);
                    let mut tags = vec!["llm_post_run".to_string()];
                    if input.explicit_remember_intent {
                        tags.push("explicit_remember".to_string());
                    }
                    MemoryRecord {
                        memory_id: format!(
                            "fake:{}:{}:{}",
                            input.task_id,
                            super::memory_type_label(memory_type),
                            &content_hash[..12.min(content_hash.len())]
                        ),
                        context_key: input.scope.context_key.clone(),
                        source_episode_id: Some(input.task_id.to_string()),
                        memory_type,
                        title,
                        short_description: content.clone(),
                        content,
                        importance: 0.9,
                        confidence: 0.95,
                        source: Some("fake_post_run_writer".to_string()),
                        content_hash: Some(content_hash),
                        reason: Some("test llm writer".to_string()),
                        tags,
                        created_at: now,
                        updated_at: now,
                        deleted_at: None,
                    }
                })
                .collect();

            Ok(ValidatedPostRunMemoryWrite {
                thread_short_summary: Some(format!("{} summary", input.task)),
                episode: ValidatedPostRunEpisode {
                    summary: format!("Completed task: {}", input.task),
                    outcome: EpisodeOutcome::Success,
                    failures: Vec::new(),
                    importance: 0.9,
                },
                memories,
            })
        }
    }

    fn test_coordinator(store: Arc<dyn PersistentMemoryStore>) -> PersistentMemoryCoordinator {
        PersistentMemoryCoordinator::new(store)
            .with_memory_writer(Arc::new(FakePostRunMemoryWriter))
    }

    #[async_trait::async_trait]
    impl MemoryEmbeddingGenerator for FakeEmbeddingGenerator {
        async fn embed_document(
            &self,
            _text: &str,
            _title: Option<&str>,
        ) -> anyhow::Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }

        async fn embed_query(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![1.0, 0.0])
        }
    }

    #[tokio::test]
    async fn persist_completed_run_writes_episode_and_session_state() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = test_coordinator(store_for_coordinator);
        let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
        let messages = vec![
            AgentMessage::tool("tool-1", "read_file", "content"),
            AgentMessage::archive_reference_with_ref(
                "Archived displaced context chunk",
                Some(ArchiveRef {
                    archive_id: "archive-1".to_string(),
                    created_at: 1_700_000_000,
                    title: "Compacted history".to_string(),
                    storage_key: "archive/topic-a/flow-1/history-1.json".to_string(),
                }),
            ),
            AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
                goal: "Implement Stage 4".to_string(),
                decisions: vec!["Use persistent memory coordinator for PostRun durable writes".to_string()],
                constraints: vec!["Sub-agent runs must never persist durable memory records".to_string()],
                discoveries: vec!["PostRun persistence is handled in crates/oxide-agent-core/src/agent/runner/responses.rs".to_string()],
                risks: vec!["Need follow-up test".to_string()],
                ..crate::agent::CompactionSummary::default()
            }),
        ];

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-1",
                task_id: "episode-1",
                scope: &scope,
                task: "Implement Stage 4",
                messages: &messages,
                explicit_remember_intent: false,
                hot_token_estimate: 77,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "Done",
                },
            })
            .await
            .expect("post-run persistence should succeed");

        let episode = MemoryRepository::get_episode(store.as_ref(), &"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .expect("episode should exist");
        assert_eq!(episode.goal, "Implement Stage 4");
        assert_eq!(episode.summary, "Completed task: Implement Stage 4");
        assert_eq!(episode.tools_used, vec!["read_file".to_string()]);
        assert!(episode.failures.is_empty());
        assert_eq!(episode.artifacts.len(), 1);
        assert_eq!(
            episode.artifacts[0].storage_key,
            "archive/topic-a/flow-1/history-1.json"
        );

        let session_state = store
            .get_session_state("session-1")
            .await
            .expect("session state lookup should succeed")
            .expect("session state should exist");
        assert_eq!(session_state.cleanup_status, CleanupStatus::Finalized);
        assert_eq!(session_state.pending_episode_id, None);

        let memories = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default(),
        )
        .await
        .expect("memory lookup should succeed");
        assert_eq!(memories.len(), 3);
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Decision));
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Constraint));
        assert!(memories
            .iter()
            .any(|memory| memory.memory_type == MemoryType::Fact));
    }

    #[tokio::test]
    async fn persist_waiting_for_user_input_only_updates_session_state() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
        let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-1",
                task_id: "episode-1",
                scope: &scope,
                task: "Need browser URL",
                messages: &[],
                explicit_remember_intent: false,
                hot_token_estimate: 21,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::WaitingForUserInput,
            })
            .await
            .expect("waiting-state persistence should succeed");

        let state = store
            .get_session_state("session-1")
            .await
            .expect("session state lookup should succeed")
            .expect("session state should exist");
        assert_eq!(state.cleanup_status, CleanupStatus::Idle);
        assert_eq!(state.pending_episode_id.as_deref(), Some("episode-1"));
        assert!(
            MemoryRepository::get_episode(store.as_ref(), &"episode-1".to_string())
                .await
                .expect("episode lookup should succeed")
                .is_none()
        );
        assert!(MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default()
        )
        .await
        .expect("memory lookup should succeed")
        .is_empty());
    }

    #[tokio::test]
    async fn persist_completed_run_propagates_explicit_remember_intent_to_writer() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = test_coordinator(store_for_coordinator);
        let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
        let messages = vec![AgentMessage::user_turn(
            "Please remember this deployment workaround for later.",
        )];

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-remember",
                task_id: "episode-remember",
                scope: &scope,
                task: "Remember this deployment workaround",
                messages: &messages,
                explicit_remember_intent: true,
                hot_token_estimate: 33,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "Saved in memory after completion.",
                },
            })
            .await
            .expect("remember-intent persistence should succeed");

        let memories = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default(),
        )
        .await
        .expect("memory lookup should succeed");
        assert!(!memories.is_empty());
        assert!(memories
            .iter()
            .all(|memory| memory.tags.iter().any(|tag| tag == "explicit_remember")));
    }

    #[tokio::test]
    async fn persist_post_run_keeps_topic_scopes_isolated() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = test_coordinator(store_for_coordinator);

        let topic_a_scope = AgentMemoryScope::new(42, "topic-a", "flow-a");
        let topic_b_scope = AgentMemoryScope::new(42, "topic-b", "flow-b");

        let topic_a_messages = vec![
            AgentMessage::tool("tool-a", "read_file", "content"),
            AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
                goal: "Topic A".to_string(),
                decisions: vec![
                    "Use persistent memory repository for topic-a durable writes".to_string(),
                ],
                constraints: vec!["Topic-a durable memory records must stay isolated".to_string()],
                discoveries: vec!["topic-a records are stored in context_key".to_string()],
                risks: vec!["Need follow-up test".to_string()],
                ..crate::agent::CompactionSummary::default()
            }),
        ];
        let topic_b_messages = vec![
            AgentMessage::tool("tool-b", "read_file", "content"),
            AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
                goal: "Topic B".to_string(),
                decisions: vec![
                    "Use persistent memory repository for topic-b durable writes".to_string(),
                ],
                constraints: vec!["Topic-b durable memory records must stay isolated".to_string()],
                discoveries: vec!["topic-b records are stored in context_key".to_string()],
                risks: vec!["Need follow-up test".to_string()],
                ..crate::agent::CompactionSummary::default()
            }),
        ];

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-a",
                task_id: "episode-a",
                scope: &topic_a_scope,
                task: "topic a task",
                messages: &topic_a_messages,
                explicit_remember_intent: false,
                hot_token_estimate: 128,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("topic-a persistence should succeed");

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-b",
                task_id: "episode-b",
                scope: &topic_b_scope,
                task: "topic b task",
                messages: &topic_b_messages,
                explicit_remember_intent: false,
                hot_token_estimate: 256,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("topic-b persistence should succeed");

        let topic_a_memories = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default(),
        )
        .await
        .expect("topic-a memory lookup should succeed");
        let topic_b_memories = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-b",
            &MemoryListFilter::default(),
        )
        .await
        .expect("topic-b memory lookup should succeed");

        assert_eq!(topic_a_memories.len(), 3);
        assert_eq!(topic_b_memories.len(), 3);
        assert!(topic_a_memories
            .iter()
            .all(|memory| memory.context_key == "topic-a"));
        assert!(topic_b_memories
            .iter()
            .all(|memory| memory.context_key == "topic-b"));
        assert!(
            MemoryRepository::get_episode(store.as_ref(), &"episode-a".to_string())
                .await
                .expect("episode-a lookup should succeed")
                .is_some()
        );
        assert!(
            MemoryRepository::get_episode(store.as_ref(), &"episode-b".to_string())
                .await
                .expect("episode-b lookup should succeed")
                .is_some()
        );
    }

    #[tokio::test]
    async fn embedding_indexer_backfills_existing_episode_records() {
        let mut storage = MockStorageProvider::new();
        let candidate = EpisodeEmbeddingCandidate {
            record: EpisodeRecord {
                episode_id: "episode-1".to_string(),
                thread_id: "thread-1".to_string(),
                context_key: "topic-a".to_string(),
                goal: "Index embeddings".to_string(),
                summary: "Backfill older persistent memory records".to_string(),
                outcome: oxide_agent_memory::EpisodeOutcome::Success,
                tools_used: vec!["memory_search".to_string()],
                artifacts: Vec::new(),
                failures: Vec::new(),
                importance: 0.8,
                created_at: chrono::Utc::now(),
            },
            embedding: None,
        };

        storage
            .expect_list_memory_episode_embedding_backfill_candidates()
            .times(1)
            .return_once(move |_| Ok(vec![candidate]));
        storage
            .expect_list_memory_record_embedding_backfill_candidates()
            .times(1)
            .return_once(|_| Ok(Vec::new()));
        storage
            .expect_upsert_memory_embedding_pending()
            .times(1)
            .returning(|update: EmbeddingPendingUpdate| {
                Ok(oxide_agent_memory::EmbeddingRecord {
                    owner_id: update.base.owner_id,
                    owner_type: update.base.owner_type,
                    model_id: update.base.model_id,
                    content_hash: update.base.content_hash,
                    embedding: None,
                    dimensions: None,
                    status: oxide_agent_memory::EmbeddingStatus::Pending,
                    last_error: None,
                    retry_count: 0,
                    created_at: update.requested_at,
                    updated_at: update.requested_at,
                    indexed_at: None,
                })
            });
        storage
            .expect_upsert_memory_embedding_ready()
            .times(1)
            .returning(|update: EmbeddingReadyUpdate| {
                Ok(oxide_agent_memory::EmbeddingRecord {
                    owner_id: update.base.owner_id,
                    owner_type: update.base.owner_type,
                    model_id: update.base.model_id,
                    content_hash: update.base.content_hash,
                    dimensions: Some(update.embedding.len()),
                    embedding: Some(update.embedding),
                    status: oxide_agent_memory::EmbeddingStatus::Ready,
                    last_error: None,
                    retry_count: 0,
                    created_at: update.indexed_at,
                    updated_at: update.indexed_at,
                    indexed_at: Some(update.indexed_at),
                })
            });

        let indexer = PersistentMemoryEmbeddingIndexer::new(
            Arc::new(storage),
            Arc::new(FakeEmbeddingGenerator),
            "gemini-embedding-001",
        );

        indexer.backfill().await.expect("backfill should succeed");
    }

    fn retrieval_scope() -> AgentMemoryScope {
        AgentMemoryScope::new(42, "topic-a", "flow-a")
    }

    fn retrieval_episode() -> EpisodeRecord {
        EpisodeRecord {
            episode_id: "episode-1".to_string(),
            thread_id: "thread-1".to_string(),
            context_key: "topic-a".to_string(),
            goal: "Fix deploy regression".to_string(),
            summary: "Earlier deploy broke staging until config was corrected.".to_string(),
            outcome: EpisodeOutcome::Success,
            tools_used: vec!["memory_search".to_string()],
            artifacts: Vec::new(),
            failures: Vec::new(),
            importance: 0.82,
            created_at: chrono::Utc::now(),
        }
    }

    fn retrieval_memory() -> MemoryRecord {
        MemoryRecord {
            memory_id: "memory-1".to_string(),
            context_key: "topic-a".to_string(),
            source_episode_id: Some("episode-9".to_string()),
            memory_type: MemoryType::Procedure,
            title: "Deploy fix procedure".to_string(),
            content: "Rebuild config, then rerun the deploy with the staging profile.".to_string(),
            short_description: "staging recovery steps".to_string(),
            importance: 0.93,
            confidence: 0.94,
            source: Some("test".to_string()),
            content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                MemoryType::Procedure,
                "Rebuild config, then rerun the deploy with the staging profile.",
            )),
            reason: Some("fixture".to_string()),
            tags: vec!["deploy".to_string(), "staging".to_string()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        }
    }

    fn ts(seconds: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .timestamp_opt(seconds, 0)
            .single()
            .expect("valid timestamp")
    }

    fn repeated_summary_messages() -> Vec<AgentMessage> {
        vec![AgentMessage::from_compaction_summary(crate::agent::CompactionSummary {
            goal: "Keep memory hygiene".to_string(),
            decisions: vec!["Use persistent memory coordinator for durable writes".to_string()],
            constraints: vec!["Sub-agent runs must never persist durable memory records".to_string()],
            discoveries: vec!["PostRun persistence is handled in crates/oxide-agent-core/src/agent/runner/responses.rs".to_string()],
            ..crate::agent::CompactionSummary::default()
        })]
    }

    #[tokio::test]
    async fn durable_memory_retriever_skips_smalltalk_queries() {
        let storage = MockStorageProvider::new();
        let retriever = DurableMemoryRetriever::new(Arc::new(storage));

        let retrieval = retriever
            .retrieve(
                "thanks",
                &retrieval_scope(),
                DurableMemoryRetrievalOptions::default(),
            )
            .await
            .expect("smalltalk retrieval should not fail");

        assert!(retrieval.is_none());
    }

    #[tokio::test]
    async fn durable_memory_retriever_fuses_vector_and_lexical_hits() {
        let episode = retrieval_episode();
        let memory_for_lexical = retrieval_memory();
        let memory_for_vector = retrieval_memory();
        let mut storage = MockStorageProvider::new();
        storage
            .expect_search_memory_episodes_lexical()
            .times(1)
            .return_once(move |_, _| {
                Ok(vec![EpisodeSearchHit {
                    record: episode,
                    score: 0.4,
                    snippet: "episode lexical".to_string(),
                }])
            });
        storage
            .expect_search_memory_episodes_vector()
            .times(1)
            .return_once(|_, _| Ok(Vec::new()));
        storage
            .expect_search_memory_records_lexical()
            .times(1)
            .return_once(move |_, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_for_lexical,
                    score: 0.3,
                    snippet: "memory lexical".to_string(),
                }])
            });
        storage
            .expect_search_memory_records_vector()
            .times(1)
            .return_once(move |_, _| {
                Ok(vec![MemorySearchHit {
                    record: memory_for_vector,
                    score: 0.96,
                    snippet: "memory semantic".to_string(),
                }])
            });

        let retriever = DurableMemoryRetriever::new(Arc::new(storage))
            .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
        let retrieval = retriever
            .retrieve(
                "how was the deploy fixed before?",
                &retrieval_scope(),
                DurableMemoryRetrievalOptions::default(),
            )
            .await
            .expect("hybrid retrieval should succeed")
            .expect("retrieval should produce candidates");

        assert_eq!(retrieval.items.len(), 2);
        assert!(matches!(retrieval.items[0], HybridCandidate::Memory { .. }));
        assert!(matches!(
            retrieval.items[1],
            HybridCandidate::Episode { .. }
        ));

        let rendered = retrieval.render_for_prompt();
        assert!(rendered.contains("Scoped durable memory context"));
        assert!(rendered.contains("memory memory-1"));
        assert!(rendered.contains("episode episode-1"));
        assert!(rendered.contains("Open full thread only if needed"));
    }

    #[tokio::test]
    async fn durable_memory_search_reuses_hybrid_retrieval_core() {
        let episode = retrieval_episode();
        let memory_for_lexical = retrieval_memory();
        let memory_for_vector = retrieval_memory();
        let mut storage = MockStorageProvider::new();
        storage
            .expect_search_memory_episodes_lexical()
            .times(2)
            .returning({
                let episode = episode.clone();
                move |_, _| {
                    Ok(vec![EpisodeSearchHit {
                        record: episode.clone(),
                        score: 0.4,
                        snippet: "episode lexical".to_string(),
                    }])
                }
            });
        storage
            .expect_search_memory_episodes_vector()
            .times(2)
            .returning(|_, _| Ok(Vec::new()));
        storage
            .expect_search_memory_records_lexical()
            .times(2)
            .returning({
                let memory_for_lexical = memory_for_lexical.clone();
                move |_, _| {
                    Ok(vec![MemorySearchHit {
                        record: memory_for_lexical.clone(),
                        score: 0.3,
                        snippet: "memory lexical".to_string(),
                    }])
                }
            });
        storage
            .expect_search_memory_records_vector()
            .times(2)
            .returning({
                let memory_for_vector = memory_for_vector.clone();
                move |_, _| {
                    Ok(vec![MemorySearchHit {
                        record: memory_for_vector.clone(),
                        score: 0.96,
                        snippet: "memory semantic".to_string(),
                    }])
                }
            });

        let retriever = DurableMemoryRetriever::new(Arc::new(storage))
            .with_query_embedding_generator(Arc::new(FakeEmbeddingGenerator));
        let prompt_retrieval = retriever
            .retrieve(
                "how was the deploy fixed before?",
                &retrieval_scope(),
                DurableMemoryRetrievalOptions::default(),
            )
            .await
            .expect("prompt retrieval should succeed")
            .expect("prompt retrieval should yield items");
        let search_items = retriever
            .search(
                &retrieval_scope(),
                DurableMemorySearchRequest {
                    query: "how was the deploy fixed before?".to_string(),
                    search_episodes: true,
                    search_memories: true,
                    memory_type: Some(MemoryType::Procedure),
                    time_range: Default::default(),
                    min_importance: Some(0.45),
                    limit: 5,
                    candidate_limit: Some(8),
                    allow_full_thread_read: true,
                },
            )
            .await
            .expect("tool search should succeed");

        assert_eq!(prompt_retrieval.items.len(), search_items.len());
        assert!(matches!(
            prompt_retrieval.items[0],
            HybridCandidate::Memory { .. }
        ));
        assert!(matches!(
            prompt_retrieval.items[1],
            HybridCandidate::Episode { .. }
        ));
        assert!(matches!(
            search_items[0],
            DurableMemorySearchItem::Memory { .. }
        ));
        assert!(matches!(
            search_items[1],
            DurableMemorySearchItem::Episode { .. }
        ));
    }

    #[tokio::test]
    async fn durable_memory_search_reports_empty_reason_when_search_returns_no_candidates() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_search_memory_episodes_lexical()
            .times(1)
            .return_once(|_, _| Ok(Vec::new()));
        storage
            .expect_search_memory_records_lexical()
            .times(1)
            .return_once(|_, _| Ok(Vec::new()));

        let retriever = DurableMemoryRetriever::new(Arc::new(storage));
        let outcome = retriever
            .search_with_diagnostics(
                &retrieval_scope(),
                DurableMemorySearchRequest {
                    query: "how was the deploy fixed before?".to_string(),
                    search_episodes: true,
                    search_memories: true,
                    memory_type: None,
                    time_range: Default::default(),
                    min_importance: None,
                    limit: 5,
                    candidate_limit: Some(8),
                    allow_full_thread_read: true,
                },
            )
            .await
            .expect("diagnostic search should succeed");

        assert!(outcome.items.is_empty());
        assert_eq!(outcome.diagnostics.empty_reason, Some("no_search_hits"));
        assert_eq!(outcome.diagnostics.episode_lexical_hits, 0);
        assert_eq!(outcome.diagnostics.injected_item_count, 0);
        assert_eq!(outcome.diagnostics.filtered_low_score, 0);
    }

    #[tokio::test]
    async fn persist_post_run_consolidates_duplicate_memories() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = test_coordinator(store_for_coordinator);
        let scope = AgentMemoryScope::new(42, "topic-a", "flow-1");
        let messages = repeated_summary_messages();

        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-1",
                task_id: "episode-1",
                scope: &scope,
                task: "keep memory hygiene",
                messages: &messages,
                explicit_remember_intent: false,
                hot_token_estimate: 32,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("first persistence should succeed");
        coordinator
            .persist_post_run(PersistentRunContext {
                session_id: "session-2",
                task_id: "episode-2",
                scope: &scope,
                task: "keep memory hygiene again",
                messages: &messages,
                explicit_remember_intent: false,
                hot_token_estimate: 40,
                tool_memory_drafts: Vec::new(),
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("second persistence should succeed");

        let active = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default(),
        )
        .await
        .expect("active memories should list");
        let all = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter {
                include_deleted: true,
                ..MemoryListFilter::default()
            },
        )
        .await
        .expect("full memory listing should succeed");

        assert_eq!(active.len(), 3);
        assert_eq!(all.len(), 6);
        assert_eq!(
            all.iter()
                .filter(|memory| memory.deleted_at.is_some())
                .count(),
            3
        );
    }

    #[tokio::test]
    async fn watchdog_pass_consolidates_stale_context() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        MemoryRepository::create_memory(
            store.as_ref(),
            MemoryRecord {
                memory_id: "memory-a".to_string(),
                context_key: "topic-a".to_string(),
                source_episode_id: Some("episode-a".to_string()),
                memory_type: MemoryType::Fact,
                title: "Fact a".to_string(),
                content: "Use cargo check before build".to_string(),
                short_description: "cargo check before build".to_string(),
                importance: 0.7,
                confidence: 0.8,
                source: Some("test".to_string()),
                content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                    MemoryType::Fact,
                    "Use cargo check before build",
                )),
                reason: None,
                tags: vec!["fact".to_string()],
                created_at: ts(10),
                updated_at: ts(10),
                deleted_at: None,
            },
        )
        .await
        .expect("first memory should store");
        MemoryRepository::create_memory(
            store.as_ref(),
            MemoryRecord {
                memory_id: "memory-b".to_string(),
                context_key: "topic-a".to_string(),
                source_episode_id: Some("episode-b".to_string()),
                memory_type: MemoryType::Fact,
                title: "Fact b".to_string(),
                content: "Use cargo check before build".to_string(),
                short_description: "cargo check before build".to_string(),
                importance: 0.6,
                confidence: 0.7,
                source: Some("test".to_string()),
                content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                    MemoryType::Fact,
                    "Use cargo check before build",
                )),
                reason: None,
                tags: vec!["fact".to_string()],
                created_at: ts(20),
                updated_at: ts(20),
                deleted_at: None,
            },
        )
        .await
        .expect("second memory should store");
        MemoryRepository::upsert_session_state(
            store.as_ref(),
            SessionStateRecord {
                session_id: "session-a".to_string(),
                context_key: "topic-a".to_string(),
                hot_token_estimate: 64,
                last_compacted_at: None,
                last_finalized_at: None,
                cleanup_status: CleanupStatus::Idle,
                pending_episode_id: None,
                updated_at: ts(0),
            },
        )
        .await
        .expect("session state should store");

        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
        coordinator.run_watchdog_pass(ts(100_000)).await;

        let active = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter::default(),
        )
        .await
        .expect("active memories should list");
        let deleted = MemoryRepository::list_memories(
            store.as_ref(),
            "topic-a",
            &MemoryListFilter {
                include_deleted: true,
                ..MemoryListFilter::default()
            },
        )
        .await
        .expect("deleted memories should list");

        assert_eq!(active.len(), 1);
        assert_eq!(
            deleted
                .iter()
                .filter(|memory| memory.deleted_at.is_some())
                .count(),
            1
        );
    }
}
