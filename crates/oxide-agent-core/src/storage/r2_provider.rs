use super::telemetry::with_storage_reason;
use super::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, Message, R2Storage, ReminderJobRecord, ReminderJobStatus,
    StorageError, StorageProvider, TopicAgentsMdRecord, TopicBindingRecord, TopicContextRecord,
    TopicInfraConfigRecord, UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions,
    UpsertTopicBindingOptions, UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
    UserConfig,
};
use crate::agent::memory::AgentMemory;
use async_trait::async_trait;
use oxide_agent_memory::{EpisodeRecord, MemoryRecord, SessionStateRecord, ThreadRecord};
use tracing::{error, info};

#[async_trait]
impl StorageProvider for R2Storage {
    /// Get user configuration
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        with_storage_reason("get_user_config", self.get_user_config_inner(user_id)).await
    }

    /// Update user configuration
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "update_user_config",
            self.update_user_config_inner(user_id, config),
        )
        .await
    }

    /// Update user system prompt
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "update_user_prompt",
            self.update_user_prompt_inner(user_id, system_prompt),
        )
        .await
    }

    /// Get user system prompt
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        with_storage_reason("get_user_prompt", self.get_user_prompt_inner(user_id)).await
    }

    /// Update user model
    async fn update_user_model(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "update_user_model",
            self.update_user_model_inner(user_id, model_name),
        )
        .await
    }

    /// Get user model
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        with_storage_reason("get_user_model", self.get_user_model_inner(user_id)).await
    }

    /// Update user state
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        with_storage_reason(
            "update_user_state",
            self.update_user_state_inner(user_id, state),
        )
        .await
    }

    /// Get user state
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        with_storage_reason("get_user_state", self.get_user_state_inner(user_id)).await
    }

    /// Save message to chat history
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "save_message",
            self.save_message_inner(user_id, role, content),
        )
        .await
    }

    /// Get chat history for a user
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        with_storage_reason(
            "get_chat_history",
            self.get_chat_history_inner(user_id, limit),
        )
        .await
    }

    /// Clear chat history for a user
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        with_storage_reason("clear_chat_history", self.clear_chat_history_inner(user_id)).await
    }

    /// Save message to chat history for a specific chat UUID
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "save_message_for_chat",
            self.save_message_for_chat_inner(user_id, chat_uuid, role, content),
        )
        .await
    }

    /// Get chat history for a specific chat UUID
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        with_storage_reason(
            "get_chat_history_for_chat",
            self.get_chat_history_for_chat_inner(user_id, chat_uuid, limit),
        )
        .await
    }

    /// Clear chat history for a specific chat UUID
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "clear_chat_history_for_chat",
            self.clear_chat_history_for_chat_inner(user_id, chat_uuid),
        )
        .await
    }

    async fn clear_chat_history_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "clear_chat_history_for_context",
            self.clear_chat_history_for_context_inner(user_id, context_key),
        )
        .await
    }

    /// Save agent memory to storage
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "save_agent_memory",
            self.save_agent_memory_inner(user_id, memory),
        )
        .await
    }

    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "save_agent_memory_for_context",
            self.save_agent_memory_for_context_inner(user_id, context_key, memory),
        )
        .await
    }

    /// Load agent memory from storage
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        with_storage_reason("load_agent_memory", self.load_agent_memory_inner(user_id)).await
    }

    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        with_storage_reason(
            "load_agent_memory_for_context",
            self.load_agent_memory_for_context_inner(user_id, context_key),
        )
        .await
    }

    /// Clear agent memory for a user
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        with_storage_reason("clear_agent_memory", self.clear_agent_memory_inner(user_id)).await
    }

    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "clear_agent_memory_for_context",
            self.clear_agent_memory_for_context_inner(user_id, context_key),
        )
        .await
    }

    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "save_agent_memory_for_flow",
            self.save_agent_memory_for_flow_inner(user_id, context_key, flow_id, memory),
        )
        .await
    }

    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        with_storage_reason(
            "load_agent_memory_for_flow",
            self.load_agent_memory_for_flow_inner(user_id, context_key, flow_id),
        )
        .await
    }

    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "clear_agent_memory_for_flow",
            self.clear_agent_memory_for_flow_inner(user_id, context_key, flow_id),
        )
        .await
    }

    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        with_storage_reason(
            "get_agent_flow_record",
            self.get_agent_flow_record_inner(user_id, context_key, flow_id),
        )
        .await
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        with_storage_reason(
            "upsert_agent_flow_record",
            self.upsert_agent_flow_record_inner(user_id, context_key, flow_id),
        )
        .await
    }

    async fn upsert_memory_thread(
        &self,
        record: ThreadRecord,
    ) -> Result<ThreadRecord, StorageError> {
        with_storage_reason(
            "upsert_memory_thread",
            self.upsert_memory_thread_inner(record),
        )
        .await
    }

    async fn create_memory_episode(
        &self,
        record: EpisodeRecord,
    ) -> Result<EpisodeRecord, StorageError> {
        with_storage_reason(
            "create_memory_episode",
            self.create_memory_episode_inner(record),
        )
        .await
    }

    async fn create_memory_record(
        &self,
        record: MemoryRecord,
    ) -> Result<MemoryRecord, StorageError> {
        with_storage_reason(
            "create_memory_record",
            self.create_memory_record_inner(record),
        )
        .await
    }

    async fn link_memory_episode_artifact(
        &self,
        episode_id: String,
        artifact: oxide_agent_memory::ArtifactRef,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        with_storage_reason(
            "link_memory_episode_artifact",
            self.link_memory_episode_artifact_inner(episode_id, artifact),
        )
        .await
    }

    async fn upsert_memory_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, StorageError> {
        with_storage_reason(
            "upsert_memory_session_state",
            self.upsert_memory_session_state_inner(record),
        )
        .await
    }

    async fn get_memory_thread(
        &self,
        thread_id: String,
    ) -> Result<Option<ThreadRecord>, StorageError> {
        with_storage_reason("get_memory_thread", self.get_memory_thread_inner(thread_id)).await
    }

    async fn get_memory_episode(
        &self,
        episode_id: String,
    ) -> Result<Option<EpisodeRecord>, StorageError> {
        with_storage_reason(
            "get_memory_episode",
            self.get_memory_episode_inner(episode_id),
        )
        .await
    }

    async fn list_memory_episodes_for_thread(
        &self,
        thread_id: String,
        filter: oxide_agent_memory::EpisodeListFilter,
    ) -> Result<Vec<EpisodeRecord>, StorageError> {
        with_storage_reason(
            "list_memory_episodes_for_thread",
            self.list_memory_episodes_for_thread_inner(thread_id, filter),
        )
        .await
    }

    async fn get_memory_record(
        &self,
        memory_id: String,
    ) -> Result<Option<MemoryRecord>, StorageError> {
        with_storage_reason("get_memory_record", self.get_memory_record_inner(memory_id)).await
    }

    async fn list_memory_records(
        &self,
        context_key: String,
        filter: oxide_agent_memory::MemoryListFilter,
    ) -> Result<Vec<MemoryRecord>, StorageError> {
        with_storage_reason(
            "list_memory_records",
            self.list_memory_records_inner(context_key, filter),
        )
        .await
    }

    async fn search_memory_episodes_lexical(
        &self,
        query: String,
        filter: oxide_agent_memory::EpisodeSearchFilter,
    ) -> Result<Vec<oxide_agent_memory::EpisodeSearchHit>, StorageError> {
        with_storage_reason(
            "search_memory_episodes_lexical",
            self.search_memory_episodes_lexical_inner(query, filter),
        )
        .await
    }

    async fn search_memory_records_lexical(
        &self,
        query: String,
        filter: oxide_agent_memory::MemorySearchFilter,
    ) -> Result<Vec<oxide_agent_memory::MemorySearchHit>, StorageError> {
        with_storage_reason(
            "search_memory_records_lexical",
            self.search_memory_records_lexical_inner(query, filter),
        )
        .await
    }

    async fn load_text_artifact(
        &self,
        storage_key: String,
    ) -> Result<Option<String>, StorageError> {
        with_storage_reason("load_text_artifact", self.load_text(&storage_key)).await
    }

    /// Clear all context (history and memory) for a user
    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_chat_history(user_id).await?;
        self.clear_agent_memory(user_id).await?;
        Ok(())
    }

    /// Check connection to R2 storage
    async fn check_connection(&self) -> Result<(), String> {
        match self.client.list_buckets().send().await {
            Ok(_) => {
                info!("Successfully connected to R2 storage.");
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("R2 connectivity test failed: {e:#?}");
                error!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    async fn get_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        with_storage_reason(
            "get_agent_profile",
            self.get_agent_profile_inner(user_id, agent_id),
        )
        .await
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        with_storage_reason(
            "upsert_agent_profile",
            self.upsert_agent_profile_inner(options),
        )
        .await
    }

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_agent_profile",
            self.delete_agent_profile_inner(user_id, agent_id),
        )
        .await
    }

    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        with_storage_reason(
            "get_topic_context",
            self.get_topic_context_inner(user_id, topic_id),
        )
        .await
    }

    async fn upsert_topic_context(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        with_storage_reason(
            "upsert_topic_context",
            self.upsert_topic_context_inner(options),
        )
        .await
    }

    async fn delete_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_topic_context",
            self.delete_topic_context_inner(user_id, topic_id),
        )
        .await
    }

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        with_storage_reason(
            "get_topic_agents_md",
            self.get_topic_agents_md_inner(user_id, topic_id),
        )
        .await
    }

    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        with_storage_reason(
            "upsert_topic_agents_md",
            self.upsert_topic_agents_md_inner(options),
        )
        .await
    }

    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_topic_agents_md",
            self.delete_topic_agents_md_inner(user_id, topic_id),
        )
        .await
    }

    async fn get_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        with_storage_reason(
            "get_topic_infra_config",
            self.get_topic_infra_config_inner(user_id, topic_id),
        )
        .await
    }

    async fn upsert_topic_infra_config(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        with_storage_reason(
            "upsert_topic_infra_config",
            self.upsert_topic_infra_config_inner(options),
        )
        .await
    }

    async fn delete_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_topic_infra_config",
            self.delete_topic_infra_config_inner(user_id, topic_id),
        )
        .await
    }

    async fn get_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        with_storage_reason(
            "get_secret_value",
            self.get_secret_value_inner(user_id, secret_ref),
        )
        .await
    }

    async fn put_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "put_secret_value",
            self.put_secret_value_inner(user_id, secret_ref, value),
        )
        .await
    }

    async fn delete_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_secret_value",
            self.delete_secret_value_inner(user_id, secret_ref),
        )
        .await
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        with_storage_reason(
            "get_topic_binding",
            self.get_topic_binding_inner(user_id, topic_id),
        )
        .await
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        with_storage_reason(
            "upsert_topic_binding",
            self.upsert_topic_binding_inner(options),
        )
        .await
    }

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_topic_binding",
            self.delete_topic_binding_inner(user_id, topic_id),
        )
        .await
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        with_storage_reason("append_audit_event", self.append_audit_event_inner(options)).await
    }

    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        with_storage_reason(
            "list_audit_events",
            self.list_audit_events_inner(user_id, limit),
        )
        .await
    }

    async fn list_audit_events_page(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        with_storage_reason(
            "list_audit_events_page",
            self.list_audit_events_page_inner(user_id, before_version, limit),
        )
        .await
    }

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        with_storage_reason(
            "create_reminder_job",
            self.create_reminder_job_inner(options),
        )
        .await
    }

    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "get_reminder_job",
            self.get_reminder_job_inner(user_id, reminder_id),
        )
        .await
    }

    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "list_reminder_jobs",
            self.list_reminder_jobs_inner(user_id, context_key, statuses, limit),
        )
        .await
    }

    async fn list_due_reminder_jobs(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "list_due_reminder_jobs",
            self.list_due_reminder_jobs_inner(user_id, now, limit),
        )
        .await
    }

    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "claim_reminder_job",
            self.claim_reminder_job_inner(user_id, reminder_id, lease_until, now),
        )
        .await
    }

    async fn reschedule_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        last_run_at: Option<i64>,
        last_error: Option<String>,
        increment_run_count: bool,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "reschedule_reminder_job",
            self.reschedule_reminder_job_inner(
                user_id,
                reminder_id,
                next_run_at,
                last_run_at,
                last_error,
                increment_run_count,
            ),
        )
        .await
    }

    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "complete_reminder_job",
            self.complete_reminder_job_inner(user_id, reminder_id, completed_at),
        )
        .await
    }

    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "fail_reminder_job",
            self.fail_reminder_job_inner(user_id, reminder_id, failed_at, error),
        )
        .await
    }

    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "cancel_reminder_job",
            self.cancel_reminder_job_inner(user_id, reminder_id, cancelled_at),
        )
        .await
    }

    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "pause_reminder_job",
            self.pause_reminder_job_inner(user_id, reminder_id, paused_at),
        )
        .await
    }

    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "resume_reminder_job",
            self.resume_reminder_job_inner(user_id, reminder_id, next_run_at, resumed_at),
        )
        .await
    }

    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        with_storage_reason(
            "retry_reminder_job",
            self.retry_reminder_job_inner(user_id, reminder_id, next_run_at, retried_at),
        )
        .await
    }

    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        with_storage_reason(
            "delete_reminder_job",
            self.delete_reminder_job_inner(user_id, reminder_id),
        )
        .await
    }
}
