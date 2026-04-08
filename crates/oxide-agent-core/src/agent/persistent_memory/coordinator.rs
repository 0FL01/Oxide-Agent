use super::*;

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

    pub(crate) async fn run_watchdog_pass(&self, now: chrono::DateTime<Utc>) {
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
