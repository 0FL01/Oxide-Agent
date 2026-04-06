//! In-memory implementation of `StorageProvider` for E2E tests.
//!
//! Implements the subset of storage operations needed by Agent Mode:
//! - user configs
//! - chat history
//! - agent memory (legacy + context-scoped + flow-scoped)
//! - topic agents md (minimal)
//! - reminder jobs used by reminder E2E tests
//!
//! All other operations (audit, secrets, infra config, bindings) return no-op
//! defaults suitable for isolated E2E testing.

use async_trait::async_trait;
use chrono::Utc;
use oxide_agent_core::agent::AgentMemory;
use oxide_agent_core::storage::{
    AgentFlowRecord, AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord,
    CreateReminderJobOptions, Message, ReminderJobRecord, ReminderJobStatus, StorageError,
    TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UserConfig,
};
use oxide_agent_memory::{EpisodeRecord, MemoryRecord, SessionStateRecord, ThreadRecord};
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
    memory_threads: RwLock<HashMap<String, ThreadRecord>>,
    memory_episodes: RwLock<HashMap<String, EpisodeRecord>>,
    memory_records: RwLock<HashMap<String, MemoryRecord>>,
    memory_session_states: RwLock<HashMap<String, SessionStateRecord>>,
    reminder_jobs: RwLock<HashMap<(i64, String), ReminderJobRecord>>,
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
            memory_threads: RwLock::new(HashMap::new()),
            memory_episodes: RwLock::new(HashMap::new()),
            memory_records: RwLock::new(HashMap::new()),
            memory_session_states: RwLock::new(HashMap::new()),
            reminder_jobs: RwLock::new(HashMap::new()),
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

    async fn upsert_memory_thread(
        &self,
        record: ThreadRecord,
    ) -> Result<ThreadRecord, StorageError> {
        let mut threads = self.memory_threads.write().await;
        let stored = if let Some(existing) = threads.get(&record.thread_id) {
            ThreadRecord {
                created_at: existing.created_at,
                ..record
            }
        } else {
            record
        };
        threads.insert(stored.thread_id.clone(), stored.clone());
        Ok(stored)
    }

    async fn create_memory_episode(
        &self,
        record: EpisodeRecord,
    ) -> Result<EpisodeRecord, StorageError> {
        let mut episodes = self.memory_episodes.write().await;
        if episodes.contains_key(&record.episode_id) {
            return Err(StorageError::InvalidInput(format!(
                "episode {} already exists",
                record.episode_id
            )));
        }
        episodes.insert(record.episode_id.clone(), record.clone());
        Ok(record)
    }

    async fn create_memory_record(
        &self,
        record: MemoryRecord,
    ) -> Result<MemoryRecord, StorageError> {
        let mut memories = self.memory_records.write().await;
        if memories.contains_key(&record.memory_id) {
            return Err(StorageError::InvalidInput(format!(
                "memory {} already exists",
                record.memory_id
            )));
        }
        memories.insert(record.memory_id.clone(), record.clone());
        Ok(record)
    }

    async fn upsert_memory_session_state(
        &self,
        record: SessionStateRecord,
    ) -> Result<SessionStateRecord, StorageError> {
        let mut session_states = self.memory_session_states.write().await;
        session_states.insert(record.session_id.clone(), record.clone());
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

    // --- Reminder jobs ---

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        let now = Utc::now().timestamp();
        let record = ReminderJobRecord {
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
            created_at: now,
            updated_at: now,
        };
        self.reminder_jobs
            .write()
            .await
            .insert((record.user_id, record.reminder_id.clone()), record.clone());
        Ok(record)
    }

    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        Ok(self
            .reminder_jobs
            .read()
            .await
            .get(&(user_id, reminder_id))
            .cloned())
    }

    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let mut records = self
            .reminder_jobs
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id)
            .cloned()
            .collect::<Vec<_>>();

        if let Some(context_key) = context_key.as_ref() {
            records.retain(|record| record.context_key == *context_key);
        }

        if let Some(statuses) = statuses.as_ref() {
            records.retain(|record| statuses.contains(&record.status));
        }

        records.sort_by(|left, right| {
            right
                .next_run_at
                .cmp(&left.next_run_at)
                .then_with(|| right.created_at.cmp(&left.created_at))
        });
        if records.len() > limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    async fn list_due_reminder_jobs(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let mut records = self
            .reminder_jobs
            .read()
            .await
            .values()
            .filter(|record| record.user_id == user_id && record.is_due(now))
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| {
            left.next_run_at
                .cmp(&right.next_run_at)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        records.truncate(limit);
        Ok(records)
    }

    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if !record.is_due(now) {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.lease_until = Some(lease_until);
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
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
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Scheduled {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.next_run_at = next_run_at;
        record.lease_until = None;
        record.last_run_at = last_run_at.or(record.last_run_at);
        record.last_error = last_error;
        if increment_run_count {
            record.run_count = record.run_count.saturating_add(1);
        }
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Scheduled {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Completed;
        record.lease_until = None;
        record.last_run_at = Some(completed_at);
        record.last_error = None;
        record.run_count = record.run_count.saturating_add(1);
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Scheduled {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Failed;
        record.lease_until = None;
        record.last_run_at = Some(failed_at);
        record.last_error = Some(error);
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Scheduled {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Cancelled;
        record.lease_until = None;
        record.last_run_at = record.last_run_at.or(Some(cancelled_at));
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Scheduled {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Paused;
        record.lease_until = None;
        record.last_run_at = record.last_run_at.or(Some(paused_at));
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Paused {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Scheduled;
        record.next_run_at = next_run_at;
        record.lease_until = None;
        record.last_run_at = record.last_run_at.or(Some(resumed_at));
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        let mut jobs = self.reminder_jobs.write().await;
        let Some(record) = jobs.get_mut(&(user_id, reminder_id)) else {
            return Ok(None);
        };
        if record.status != ReminderJobStatus::Failed {
            return Ok(None);
        }
        record.version = record.version.saturating_add(1);
        record.status = ReminderJobStatus::Scheduled;
        record.next_run_at = next_run_at;
        record.lease_until = None;
        record.last_run_at = record.last_run_at.or(Some(retried_at));
        record.last_error = None;
        record.updated_at = Utc::now().timestamp();
        Ok(Some(record.clone()))
    }

    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        self.reminder_jobs
            .write()
            .await
            .remove(&(user_id, reminder_id));
        Ok(())
    }
}
