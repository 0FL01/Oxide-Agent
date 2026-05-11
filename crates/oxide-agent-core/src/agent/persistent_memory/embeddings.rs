use super::*;

const EMBEDDING_BACKFILL_LIMIT: usize = 8;

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
