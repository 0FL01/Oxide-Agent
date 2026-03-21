//! In-memory implementation of `StorageProvider` for E2E tests.
//!
//! Implements the subset of storage operations needed by Agent Mode:
//! - user configs
//! - chat history
//! - agent memory (legacy + context-scoped + flow-scoped)
//! - topic agents md (minimal)
//!
//! All other operations (reminders, audit, secrets, infra config, bindings) return
//! no-op defaults suitable for isolated E2E testing.

use async_trait::async_trait;
use oxide_agent_core::agent::AgentMemory;
use oxide_agent_core::storage::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, Message, ReminderJobRecord, ReminderJobStatus, StorageError,
    TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UserConfig,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// In-memory storage for E2E testing.
///
/// All data is held in memory and lost on process exit.
/// Thread-safe via `RwLock`.
pub struct InMemoryStorage {
    user_configs: RwLock<HashMap<i64, UserConfig>>,
    user_prompts: RwLock<HashMap<i64, String>>,
    user_models: RwLock<HashMap<i64, String>>,
    chat_histories: RwLock<HashMap<i64, Vec<Message>>>,
    chat_histories_by_chat: RwLock<HashMap<(i64, String), Vec<Message>>>,
    agent_memories: RwLock<HashMap<i64, AgentMemory>>,
    agent_memories_context: RwLock<HashMap<(i64, String), AgentMemory>>,
    agent_memories_flow: RwLock<HashMap<(i64, String, String), AgentMemory>>,
    flow_records: RwLock<HashMap<(i64, String, String), AgentFlowRecord>>,
    topic_agents_md: RwLock<HashMap<(i64, String), TopicAgentsMdRecord>>,
}

impl InMemoryStorage {
    /// Create a new empty in-memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_configs: RwLock::new(HashMap::new()),
            user_prompts: RwLock::new(HashMap::new()),
            user_models: RwLock::new(HashMap::new()),
            chat_histories: RwLock::new(HashMap::new()),
            chat_histories_by_chat: RwLock::new(HashMap::new()),
            agent_memories: RwLock::new(HashMap::new()),
            agent_memories_context: RwLock::new(HashMap::new()),
            agent_memories_flow: RwLock::new(HashMap::new()),
            flow_records: RwLock::new(HashMap::new()),
            topic_agents_md: RwLock::new(HashMap::new()),
        }
    }

    /// Create a storage shared as an `Arc<dyn StorageProvider>`.
    #[must_use]
    pub fn into_arc(self) -> Arc<dyn crate::api::StorageProvider> {
        Arc::new(self)
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl crate::api::StorageProvider for InMemoryStorage {
    // --- User config ---

    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        let configs = self.user_configs.read().await;
        Ok(configs.get(&user_id).cloned().unwrap_or_default())
    }

    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        let mut configs = self.user_configs.write().await;
        configs.insert(user_id, config);
        Ok(())
    }

    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        let mut prompts = self.user_prompts.write().await;
        prompts.insert(user_id, system_prompt);
        Ok(())
    }

    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let prompts = self.user_prompts.read().await;
        Ok(prompts.get(&user_id).cloned())
    }

    async fn update_user_model(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        let mut models = self.user_models.write().await;
        models.insert(user_id, model_name);
        Ok(())
    }

    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let models = self.user_models.read().await;
        Ok(models.get(&user_id).cloned())
    }

    // User state is intentionally noop — not needed for E2E.

    async fn update_user_state(&self, _user_id: i64, _state: String) -> Result<(), StorageError> {
        Ok(())
    }

    async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
        Ok(None)
    }

    // --- Chat history ---

    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let mut histories = self.chat_histories.write().await;
        let messages = histories.entry(user_id).or_insert_with(Vec::new);
        messages.push(Message { role, content });
        Ok(())
    }

    async fn get_chat_history(
        &self,
        user_id: i64,
        _limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let histories = self.chat_histories.read().await;
        Ok(histories.get(&user_id).cloned().unwrap_or_default())
    }

    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        let mut histories = self.chat_histories.write().await;
        histories.remove(&user_id);
        Ok(())
    }

    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let mut histories = self.chat_histories_by_chat.write().await;
        let messages = histories
            .entry((user_id, chat_uuid))
            .or_insert_with(Vec::new);
        messages.push(Message { role, content });
        Ok(())
    }

    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        _limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let histories = self.chat_histories_by_chat.read().await;
        Ok(histories
            .get(&(user_id, chat_uuid))
            .cloned()
            .unwrap_or_default())
    }

    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        let mut histories = self.chat_histories_by_chat.write().await;
        histories.remove(&(user_id, chat_uuid));
        Ok(())
    }

    // --- Agent memory: legacy ---

    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let mut memories = self.agent_memories.write().await;
        memories.insert(user_id, memory.clone());
        Ok(())
    }

    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        let memories = self.agent_memories.read().await;
        Ok(memories.get(&user_id).cloned())
    }

    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        let mut memories = self.agent_memories.write().await;
        memories.remove(&user_id);
        Ok(())
    }

    // --- Agent memory: context-scoped ---

    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let mut memories = self.agent_memories_context.write().await;
        memories.insert((user_id, context_key), memory.clone());
        Ok(())
    }

    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        let memories = self.agent_memories_context.read().await;
        Ok(memories.get(&(user_id, context_key)).cloned())
    }

    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let mut memories = self.agent_memories_context.write().await;
        memories.remove(&(user_id, context_key));
        Ok(())
    }

    // --- Agent memory: flow-scoped ---

    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        let mut memories = self.agent_memories_flow.write().await;
        memories.insert((user_id, context_key, flow_id), memory.clone());
        Ok(())
    }

    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        let memories = self.agent_memories_flow.read().await;
        Ok(memories.get(&(user_id, context_key, flow_id)).cloned())
    }

    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        let mut memories = self.agent_memories_flow.write().await;
        memories.remove(&(user_id, context_key, flow_id));
        Ok(())
    }

    // --- Flow records ---

    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        let records = self.flow_records.read().await;
        Ok(records.get(&(user_id, context_key, flow_id)).cloned())
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        let now = chrono::Utc::now().timestamp();
        let record = AgentFlowRecord {
            schema_version: 1,
            user_id,
            context_key: context_key.clone(),
            flow_id: flow_id.clone(),
            created_at: now,
            updated_at: now,
        };
        let mut records = self.flow_records.write().await;
        records.insert((user_id, context_key, flow_id), record.clone());
        Ok(record)
    }

    // --- Topic agents md ---

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        let records = self.topic_agents_md.read().await;
        Ok(records.get(&(user_id, topic_id)).cloned())
    }

    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        let mut records = self.topic_agents_md.write().await;
        let now = chrono::Utc::now().timestamp();
        let key = (options.user_id, options.topic_id.clone());
        let record = if let Some(existing) = records.get(&key) {
            TopicAgentsMdRecord {
                schema_version: existing.schema_version,
                version: existing.version + 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agents_md: options.agents_md,
                created_at: existing.created_at,
                updated_at: now,
            }
        } else {
            TopicAgentsMdRecord {
                schema_version: 1,
                version: 1,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agents_md: options.agents_md,
                created_at: now,
                updated_at: now,
            }
        };

        records.insert(key, record.clone());
        Ok(record)
    }

    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        let mut records = self.topic_agents_md.write().await;
        records.remove(&(user_id, topic_id));
        Ok(())
    }

    // --- System ---

    async fn clear_all_context(&self, user_id: i64) -> Result<(), StorageError> {
        let mut histories = self.chat_histories.write().await;
        histories.remove(&user_id);
        let mut memories = self.agent_memories.write().await;
        memories.remove(&user_id);
        Ok(())
    }

    async fn check_connection(&self) -> Result<(), String> {
        Ok(())
    }

    // --- Profile (noop for E2E) ---

    async fn get_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<Option<AgentProfileRecord>, StorageError> {
        Ok(None)
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        Ok(AgentProfileRecord {
            schema_version: 1,
            version: 1,
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn delete_agent_profile(
        &self,
        _user_id: i64,
        _agent_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    // --- Topic binding (noop for E2E) ---

    async fn get_topic_binding(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        Ok(None)
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        Ok(TopicBindingRecord {
            schema_version: 1,
            version: 1,
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
            chat_id: options.chat_id.for_new_record(),
            thread_id: options.thread_id.for_new_record(),
            expires_at: options.expires_at.for_new_record(),
            last_activity_at: options.last_activity_at,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn delete_topic_binding(
        &self,
        _user_id: i64,
        _topic_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }

    // --- Audit (noop for E2E) ---

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        Ok(AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: uuid::Uuid::new_v4().to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 0,
        })
    }

    async fn list_audit_events(
        &self,
        _user_id: i64,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        Ok(Vec::new())
    }

    async fn list_audit_events_page(
        &self,
        _user_id: i64,
        _before_version: Option<u64>,
        _limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        Ok(Vec::new())
    }

    // --- Reminder jobs (noop for E2E) ---

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        Ok(ReminderJobRecord {
            schema_version: 1,
            version: 1,
            reminder_id: uuid::Uuid::new_v4().to_string(),
            user_id: options.user_id,
            context_key: options.context_key,
            flow_id: options.flow_id,
            chat_id: options.chat_id,
            thread_id: options.thread_id,
            thread_kind: options.thread_kind,
            task_prompt: options.task_prompt,
            schedule_kind: options.schedule_kind,
            status: ReminderJobStatus::Scheduled,
            next_run_at: options.next_run_at,
            interval_secs: options.interval_secs,
            cron_expression: options.cron_expression,
            timezone: options.timezone,
            lease_until: None,
            last_run_at: None,
            last_error: None,
            run_count: 0,
            created_at: 0,
            updated_at: 0,
        })
    }

    async fn get_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn list_reminder_jobs(
        &self,
        _user_id: i64,
        _context_key: Option<String>,
        _statuses: Option<Vec<ReminderJobStatus>>,
        _limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        Ok(Vec::new())
    }

    async fn list_due_reminder_jobs(
        &self,
        _user_id: i64,
        _now: i64,
        _limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        Ok(Vec::new())
    }

    async fn claim_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _lease_until: i64,
        _now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn reschedule_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _next_run_at: i64,
        _last_run_at: Option<i64>,
        _last_error: Option<String>,
        _increment_run_count: bool,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn complete_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn fail_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _failed_at: i64,
        _error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn cancel_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn pause_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn resume_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _next_run_at: i64,
        _resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn retry_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
        _next_run_at: i64,
        _retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(None)
    }

    async fn delete_reminder_job(
        &self,
        _user_id: i64,
        _reminder_id: String,
    ) -> Result<(), StorageError> {
        Ok(())
    }
}
