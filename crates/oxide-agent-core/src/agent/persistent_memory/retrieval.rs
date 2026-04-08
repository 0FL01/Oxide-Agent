use super::*;

const HYBRID_RETRIEVAL_CANDIDATE_LIMIT: usize = 8;
const HYBRID_RETRIEVAL_TOP_K: usize = 5;
const HYBRID_RETRIEVAL_MIN_SCORE: f32 = 0.45;

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
    pub task_class: String,
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
    pub filtered_vector_only_memory: usize,
    pub filtered_duplicate_snippet: usize,
    pub filtered_covered_episode: usize,
    pub empty_reason: Option<&'static str>,
    pub episode_vector_status: RetrievalVectorStatus,
    pub memory_vector_status: RetrievalVectorStatus,
}

impl DurableMemoryRetrievalDiagnostics {
    fn skipped(
        query: impl Into<String>,
        task_class: impl Into<String>,
        empty_reason: &'static str,
    ) -> Self {
        Self {
            query: query.into(),
            task_class: task_class.into(),
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
            filtered_vector_only_memory: 0,
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
            task_class: plan.class_label.clone(),
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
            filtered_vector_only_memory: 0,
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
            task_class = diagnostics.task_class,
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
            filtered_vector_only_memory = diagnostics.filtered_vector_only_memory,
            filtered_duplicate_snippet = diagnostics.filtered_duplicate_snippet,
            filtered_covered_episode = diagnostics.filtered_covered_episode,
            empty_reason = diagnostics.empty_reason.unwrap_or("none"),
            "Durable memory retrieval telemetry"
        );
    }

    pub async fn render_prompt_context(
        &self,
        task: &str,
        classification: &MemoryClassificationDecision,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<String>> {
        let outcome = self
            .retrieve_outcome_for_task(task, classification, scope, options)
            .await?;
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
        let plan = match RetrievalPlan::from_search_request(&request) {
            Ok(plan) => plan,
            Err(empty_reason) => {
                let diagnostics = DurableMemoryRetrievalDiagnostics::skipped(
                    query,
                    "explicit_search",
                    empty_reason,
                );
                Self::log_retrieval_telemetry("tool", &diagnostics);
                return Ok(DurableMemorySearchOutcome {
                    items: Vec::new(),
                    diagnostics,
                });
            }
        };

        let candidate_limit = request
            .candidate_limit
            .unwrap_or_else(|| request.limit.max(HYBRID_RETRIEVAL_CANDIDATE_LIMIT));

        let outcome = self
            .retrieve_with_plan(scope, plan, request.time_range.clone(), candidate_limit)
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
    pub(crate) async fn retrieve(
        &self,
        task: &str,
        classification: &MemoryClassificationDecision,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Option<DurableMemoryRetrieval>> {
        Ok(self
            .retrieve_outcome_for_task(task, classification, scope, options)
            .await?
            .retrieval)
    }

    async fn retrieve_outcome_for_task(
        &self,
        task: &str,
        classification: &MemoryClassificationDecision,
        scope: &AgentMemoryScope,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<DurableMemoryRetrievalOutcome> {
        let plan = match RetrievalPlan::from_classifier(task, classification, options) {
            Ok(plan) => plan,
            Err(empty_reason) => {
                let diagnostics = DurableMemoryRetrievalDiagnostics::skipped(
                    task,
                    classification.class.as_str(),
                    empty_reason,
                );
                return Ok(DurableMemoryRetrievalOutcome {
                    retrieval: None,
                    diagnostics,
                });
            }
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

            if matches!(
                candidate,
                HybridCandidate::Memory {
                    lexical_score: None,
                    vector_score: Some(_),
                    ..
                }
            ) && !plan.allow_vector_only_memory
            {
                diagnostics.filtered_vector_only_memory += 1;
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
    class_label: String,
    search_episodes: bool,
    search_memories: bool,
    memory_type: Option<MemoryType>,
    min_importance: f32,
    top_k: usize,
    allow_full_thread_read: bool,
    allow_vector_only_memory: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DurableMemoryRetrieval {
    pub(crate) items: Vec<HybridCandidate>,
    plan: RetrievalPlan,
}

impl DurableMemoryRetrieval {
    fn into_search_items(self) -> Vec<DurableMemorySearchItem> {
        self.items
            .into_iter()
            .map(DurableMemorySearchItem::from)
            .collect()
    }

    pub(crate) fn render_for_prompt(&self) -> String {
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
pub(crate) enum HybridCandidate {
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

impl RetrievalPlan {
    fn from_classifier(
        task: &str,
        classification: &MemoryClassificationDecision,
        options: DurableMemoryRetrievalOptions,
    ) -> Result<Self, &'static str> {
        let query = task.trim();
        if query.is_empty() {
            return Err("empty_query");
        }

        let read_policy = &classification.read_policy;
        if !read_policy.inject_prompt_memory
            || (!read_policy.search_episodes && !read_policy.search_memories)
        {
            return Err("query_class_disallows_prompt_memory");
        }

        Ok(Self {
            query: query.to_string(),
            class_label: classification.class.as_str().to_string(),
            search_episodes: read_policy.search_episodes,
            search_memories: read_policy.search_memories,
            memory_type: read_policy.memory_type,
            min_importance: read_policy.min_importance.clamp(0.0, 1.0),
            top_k: options
                .top_k
                .unwrap_or(read_policy.top_k)
                .clamp(1, HYBRID_RETRIEVAL_TOP_K),
            allow_full_thread_read: read_policy.allow_full_thread_read,
            allow_vector_only_memory: read_policy.allow_vector_only_memory,
        })
    }

    fn from_search_request(request: &DurableMemorySearchRequest) -> Result<Self, &'static str> {
        let query = request.query.trim();
        if query.is_empty() {
            return Err("empty_query");
        }
        if !request.search_episodes && !request.search_memories {
            return Err("no_sources_requested");
        }

        Ok(Self {
            query: query.to_string(),
            class_label: "explicit_search".to_string(),
            search_episodes: request.search_episodes,
            search_memories: request.search_memories,
            memory_type: request.memory_type,
            min_importance: request.min_importance.unwrap_or(0.0).clamp(0.0, 1.0),
            top_k: request.limit.max(1),
            allow_full_thread_read: request.allow_full_thread_read,
            allow_vector_only_memory: true,
        })
    }
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

fn normalized_snippet_key(snippet: &str) -> String {
    snippet
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
