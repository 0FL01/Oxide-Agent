use crate::agent::memory::AgentMessage;
use crate::agent::session::AgentMemoryScope;
use crate::llm::{EmbeddingTaskType, LlmClient};
use crate::storage::StorageProvider;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use oxide_agent_memory::{
    ArtifactRef, EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType,
    EmbeddingPendingUpdate, EmbeddingReadyUpdate, EmbeddingUpdateBase, EpisodeFinalizationInput,
    EpisodeFinalizer, EpisodeMemorySignals, EpisodeRecord, MemoryRecord, MemoryRepository,
    RepositoryError, ReusableMemoryExtractor, SessionStateRecord, ThreadRecord,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::warn;

const EMBEDDING_BACKFILL_LIMIT: usize = 8;

/// Object-safe persistent-memory write surface used by the runner.
#[async_trait]
pub trait PersistentMemoryStore: Send + Sync {
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError>;
    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError>;
    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError>;
    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError>;
}

#[async_trait]
impl<T> PersistentMemoryStore for T
where
    T: MemoryRepository + Send + Sync,
{
    async fn upsert_thread(&self, record: ThreadRecord) -> Result<ThreadRecord, RepositoryError> {
        MemoryRepository::upsert_thread(self, record).await
    }

    async fn create_episode(
        &self,
        record: oxide_agent_memory::EpisodeRecord,
    ) -> Result<oxide_agent_memory::EpisodeRecord, RepositoryError> {
        MemoryRepository::create_episode(self, record).await
    }

    async fn create_memory(
        &self,
        record: oxide_agent_memory::MemoryRecord,
    ) -> Result<oxide_agent_memory::MemoryRecord, RepositoryError> {
        MemoryRepository::create_memory(self, record).await
    }

    async fn upsert_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, RepositoryError> {
        MemoryRepository::upsert_session_state(self, record).await
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
    pub hot_token_estimate: usize,
    pub phase: PersistentRunPhase<'a>,
}

#[derive(Clone)]
pub struct PersistentMemoryCoordinator {
    store: Arc<dyn PersistentMemoryStore>,
    finalizer: EpisodeFinalizer,
    extractor: ReusableMemoryExtractor,
    embedding_indexer: Option<PersistentMemoryEmbeddingIndexer>,
}

impl PersistentMemoryCoordinator {
    #[must_use]
    pub fn new(store: Arc<dyn PersistentMemoryStore>) -> Self {
        Self {
            store,
            finalizer: EpisodeFinalizer,
            extractor: ReusableMemoryExtractor::new(),
            embedding_indexer: None,
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

    pub async fn persist_post_run(&self, ctx: PersistentRunContext<'_>) -> Result<()> {
        let summary_signal = latest_summary_signal(ctx.messages);
        let artifacts = collect_artifacts(ctx.messages);
        let tools_used = collect_tools_used(ctx.messages);
        let final_answer = match ctx.phase {
            PersistentRunPhase::Completed { final_answer } => Some(final_answer.to_string()),
            PersistentRunPhase::WaitingForUserInput => None,
        };
        let plan = self.finalizer.build_plan(EpisodeFinalizationInput {
            user_id: ctx.scope.user_id,
            context_key: ctx.scope.context_key.clone(),
            flow_id: ctx.scope.flow_id.clone(),
            session_id: ctx.session_id.to_string(),
            episode_id: ctx.task_id.to_string(),
            goal: ctx.task.to_string(),
            final_answer,
            compaction_summary: summary_signal
                .as_ref()
                .map(|signal| signal.summary_text.clone()),
            tools_used,
            artifacts,
            failures: summary_signal
                .as_ref()
                .map_or_else(Vec::new, |signal| signal.failures.clone()),
            hot_token_estimate: ctx.hot_token_estimate,
            finalized_at: Utc::now(),
        });

        self.store.upsert_thread(plan.thread).await?;
        let episode = if let Some(episode) = plan.episode {
            Some(self.store.create_episode(episode).await?)
        } else {
            None
        };
        if let (Some(indexer), Some(episode)) = (self.embedding_indexer.as_ref(), episode.as_ref())
        {
            if let Err(error) = indexer.index_episode(episode).await {
                warn!(error = %error, episode_id = %episode.episode_id, "episode embedding write failed");
            }
        }
        if let Some(episode) = episode.as_ref() {
            self.persist_reusable_memories(episode, summary_signal.as_ref())
                .await;
        }
        if let Some(indexer) = self.embedding_indexer.as_ref() {
            if let Err(error) = indexer.backfill().await {
                warn!(error = %error, "persistent memory embedding backfill failed");
            }
        }
        self.store.upsert_session_state(plan.session_state).await?;
        Ok(())
    }

    async fn persist_reusable_memories(
        &self,
        episode: &oxide_agent_memory::EpisodeRecord,
        summary_signal: Option<&PersistentSummarySignal>,
    ) {
        let Some(summary_signal) = summary_signal else {
            return;
        };

        let signals = EpisodeMemorySignals {
            decisions: summary_signal.decisions.clone(),
            constraints: summary_signal.constraints.clone(),
            discoveries: summary_signal.discoveries.clone(),
        };
        for memory in self.extractor.extract(episode, &signals) {
            match self.store.create_memory(memory).await {
                Ok(memory) => {
                    if let Some(indexer) = self.embedding_indexer.as_ref() {
                        if let Err(error) = indexer.index_memory(&memory).await {
                            warn!(error = %error, memory_id = %memory.memory_id, "reusable memory embedding write failed");
                        }
                    }
                }
                Err(error) => {
                    warn!(error = %error, episode_id = %episode.episode_id, "Reusable memory extraction write failed");
                }
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
    storage: Arc<dyn StorageProvider>,
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
        Self {
            storage,
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
        self.storage
            .upsert_memory_embedding_pending(EmbeddingPendingUpdate {
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
                self.storage
                    .upsert_memory_embedding_ready(EmbeddingReadyUpdate {
                        base,
                        embedding,
                        indexed_at: Utc::now(),
                    })
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.storage
                    .upsert_memory_embedding_failure(EmbeddingFailureUpdate {
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
        self.storage
            .upsert_memory_embedding_pending(EmbeddingPendingUpdate {
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
                self.storage
                    .upsert_memory_embedding_ready(EmbeddingReadyUpdate {
                        base,
                        embedding,
                        indexed_at: Utc::now(),
                    })
                    .await?;
                Ok(())
            }
            Err(error) => {
                self.storage
                    .upsert_memory_embedding_failure(EmbeddingFailureUpdate {
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
        for candidate in self
            .storage
            .list_memory_episode_embedding_backfill_candidates(request.clone())
            .await?
        {
            self.index_episode(&candidate.record).await?;
        }
        for candidate in self
            .storage
            .list_memory_record_embedding_backfill_candidates(request)
            .await?
        {
            self.index_memory(&candidate.record).await?;
        }
        Ok(())
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
    decisions: Vec<String>,
    constraints: Vec<String>,
    discoveries: Vec<String>,
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
            decisions: summary
                .decisions
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            constraints: summary
                .constraints
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            discoveries: summary
                .discoveries
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
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
        MemoryEmbeddingGenerator, PersistentMemoryCoordinator, PersistentMemoryEmbeddingIndexer,
        PersistentMemoryStore, PersistentRunContext, PersistentRunPhase,
    };
    use crate::agent::compaction::ArchiveRef;
    use crate::agent::memory::AgentMessage;
    use crate::agent::session::AgentMemoryScope;
    use crate::storage::MockStorageProvider;
    use oxide_agent_memory::{
        CleanupStatus, EmbeddingPendingUpdate, EmbeddingReadyUpdate, EpisodeEmbeddingCandidate,
        EpisodeRecord, InMemoryMemoryRepository, MemoryListFilter, MemoryRepository, MemoryType,
    };
    use std::sync::Arc;

    struct FakeEmbeddingGenerator;

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
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);
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
                hot_token_estimate: 77,
                phase: PersistentRunPhase::Completed {
                    final_answer: "Done",
                },
            })
            .await
            .expect("post-run persistence should succeed");

        let episode = store
            .get_episode(&"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .expect("episode should exist");
        assert_eq!(episode.goal, "Implement Stage 4");
        assert_eq!(episode.tools_used, vec!["read_file".to_string()]);
        assert_eq!(episode.failures, vec!["Need follow-up test".to_string()]);
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

        let memories = store
            .list_memories("topic-a", &MemoryListFilter::default())
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
                hot_token_estimate: 21,
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
        assert!(store
            .get_episode(&"episode-1".to_string())
            .await
            .expect("episode lookup should succeed")
            .is_none());
        assert!(store
            .list_memories("topic-a", &MemoryListFilter::default())
            .await
            .expect("memory lookup should succeed")
            .is_empty());
    }

    #[tokio::test]
    async fn persist_post_run_keeps_topic_scopes_isolated() {
        let store = Arc::new(InMemoryMemoryRepository::new());
        let store_for_coordinator = Arc::clone(&store);
        let store_for_coordinator: Arc<dyn PersistentMemoryStore> = store_for_coordinator;
        let coordinator = PersistentMemoryCoordinator::new(store_for_coordinator);

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
                hot_token_estimate: 128,
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
                hot_token_estimate: 256,
                phase: PersistentRunPhase::Completed {
                    final_answer: "done",
                },
            })
            .await
            .expect("topic-b persistence should succeed");

        let topic_a_memories = store
            .list_memories("topic-a", &MemoryListFilter::default())
            .await
            .expect("topic-a memory lookup should succeed");
        let topic_b_memories = store
            .list_memories("topic-b", &MemoryListFilter::default())
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
        assert!(store
            .get_episode(&"episode-a".to_string())
            .await
            .expect("episode-a lookup should succeed")
            .is_some());
        assert!(store
            .get_episode(&"episode-b".to_string())
            .await
            .expect("episode-b lookup should succeed")
            .is_some());
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
}
