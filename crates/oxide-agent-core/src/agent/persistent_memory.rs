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
    EpisodeFinalizer, EpisodeMemorySignals, EpisodeOutcome, EpisodeRecord, EpisodeSearchFilter,
    EpisodeSearchHit, MemoryRecord, MemoryRepository, MemorySearchFilter, MemorySearchHit,
    MemoryType, RepositoryError, ReusableMemoryExtractor, SessionStateRecord, ThreadRecord,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::warn;

const EMBEDDING_BACKFILL_LIMIT: usize = 8;
const HYBRID_RETRIEVAL_CANDIDATE_LIMIT: usize = 8;
const HYBRID_RETRIEVAL_TOP_K: usize = 5;
const HYBRID_RETRIEVAL_MIN_SCORE: f32 = 0.45;

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

#[derive(Debug, Clone, Copy, Default)]
pub struct DurableMemoryRetrievalOptions {
    pub rerank: bool,
    pub top_k: Option<usize>,
}

#[derive(Clone)]
pub struct DurableMemoryRetriever {
    storage: Arc<dyn StorageProvider>,
    generator: Option<Arc<dyn MemoryEmbeddingGenerator>>,
}

impl DurableMemoryRetriever {
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>) -> Self {
        Self {
            storage,
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

    pub async fn render_prompt_context(
        &self,
        task: &str,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<String>> {
        let Some(retrieval) = self.retrieve(task, scope, options).await? else {
            return Ok(None);
        };
        Ok(Some(retrieval.render_for_prompt()))
    }

    async fn retrieve(
        &self,
        task: &str,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<DurableMemoryRetrieval>> {
        let Some(plan) = query_retrieval_plan(task, options) else {
            return Ok(None);
        };

        let mut candidates = Vec::new();

        if plan.search_episodes {
            let filter = episode_search_filter(scope, &plan);
            let lexical_hits = self
                .storage
                .search_memory_episodes_lexical(plan.query.clone(), filter.clone())
                .await?;
            let vector_hits = self.search_episode_vectors(&plan, &filter).await;
            candidates.extend(fuse_episode_hits(lexical_hits, vector_hits));
        }

        if plan.search_memories {
            let filter = memory_search_filter(scope, &plan);
            let lexical_hits = self
                .storage
                .search_memory_records_lexical(plan.query.clone(), filter.clone())
                .await?;
            let vector_hits = self.search_memory_vectors(&plan, &filter).await;
            candidates.extend(fuse_memory_hits(lexical_hits, vector_hits));
        }

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
                continue;
            }

            if !seen_snippets.insert(normalized_snippet_key(candidate.snippet())) {
                continue;
            }

            if let Some(source_episode_id) = candidate.source_episode_id() {
                if covered_episode_ids.contains(source_episode_id) {
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

        if items.is_empty() {
            return Ok(None);
        }

        Ok(Some(DurableMemoryRetrieval {
            plan,
            items,
            rerank_applied: false,
        }))
    }

    async fn search_episode_vectors(
        &self,
        plan: &RetrievalPlan,
        filter: &EpisodeSearchFilter,
    ) -> Vec<EpisodeSearchHit> {
        let Some(generator) = self.generator.as_ref() else {
            return Vec::new();
        };

        let query_embedding = match generator.embed_query(&plan.query).await {
            Ok(query_embedding) => query_embedding,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory query embedding failed");
                return Vec::new();
            }
        };

        match self
            .storage
            .search_memory_episodes_vector(query_embedding, filter.clone())
            .await
        {
            Ok(hits) => hits,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory episode vector search failed");
                Vec::new()
            }
        }
    }

    async fn search_memory_vectors(
        &self,
        plan: &RetrievalPlan,
        filter: &MemorySearchFilter,
    ) -> Vec<MemorySearchHit> {
        let Some(generator) = self.generator.as_ref() else {
            return Vec::new();
        };

        let query_embedding = match generator.embed_query(&plan.query).await {
            Ok(query_embedding) => query_embedding,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory query embedding failed");
                return Vec::new();
            }
        };

        match self
            .storage
            .search_memory_records_vector(query_embedding, filter.clone())
            .await
        {
            Ok(hits) => hits,
            Err(error) => {
                warn!(error = %error, query = %plan.query, "durable memory record vector search failed");
                Vec::new()
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
    rerank_requested: bool,
}

#[derive(Debug, Clone)]
struct DurableMemoryRetrieval {
    plan: RetrievalPlan,
    items: Vec<HybridCandidate>,
    rerank_applied: bool,
}

impl DurableMemoryRetrieval {
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
            format!(
                "- rerank: {}",
                if self.plan.rerank_requested && self.rerank_applied {
                    "applied"
                } else if self.plan.rerank_requested {
                    "requested but disabled"
                } else {
                    "disabled"
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
        rerank_requested: options.rerank,
    })
}

fn episode_search_filter(scope: &AgentMemoryScope, plan: &RetrievalPlan) -> EpisodeSearchFilter {
    EpisodeSearchFilter {
        context_key: Some(scope.context_key.clone()),
        user_id: Some(scope.user_id),
        outcome: None,
        min_importance: Some(plan.min_importance),
        time_range: Default::default(),
        limit: Some(HYBRID_RETRIEVAL_CANDIDATE_LIMIT),
    }
}

fn memory_search_filter(scope: &AgentMemoryScope, plan: &RetrievalPlan) -> MemorySearchFilter {
    MemorySearchFilter {
        context_key: Some(scope.context_key.clone()),
        user_id: Some(scope.user_id),
        memory_type: plan.memory_type,
        min_importance: Some(plan.min_importance),
        tags: Vec::new(),
        time_range: Default::default(),
        limit: Some(HYBRID_RETRIEVAL_CANDIDATE_LIMIT),
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
        DurableMemoryRetrievalOptions, DurableMemoryRetriever, HybridCandidate,
        MemoryEmbeddingGenerator, PersistentMemoryCoordinator, PersistentMemoryEmbeddingIndexer,
        PersistentMemoryStore, PersistentRunContext, PersistentRunPhase,
    };
    use crate::agent::compaction::ArchiveRef;
    use crate::agent::memory::AgentMessage;
    use crate::agent::session::AgentMemoryScope;
    use crate::storage::MockStorageProvider;
    use oxide_agent_memory::{
        CleanupStatus, EmbeddingPendingUpdate, EmbeddingReadyUpdate, EpisodeEmbeddingCandidate,
        EpisodeOutcome, EpisodeRecord, EpisodeSearchHit, InMemoryMemoryRepository,
        MemoryListFilter, MemoryRecord, MemoryRepository, MemorySearchHit, MemoryType,
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
            reason: Some("fixture".to_string()),
            tags: vec!["deploy".to_string(), "staging".to_string()],
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
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
}
