use super::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, Message, ReminderJobRecord, ReminderJobStatus, StorageError,
    TopicAgentsMdRecord, TopicBindingRecord, TopicContextRecord, TopicInfraConfigRecord,
    UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions,
    UpsertTopicContextOptions, UpsertTopicInfraConfigOptions, UserConfig,
};
use crate::agent::memory::AgentMemory;
use async_trait::async_trait;
use oxide_agent_memory::{
    ArtifactRef, EmbeddingBackfillRequest, EmbeddingFailureUpdate, EmbeddingOwnerType,
    EmbeddingPendingUpdate, EmbeddingReadyUpdate, EmbeddingRecord, EpisodeEmbeddingCandidate,
    EpisodeListFilter, EpisodeRecord, EpisodeSearchFilter, EpisodeSearchHit,
    MemoryEmbeddingCandidate, MemoryListFilter, MemoryRecord, MemorySearchFilter, MemorySearchHit,
    SessionStateRecord, ThreadRecord,
};

/// Interface for storage providers.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Get user configuration.
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError>;
    /// Update user configuration.
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError>;
    /// Update user system prompt.
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError>;
    /// Get user system prompt.
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Update user model.
    async fn update_user_model(&self, user_id: i64, model_name: String)
        -> Result<(), StorageError>;
    /// Get user model.
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Update user state.
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError>;
    /// Get user state.
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError>;
    /// Save message to chat history.
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError>;
    /// Get chat history for a user.
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError>;
    /// Clear chat history for a user.
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError>;
    /// Save message to chat history scoped by chat UUID.
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError>;
    /// Get chat history for a user scoped by chat UUID.
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError>;
    /// Clear chat history for a user scoped by chat UUID.
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError>;
    /// Clear all chat histories scoped by transport context.
    async fn clear_chat_history_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let config = self.get_user_config(user_id).await?;
        if let Some(chat_uuid) = config
            .contexts
            .get(&context_key)
            .and_then(|context| context.current_chat_uuid.clone())
        {
            self.clear_chat_history_for_chat(user_id, chat_uuid).await?;
        }
        Ok(())
    }
    /// Save agent memory to storage.
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError>;
    /// Save agent memory scoped by transport context.
    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let _ = context_key;
        self.save_agent_memory(user_id, memory).await
    }
    /// Load agent memory from storage.
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError>;
    /// Load agent memory scoped by transport context.
    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        let _ = context_key;
        self.load_agent_memory(user_id).await
    }
    /// Clear agent memory for a user.
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError>;
    /// Clear agent memory scoped by transport context.
    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let _ = context_key;
        self.clear_agent_memory(user_id).await
    }
    /// Save agent memory scoped by transport context and specific agent flow.
    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let _ = flow_id;
        self.save_agent_memory_for_context(user_id, context_key, memory)
            .await
    }
    /// Load agent memory scoped by transport context and specific agent flow.
    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        let _ = flow_id;
        self.load_agent_memory_for_context(user_id, context_key)
            .await
    }
    /// Clear agent memory scoped by transport context and specific agent flow.
    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        let _ = flow_id;
        self.clear_agent_memory_for_context(user_id, context_key)
            .await
    }
    /// Get metadata for a persisted topic-scoped agent flow.
    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError>;
    /// Upsert metadata for a persisted topic-scoped agent flow.
    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError>;
    /// Upsert a persistent memory thread record.
    async fn upsert_memory_thread(
        &self,
        record: ThreadRecord,
    ) -> Result<ThreadRecord, StorageError> {
        let _ = record;
        Err(StorageError::Config(
            "persistent memory threads are not implemented for this storage provider".to_string(),
        ))
    }
    /// Create a persistent memory episode record.
    async fn create_memory_episode(
        &self,
        record: EpisodeRecord,
    ) -> Result<EpisodeRecord, StorageError> {
        let _ = record;
        Err(StorageError::Config(
            "persistent memory episodes are not implemented for this storage provider".to_string(),
        ))
    }
    /// Link or merge one artifact into a persistent memory episode.
    async fn link_memory_episode_artifact(
        &self,
        episode_id: String,
        artifact: ArtifactRef,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        let _ = episode_id;
        let _ = artifact;
        Err(StorageError::Config(
            "persistent memory artifact linking is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Create a reusable persistent memory record.
    async fn create_memory_record(
        &self,
        record: MemoryRecord,
    ) -> Result<MemoryRecord, StorageError> {
        let _ = record;
        Err(StorageError::Config(
            "reusable persistent memories are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Upsert a persistent memory session-state record.
    async fn upsert_memory_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, StorageError> {
        let _ = record;
        Err(StorageError::Config(
            "persistent memory session state is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Get a persistent memory thread record by identifier.
    async fn get_memory_thread(
        &self,
        thread_id: String,
    ) -> Result<Option<ThreadRecord>, StorageError> {
        let _ = thread_id;
        Err(StorageError::Config(
            "persistent memory thread reads are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Get a persistent memory episode record by identifier.
    async fn get_memory_episode(
        &self,
        episode_id: String,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        let _ = episode_id;
        Err(StorageError::Config(
            "persistent memory episode reads are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// List persistent memory episodes for one thread.
    async fn list_memory_episodes_for_thread(
        &self,
        thread_id: String,
        filter: EpisodeListFilter,
    ) -> Result<Vec<EpisodeRecord>, StorageError> {
        let _ = thread_id;
        let _ = filter;
        Err(StorageError::Config(
            "persistent memory episode listing is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Get a reusable persistent memory record by identifier.
    async fn get_memory_record(
        &self,
        memory_id: String,
    ) -> Result<Option<MemoryRecord>, StorageError> {
        let _ = memory_id;
        Err(StorageError::Config(
            "reusable persistent memory reads are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// List reusable persistent memory records for one context.
    async fn list_memory_records(
        &self,
        context_key: String,
        filter: MemoryListFilter,
    ) -> Result<Vec<MemoryRecord>, StorageError> {
        let _ = context_key;
        let _ = filter;
        Err(StorageError::Config(
            "reusable persistent memory listing is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Search episode records lexically.
    async fn search_memory_episodes_lexical(
        &self,
        query: String,
        filter: EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, StorageError> {
        let _ = query;
        let _ = filter;
        Err(StorageError::Config(
            "persistent memory episode lexical search is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Search reusable memory records lexically.
    async fn search_memory_records_lexical(
        &self,
        query: String,
        filter: MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, StorageError> {
        let _ = query;
        let _ = filter;
        Err(StorageError::Config(
            "persistent memory record lexical search is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Read embedding state for one owner.
    async fn get_memory_embedding(
        &self,
        owner_type: EmbeddingOwnerType,
        owner_id: String,
    ) -> Result<Option<EmbeddingRecord>, StorageError> {
        let _ = owner_type;
        let _ = owner_id;
        Err(StorageError::Config(
            "persistent memory embeddings are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Mark one embedding row as pending generation.
    async fn upsert_memory_embedding_pending(
        &self,
        update: EmbeddingPendingUpdate,
    ) -> Result<EmbeddingRecord, StorageError> {
        let _ = update;
        Err(StorageError::Config(
            "persistent memory embedding pending writes are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Persist a successful embedding vector.
    async fn upsert_memory_embedding_ready(
        &self,
        update: EmbeddingReadyUpdate,
    ) -> Result<EmbeddingRecord, StorageError> {
        let _ = update;
        Err(StorageError::Config(
            "persistent memory embedding writes are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Persist a failed embedding attempt.
    async fn upsert_memory_embedding_failure(
        &self,
        update: EmbeddingFailureUpdate,
    ) -> Result<EmbeddingRecord, StorageError> {
        let _ = update;
        Err(StorageError::Config(
            "persistent memory embedding failure writes are not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Discover episode records that still need embeddings.
    async fn list_memory_episode_embedding_backfill_candidates(
        &self,
        request: EmbeddingBackfillRequest,
    ) -> Result<Vec<EpisodeEmbeddingCandidate>, StorageError> {
        let _ = request;
        Err(StorageError::Config(
            "persistent memory episode embedding backfill is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Discover memory records that still need embeddings.
    async fn list_memory_record_embedding_backfill_candidates(
        &self,
        request: EmbeddingBackfillRequest,
    ) -> Result<Vec<MemoryEmbeddingCandidate>, StorageError> {
        let _ = request;
        Err(StorageError::Config(
            "persistent memory record embedding backfill is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Vector search over episode records.
    async fn search_memory_episodes_vector(
        &self,
        query_embedding: Vec<f32>,
        filter: EpisodeSearchFilter,
    ) -> Result<Vec<EpisodeSearchHit>, StorageError> {
        let _ = query_embedding;
        let _ = filter;
        Err(StorageError::Config(
            "persistent memory episode vector search is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Vector search over reusable memory records.
    async fn search_memory_records_vector(
        &self,
        query_embedding: Vec<f32>,
        filter: MemorySearchFilter,
    ) -> Result<Vec<MemorySearchHit>, StorageError> {
        let _ = query_embedding;
        let _ = filter;
        Err(StorageError::Config(
            "persistent memory record vector search is not implemented for this storage provider"
                .to_string(),
        ))
    }
    /// Load archived or artifact text payload by storage key.
    async fn load_text_artifact(
        &self,
        storage_key: String,
    ) -> Result<Option<String>, StorageError> {
        let _ = storage_key;
        Err(StorageError::Config(
            "artifact text loading is not implemented for this storage provider".to_string(),
        ))
    }
    /// Clear all context (history and memory) for a user.
    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError>;
    /// Check connection to storage.
    async fn check_connection(&self) -> Result<(), String>;
    /// Get an agent profile record.
    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError>;
    /// Upsert an agent profile record.
    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError>;
    /// Delete an agent profile record.
    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError>;
    /// Get a topic context record.
    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(None)
    }
    /// Upsert a topic context record.
    async fn upsert_topic_context(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        let _ = options;
        Err(StorageError::Config(
            "topic context upsert is not implemented for this storage provider".to_string(),
        ))
    }
    /// Delete a topic context record.
    async fn delete_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(())
    }
    /// Get a topic-scoped `AGENTS.md` record.
    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(None)
    }
    /// Upsert a topic-scoped `AGENTS.md` record.
    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        let _ = options;
        Err(StorageError::Config(
            "topic AGENTS.md upsert is not implemented for this storage provider".to_string(),
        ))
    }
    /// Delete a topic-scoped `AGENTS.md` record.
    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(())
    }
    /// Get a topic infrastructure configuration record.
    async fn get_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(None)
    }
    /// Upsert a topic infrastructure configuration record.
    async fn upsert_topic_infra_config(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        let _ = options;
        Err(StorageError::Config(
            "topic infra config upsert is not implemented for this storage provider".to_string(),
        ))
    }
    /// Delete a topic infrastructure configuration record.
    async fn delete_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = topic_id;
        Ok(())
    }
    /// Resolve secret material from a private storage namespace.
    async fn get_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        let _ = user_id;
        let _ = secret_ref;
        Ok(None)
    }
    /// Persist secret material in a private storage namespace.
    async fn put_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = secret_ref;
        let _ = value;
        Err(StorageError::Config(
            "secret storage is not implemented for this storage provider".to_string(),
        ))
    }
    /// Delete secret material from a private storage namespace.
    async fn delete_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = secret_ref;
        Ok(())
    }
    /// Get a topic binding record.
    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError>;
    /// Upsert a topic binding record.
    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError>;
    /// Delete a topic binding record.
    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError>;
    /// Append an audit event to stream.
    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError>;
    /// List recent audit events for a user.
    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError>;
    /// List audit events page in descending version order.
    ///
    /// `before_version` acts as an exclusive cursor. When `None`, returns the latest page.
    async fn list_audit_events_page(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError>;
    /// Create a new reminder job.
    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        let _ = options;
        Err(StorageError::Config(
            "reminder job creation is not implemented for this storage provider".to_string(),
        ))
    }
    /// Get a reminder job by id.
    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        Ok(None)
    }
    /// List reminder jobs for a user with optional context and status filters.
    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = context_key;
        let _ = statuses;
        let _ = limit;
        Ok(Vec::new())
    }
    /// List reminder jobs that are due for execution.
    async fn list_due_reminder_jobs(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = now;
        let _ = limit;
        Ok(Vec::new())
    }
    /// Claim a due reminder job by assigning a temporary lease.
    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = lease_until;
        let _ = now;
        Ok(None)
    }
    /// Reschedule an existing reminder and clear any active lease.
    async fn reschedule_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        last_run_at: Option<i64>,
        last_error: Option<String>,
        increment_run_count: bool,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = next_run_at;
        let _ = last_run_at;
        let _ = last_error;
        let _ = increment_run_count;
        Ok(None)
    }
    /// Mark a reminder job as completed.
    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = completed_at;
        Ok(None)
    }
    /// Mark a reminder job as failed and stop future executions.
    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = failed_at;
        let _ = error;
        Ok(None)
    }
    /// Cancel an existing reminder job.
    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = cancelled_at;
        Ok(None)
    }
    /// Pause an active reminder job.
    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = paused_at;
        Ok(None)
    }
    /// Resume a paused reminder job with a new next execution timestamp.
    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = next_run_at;
        let _ = resumed_at;
        Ok(None)
    }
    /// Retry a failed reminder job by scheduling it again.
    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        let _ = next_run_at;
        let _ = retried_at;
        Ok(None)
    }
    /// Permanently delete a reminder job record.
    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        let _ = user_id;
        let _ = reminder_id;
        Err(StorageError::Config(
            "reminder job deletion is not implemented for this storage provider".to_string(),
        ))
    }
}
