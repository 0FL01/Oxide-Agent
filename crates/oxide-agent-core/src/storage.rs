//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

mod control_plane;
mod error;
mod flows;
mod keys;
mod provider;
mod r2_base;
mod reminder;
mod user;

pub use control_plane::{
    binding_is_active, resolve_active_topic_binding, AgentProfileRecord, AppendAuditEventOptions,
    AuditEventRecord, OptionalMetadataPatch, TopicAgentsMdRecord, TopicBindingKind,
    TopicBindingRecord, TopicContextRecord, TopicInfraAuthMode, TopicInfraConfigRecord,
    TopicInfraToolMode, UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions,
    UpsertTopicBindingOptions, UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
};
pub(crate) use control_plane::{
    normalize_topic_prompt_payload, validate_topic_agents_md_content,
    validate_topic_context_content,
};
pub(crate) const TOPIC_CONTEXT_MAX_LINES: usize = control_plane::TOPIC_CONTEXT_MAX_LINES;
pub(crate) const TOPIC_CONTEXT_MAX_CHARS: usize = control_plane::TOPIC_CONTEXT_MAX_CHARS;
#[cfg(test)]
pub(crate) const TOPIC_AGENTS_MD_MAX_LINES: usize = control_plane::TOPIC_AGENTS_MD_MAX_LINES;
pub use error::StorageError;
pub use flows::AgentFlowRecord;
pub use keys::{
    agent_profile_key, audit_events_key, generate_chat_uuid, private_secret_key, reminder_job_key,
    reminder_jobs_prefix, topic_agents_md_key, topic_binding_key, topic_context_key,
    topic_infra_config_key, user_agent_memory_key, user_chat_history_key, user_config_key,
    user_context_agent_flow_key, user_context_agent_flow_memory_key,
    user_context_agent_flow_prefix, user_context_agent_flows_prefix, user_context_agent_memory_key,
    user_context_chat_history_prefix, user_history_key,
};
#[cfg(test)]
pub use provider::MockStorageProvider;
pub use provider::StorageProvider;
pub use reminder::{
    compute_cron_next_run_at, compute_next_reminder_run_at, parse_reminder_timezone,
    CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind,
    ReminderThreadKind,
};
pub use user::{Message, UserConfig, UserContextConfig};

use self::keys::topic_prompt_guard_key;
use self::r2_base::{ControlPlaneLocks, TopicPromptStoreKind};

use crate::agent::memory::AgentMemory;
use async_trait::async_trait;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::Client;
use tracing::{error, info, warn};

use moka::future::Cache;
use std::sync::Arc;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::sleep;
use uuid::Uuid;

const AGENT_PROFILE_SCHEMA_VERSION: u32 = 1;
const TOPIC_CONTEXT_SCHEMA_VERSION: u32 = 1;
const TOPIC_AGENTS_MD_SCHEMA_VERSION: u32 = 1;
const TOPIC_INFRA_CONFIG_SCHEMA_VERSION: u32 = 1;
const AGENT_FLOW_SCHEMA_VERSION: u32 = 1;
const TOPIC_BINDING_SCHEMA_VERSION: u32 = 2;
const AUDIT_EVENT_SCHEMA_VERSION: u32 = 1;
const REMINDER_JOB_SCHEMA_VERSION: u32 = 2;
const CONTROL_PLANE_RMW_MAX_RETRIES: usize = 5;
const CONTROL_PLANE_RMW_RETRY_BACKOFF_MS: u64 = 25;

/// R2-backed storage implementation
pub struct R2Storage {
    client: Client,
    bucket: String,
    cache: Cache<String, Arc<Vec<u8>>>,
    control_plane_locks: ControlPlaneLocks,
}

#[async_trait]
impl StorageProvider for R2Storage {
    /// Get user configuration
    async fn get_user_config(&self, user_id: i64) -> Result<UserConfig, StorageError> {
        Ok(self
            .load_json(&user_config_key(user_id))
            .await?
            .unwrap_or_default())
    }

    /// Update user configuration
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        self.save_json(&user_config_key(user_id), &config).await
    }

    /// Update user system prompt
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.system_prompt = Some(system_prompt);
        })
        .await
    }

    /// Get user system prompt
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.system_prompt)
    }

    /// Update user model
    async fn update_user_model(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.model_name = Some(model_name);
        })
        .await
    }

    /// Get user model
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.model_name)
    }

    /// Update user state
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.state = Some(state);
        })
        .await
    }

    /// Get user state
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config(user_id).await?;
        Ok(config.state)
    }

    /// Save message to chat history
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_history_key(user_id);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    /// Get chat history for a user
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_history_key(user_id))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    /// Clear chat history for a user
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_history_key(user_id)).await
    }

    /// Save message to chat history for a specific chat UUID
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_chat_history_key(user_id, &chat_uuid);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    /// Get chat history for a specific chat UUID
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_chat_history_key(user_id, &chat_uuid))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    /// Clear chat history for a specific chat UUID
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&user_chat_history_key(user_id, &chat_uuid))
            .await
    }

    async fn clear_chat_history_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let prefix = user_context_chat_history_prefix(user_id, &context_key);
        self.delete_prefix(&prefix).await
    }

    /// Save agent memory to storage
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(&user_agent_memory_key(user_id), memory)
            .await
    }

    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(
            &user_context_agent_memory_key(user_id, &context_key),
            memory,
        )
        .await
    }

    /// Load agent memory from storage
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_agent_memory_key(user_id)).await
    }

    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_context_agent_memory_key(user_id, &context_key))
            .await
    }

    /// Clear agent memory for a user
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_agent_memory_key(user_id)).await
    }

    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.delete_prefix(&user_context_agent_flows_prefix(user_id, &context_key))
            .await?;
        self.delete_object(&user_context_agent_memory_key(user_id, &context_key))
            .await
    }

    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_json(
            &user_context_agent_flow_memory_key(user_id, &context_key, &flow_id),
            memory,
        )
        .await
    }

    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_json(&user_context_agent_flow_memory_key(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        self.delete_prefix(&user_context_agent_flow_prefix(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        self.load_json(&user_context_agent_flow_key(
            user_id,
            &context_key,
            &flow_id,
        ))
        .await
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        let key = user_context_agent_flow_key(user_id, &context_key, &flow_id);
        let now = current_timestamp_unix_secs();
        let existing = self.load_json::<AgentFlowRecord>(&key).await?;
        let record = build_agent_flow_record(user_id, context_key, flow_id, existing, now);
        self.save_json(&key, &record).await?;
        Ok(record)
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
        self.load_json(&agent_profile_key(user_id, &agent_id)).await
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        let key = agent_profile_key(options.user_id, &options.agent_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<AgentProfileRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_agent_profile_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "agent profile optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&agent_profile_key(user_id, &agent_id))
            .await
    }

    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        self.load_json(&topic_context_key(user_id, &topic_id)).await
    }

    async fn upsert_topic_context(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        let context = validate_topic_context_content(&options.context)?;
        let topic_prompt_guard_key = topic_prompt_guard_key(options.user_id, &options.topic_id);
        let _topic_prompt_guard = self
            .control_plane_locks
            .acquire(topic_prompt_guard_key)
            .await;
        let key = topic_context_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;
        self.ensure_topic_prompt_not_duplicated(
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::Context,
            &context,
        )
        .await?;
        let options = UpsertTopicContextOptions { context, ..options };

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<TopicContextRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_context_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic context optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn delete_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_context_key(user_id, &topic_id))
            .await
    }

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        self.load_json(&topic_agents_md_key(user_id, &topic_id))
            .await
    }

    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        let agents_md = validate_topic_agents_md_content(&options.agents_md)?;
        let topic_prompt_guard_key = topic_prompt_guard_key(options.user_id, &options.topic_id);
        let _topic_prompt_guard = self
            .control_plane_locks
            .acquire(topic_prompt_guard_key)
            .await;
        let key = topic_agents_md_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;
        self.ensure_topic_prompt_not_duplicated(
            options.user_id,
            &options.topic_id,
            TopicPromptStoreKind::AgentsMd,
            &agents_md,
        )
        .await?;
        let options = UpsertTopicAgentsMdOptions {
            agents_md,
            ..options
        };

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self
                .load_json_with_etag::<TopicAgentsMdRecord>(&key)
                .await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_agents_md_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic AGENTS.md optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_agents_md_key(user_id, &topic_id))
            .await
    }

    async fn get_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        self.load_json(&topic_infra_config_key(user_id, &topic_id))
            .await
    }

    async fn upsert_topic_infra_config(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        let key = topic_infra_config_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self
                .load_json_with_etag::<TopicInfraConfigRecord>(&key)
                .await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_infra_config_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic infra config optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn delete_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_infra_config_key(user_id, &topic_id))
            .await
    }

    async fn get_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        self.load_text(&private_secret_key(user_id, &secret_ref))
            .await
    }

    async fn put_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        self.save_text(&private_secret_key(user_id, &secret_ref), &value)
            .await
    }

    async fn delete_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&private_secret_key(user_id, &secret_ref))
            .await
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        self.load_json(&topic_binding_key(user_id, &topic_id)).await
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        let key = topic_binding_key(options.user_id, &options.topic_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (existing, etag) = self.load_json_with_etag::<TopicBindingRecord>(&key).await?;
            let now = current_timestamp_unix_secs();
            let record = build_topic_binding_record(options.clone(), existing, now);

            if self
                .save_json_conditionally(&key, &record, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "topic binding optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&topic_binding_key(user_id, &topic_id))
            .await
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        let key = audit_events_key(options.user_id);
        let _lock_guard = self.control_plane_locks.acquire(key.clone()).await;

        for attempt in 1..=CONTROL_PLANE_RMW_MAX_RETRIES {
            let (current_events, etag) = self
                .load_json_with_etag::<Vec<AuditEventRecord>>(&key)
                .await?;
            let mut events = current_events.unwrap_or_default();
            let now = current_timestamp_unix_secs();
            let record = build_audit_event_record(
                options.clone(),
                events.last().map(|event| event.version),
                now,
                Uuid::new_v4().to_string(),
            );

            events.push(record.clone());
            if self
                .save_json_conditionally(&key, &events, etag.as_deref())
                .await?
            {
                return Ok(record);
            }

            if should_retry_control_plane_rmw(attempt) {
                warn!(
                    key = %key,
                    attempt,
                    "audit stream optimistic concurrency conflict, retrying"
                );
                sleep(Duration::from_millis(
                    CONTROL_PLANE_RMW_RETRY_BACKOFF_MS * attempt as u64,
                ))
                .await;
            }
        }

        Err(StorageError::ConcurrencyConflict {
            key,
            attempts: CONTROL_PLANE_RMW_MAX_RETRIES,
        })
    }

    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let events: Vec<AuditEventRecord> = self
            .load_json(&audit_events_key(user_id))
            .await?
            .unwrap_or_default();
        let start = events.len().saturating_sub(limit);
        Ok(events[start..].to_vec())
    }

    async fn list_audit_events_page(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        let events: Vec<AuditEventRecord> = self
            .load_json(&audit_events_key(user_id))
            .await?
            .unwrap_or_default();

        Ok(select_audit_events_page(events, before_version, limit))
    }

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        let reminder_id = Uuid::new_v4().to_string();
        let key = reminder_job_key(options.user_id, &reminder_id);
        let now = current_timestamp_unix_secs();
        let record = build_reminder_job_record(options, reminder_id, now);
        self.save_json(&key, &record).await?;
        Ok(record)
    }

    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.load_json(&reminder_job_key(user_id, &reminder_id))
            .await
    }

    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        let mut records = self
            .list_json_under_prefix::<ReminderJobRecord>(&reminder_jobs_prefix(user_id))
            .await?;

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
            .list_json_under_prefix::<ReminderJobRecord>(&reminder_jobs_prefix(user_id))
            .await?;
        records.retain(|record| record.is_due(now));
        records.sort_by(|left, right| {
            left.next_run_at
                .cmp(&right.next_run_at)
                .then_with(|| left.created_at.cmp(&right.created_at))
        });
        if records.len() > limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if !record.is_due(now) {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                lease_until: Some(lease_until),
                updated_at: mutation_now,
                ..record
            })
        })
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
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            let run_count = if increment_run_count {
                record.run_count.saturating_add(1)
            } else {
                record.run_count
            };
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: last_run_at.or(record.last_run_at),
                last_error: last_error.clone(),
                run_count,
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Completed,
                lease_until: None,
                last_run_at: Some(completed_at),
                last_error: None,
                run_count: record.run_count.saturating_add(1),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Failed,
                lease_until: None,
                last_run_at: Some(failed_at),
                last_error: Some(error.clone()),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Cancelled,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(cancelled_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Scheduled {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Paused,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(paused_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Paused {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(resumed_at)),
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.mutate_reminder_job(user_id, &reminder_id, move |record, mutation_now| {
            if record.status != ReminderJobStatus::Failed {
                return None;
            }
            Some(ReminderJobRecord {
                version: with_next_reminder_version(&record),
                status: ReminderJobStatus::Scheduled,
                next_run_at,
                lease_until: None,
                last_run_at: record.last_run_at.or(Some(retried_at)),
                last_error: None,
                updated_at: mutation_now,
                ..record
            })
        })
        .await
    }

    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&reminder_job_key(user_id, &reminder_id))
            .await
    }
}

#[must_use]
fn select_audit_events_page(
    events: Vec<AuditEventRecord>,
    before_version: Option<u64>,
    limit: usize,
) -> Vec<AuditEventRecord> {
    events
        .into_iter()
        .rev()
        .filter(|event| before_version.is_none_or(|cursor| event.version < cursor))
        .take(limit)
        .collect()
}

#[must_use]
fn build_agent_profile_record(
    options: UpsertAgentProfileOptions,
    existing: Option<AgentProfileRecord>,
    now: i64,
) -> AgentProfileRecord {
    match existing {
        Some(existing_record) => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => AgentProfileRecord {
            schema_version: AGENT_PROFILE_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            agent_id: options.agent_id,
            profile: options.profile,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_topic_context_record(
    options: UpsertTopicContextOptions,
    existing: Option<TopicContextRecord>,
    now: i64,
) -> TopicContextRecord {
    match existing {
        Some(existing_record) => TopicContextRecord {
            schema_version: TOPIC_CONTEXT_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            context: options.context,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicContextRecord {
            schema_version: TOPIC_CONTEXT_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            context: options.context,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_topic_agents_md_record(
    options: UpsertTopicAgentsMdOptions,
    existing: Option<TopicAgentsMdRecord>,
    now: i64,
) -> TopicAgentsMdRecord {
    match existing {
        Some(existing_record) => TopicAgentsMdRecord {
            schema_version: TOPIC_AGENTS_MD_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agents_md: options.agents_md,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicAgentsMdRecord {
            schema_version: TOPIC_AGENTS_MD_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agents_md: options.agents_md,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_topic_infra_config_record(
    options: UpsertTopicInfraConfigOptions,
    existing: Option<TopicInfraConfigRecord>,
    now: i64,
) -> TopicInfraConfigRecord {
    match existing {
        Some(existing_record) => TopicInfraConfigRecord {
            schema_version: TOPIC_INFRA_CONFIG_SCHEMA_VERSION,
            version: next_record_version(Some(existing_record.version)),
            user_id: options.user_id,
            topic_id: options.topic_id,
            target_name: options.target_name,
            host: options.host,
            port: options.port,
            remote_user: options.remote_user,
            auth_mode: options.auth_mode,
            secret_ref: options.secret_ref,
            sudo_secret_ref: options.sudo_secret_ref,
            environment: options.environment,
            tags: options.tags,
            allowed_tool_modes: options.allowed_tool_modes,
            approval_required_modes: options.approval_required_modes,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => TopicInfraConfigRecord {
            schema_version: TOPIC_INFRA_CONFIG_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            target_name: options.target_name,
            host: options.host,
            port: options.port,
            remote_user: options.remote_user,
            auth_mode: options.auth_mode,
            secret_ref: options.secret_ref,
            sudo_secret_ref: options.sudo_secret_ref,
            environment: options.environment,
            tags: options.tags,
            allowed_tool_modes: options.allowed_tool_modes,
            approval_required_modes: options.approval_required_modes,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_topic_binding_record(
    options: UpsertTopicBindingOptions,
    existing: Option<TopicBindingRecord>,
    now: i64,
) -> TopicBindingRecord {
    match existing {
        Some(existing_record) => {
            let binding_kind = options.binding_kind.unwrap_or(existing_record.binding_kind);
            let chat_id = options.chat_id.apply(existing_record.chat_id);
            let thread_id = options.thread_id.apply(existing_record.thread_id);
            let expires_at = options.expires_at.apply(existing_record.expires_at);
            let last_activity_at = Some(options.last_activity_at.unwrap_or(now));

            TopicBindingRecord {
                schema_version: TOPIC_BINDING_SCHEMA_VERSION,
                version: next_record_version(Some(existing_record.version)),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind,
                chat_id,
                thread_id,
                expires_at,
                last_activity_at,
                created_at: existing_record.created_at,
                updated_at: now,
            }
        }
        None => TopicBindingRecord {
            schema_version: TOPIC_BINDING_SCHEMA_VERSION,
            version: next_record_version(None),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            binding_kind: options.binding_kind.unwrap_or_default(),
            chat_id: options.chat_id.for_new_record(),
            thread_id: options.thread_id.for_new_record(),
            expires_at: options.expires_at.for_new_record(),
            last_activity_at: Some(options.last_activity_at.unwrap_or(now)),
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_agent_flow_record(
    user_id: i64,
    context_key: String,
    flow_id: String,
    existing: Option<AgentFlowRecord>,
    now: i64,
) -> AgentFlowRecord {
    match existing {
        Some(existing_record) => AgentFlowRecord {
            schema_version: AGENT_FLOW_SCHEMA_VERSION,
            user_id,
            context_key,
            flow_id,
            created_at: existing_record.created_at,
            updated_at: now,
        },
        None => AgentFlowRecord {
            schema_version: AGENT_FLOW_SCHEMA_VERSION,
            user_id,
            context_key,
            flow_id,
            created_at: now,
            updated_at: now,
        },
    }
}

#[must_use]
fn build_audit_event_record(
    options: AppendAuditEventOptions,
    current_version: Option<u64>,
    now: i64,
    event_id: String,
) -> AuditEventRecord {
    AuditEventRecord {
        schema_version: AUDIT_EVENT_SCHEMA_VERSION,
        version: next_record_version(current_version),
        event_id,
        user_id: options.user_id,
        topic_id: options.topic_id,
        agent_id: options.agent_id,
        action: options.action,
        payload: options.payload,
        created_at: now,
    }
}

#[must_use]
fn build_reminder_job_record(
    options: CreateReminderJobOptions,
    reminder_id: String,
    now: i64,
) -> ReminderJobRecord {
    ReminderJobRecord {
        schema_version: REMINDER_JOB_SCHEMA_VERSION,
        version: next_record_version(None),
        reminder_id,
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
    }
}

#[must_use]
fn with_next_reminder_version(record: &ReminderJobRecord) -> u64 {
    next_record_version(Some(record.version))
}

#[must_use]
fn next_record_version(current_version: Option<u64>) -> u64 {
    match current_version {
        Some(version) => version.saturating_add(1),
        None => 1,
    }
}

#[must_use]
fn should_retry_control_plane_rmw(attempt: usize) -> bool {
    attempt < CONTROL_PLANE_RMW_MAX_RETRIES
}

#[must_use]
fn current_timestamp_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

#[must_use]
fn is_precondition_failed_put_error(err: &SdkError<PutObjectError>) -> bool {
    match err {
        SdkError::ServiceError(service_err) => service_err.raw().status().as_u16() == 412,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        agent_profile_key, audit_events_key, binding_is_active, build_agent_flow_record,
        build_agent_profile_record, build_audit_event_record, build_topic_agents_md_record,
        build_topic_binding_record, build_topic_context_record, build_topic_infra_config_record,
        compute_cron_next_run_at, compute_next_reminder_run_at, generate_chat_uuid,
        next_record_version, normalize_topic_prompt_payload, parse_reminder_timezone,
        private_secret_key, resolve_active_topic_binding, select_audit_events_page,
        should_retry_control_plane_rmw, topic_agents_md_key, topic_binding_key, topic_context_key,
        topic_infra_config_key, user_chat_history_key, user_config_key,
        user_context_agent_flow_key, user_context_agent_flow_memory_key,
        user_context_agent_flows_prefix, user_context_agent_memory_key,
        user_context_chat_history_prefix, user_history_key, validate_topic_agents_md_content,
        validate_topic_context_content, AgentFlowRecord, AgentProfileRecord,
        AppendAuditEventOptions, AuditEventRecord, ControlPlaneLocks, OptionalMetadataPatch,
        ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
        TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord, TopicContextRecord,
        TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode, UpsertAgentProfileOptions,
        UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UpsertTopicContextOptions,
        UpsertTopicInfraConfigOptions, UserConfig, UserContextConfig, TOPIC_AGENTS_MD_MAX_LINES,
        TOPIC_CONTEXT_MAX_CHARS, TOPIC_CONTEXT_MAX_LINES,
    };
    use chrono::TimeZone;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tokio::time::timeout;
    use uuid::Uuid;

    #[test]
    fn user_chat_history_key_uses_chat_uuid_namespace() {
        let key = user_chat_history_key(42, "chat-123");
        assert_eq!(key, "users/42/chats/chat-123/history.json");
    }

    #[test]
    fn legacy_user_history_key_stays_unchanged() {
        let key = user_history_key(42);
        assert_eq!(key, "users/42/history.json");
    }

    #[test]
    fn user_chat_history_key_isolated_by_user_and_chat_uuid() {
        let key_a = user_chat_history_key(1, "chat-a");
        let key_b = user_chat_history_key(1, "chat-b");
        let key_c = user_chat_history_key(2, "chat-a");

        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
        assert_ne!(key_b, key_c);
    }

    #[test]
    fn user_context_agent_memory_key_uses_topic_namespace() {
        let key = user_context_agent_memory_key(42, "-1001:77");
        assert_eq!(key, "users/42/topics/-1001:77/agent_memory.json");
    }

    #[test]
    fn user_context_agent_flows_prefix_uses_topic_namespace() {
        let prefix = user_context_agent_flows_prefix(42, "-1001:77");
        assert_eq!(prefix, "users/42/topics/-1001:77/flows/");
    }

    #[test]
    fn user_context_agent_flow_key_uses_flow_namespace() {
        let key = user_context_agent_flow_key(42, "-1001:77", "flow-123");
        assert_eq!(key, "users/42/topics/-1001:77/flows/flow-123/meta.json");
    }

    #[test]
    fn user_context_agent_flow_memory_key_uses_flow_namespace() {
        let key = user_context_agent_flow_memory_key(42, "-1001:77", "flow-123");
        assert_eq!(key, "users/42/topics/-1001:77/flows/flow-123/memory.json");
    }

    #[test]
    fn user_context_chat_history_prefix_uses_topic_namespace() {
        let prefix = user_context_chat_history_prefix(42, "-1001:77");
        assert_eq!(prefix, "users/42/chats/-1001:77/");
    }

    #[test]
    fn generate_chat_uuid_returns_v4_uuid() {
        let chat_uuid = generate_chat_uuid();
        let parsed = Uuid::parse_str(&chat_uuid);
        assert!(parsed.is_ok());
        let version = parsed.map(|uuid| uuid.get_version_num());
        assert_eq!(version, Ok(4));
    }

    #[test]
    fn user_config_deserializes_without_current_chat_uuid() {
        let json = r#"{
            "system_prompt": "You are helpful",
            "model_name": "gpt",
            "state": "idle"
        }"#;

        let parsed: Result<UserConfig, serde_json::Error> = serde_json::from_str(json);
        assert!(parsed.is_ok());
        let config = parsed.ok();
        assert!(config.is_some());
        assert_eq!(config.and_then(|cfg| cfg.current_chat_uuid), None);
    }

    #[test]
    fn user_config_roundtrip_preserves_current_chat_uuid() {
        let config = UserConfig {
            system_prompt: Some("You are helpful".to_string()),
            model_name: Some("gpt".to_string()),
            state: Some("chat_mode".to_string()),
            current_chat_uuid: Some("123e4567-e89b-12d3-a456-426614174000".to_string()),
            contexts: HashMap::new(),
        };

        let json = serde_json::to_string(&config);
        assert!(json.is_ok());

        let parsed: Result<UserConfig, serde_json::Error> =
            serde_json::from_str(&json.unwrap_or_default());
        assert!(parsed.is_ok());

        let parsed = parsed.unwrap_or_default();
        assert_eq!(
            parsed.current_chat_uuid,
            Some("123e4567-e89b-12d3-a456-426614174000".to_string())
        );
    }

    #[test]
    fn user_config_roundtrip_preserves_context_scoped_metadata() {
        let mut contexts = HashMap::new();
        contexts.insert(
            "-1001:42".to_string(),
            UserContextConfig {
                state: Some("agent_mode".to_string()),
                current_chat_uuid: Some("chat-42".to_string()),
                current_agent_flow_id: Some("flow-42".to_string()),
                chat_id: Some(-1001),
                thread_id: Some(42),
                forum_topic_name: Some("Topic 42".to_string()),
                forum_topic_icon_color: Some(7_322_096),
                forum_topic_icon_custom_emoji_id: Some("emoji-42".to_string()),
                forum_topic_closed: true,
            },
        );
        let config = UserConfig {
            contexts,
            ..UserConfig::default()
        };

        let json = serde_json::to_string(&config).expect("config must encode");
        let parsed: UserConfig = serde_json::from_str(&json).expect("config must decode");

        assert_eq!(parsed.contexts.len(), 1);
        assert_eq!(
            parsed
                .contexts
                .get("-1001:42")
                .and_then(|context| context.state.as_deref()),
            Some("agent_mode")
        );
        assert_eq!(
            parsed
                .contexts
                .get("-1001:42")
                .and_then(|context| context.current_agent_flow_id.as_deref()),
            Some("flow-42")
        );
        assert_eq!(
            parsed
                .contexts
                .get("-1001:42")
                .and_then(|context| context.forum_topic_name.as_deref()),
            Some("Topic 42")
        );
        assert!(parsed
            .contexts
            .get("-1001:42")
            .is_some_and(|context| context.forum_topic_closed));
    }

    #[test]
    fn build_agent_flow_record_preserves_created_at() {
        let existing = AgentFlowRecord {
            schema_version: 1,
            user_id: 7,
            context_key: "topic-a".to_string(),
            flow_id: "flow-a".to_string(),
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_agent_flow_record(
            7,
            "topic-a".to_string(),
            "flow-a".to_string(),
            Some(existing),
            999,
        );

        assert_eq!(updated.schema_version, 1);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
    }

    #[test]
    fn user_config_key_stays_stable() {
        let key = user_config_key(42);
        assert_eq!(key, "users/42/config.json");
    }

    #[test]
    fn agent_profile_key_uses_control_plane_namespace() {
        let key = agent_profile_key(42, "agent-a");
        assert_eq!(key, "users/42/control_plane/agent_profiles/agent-a.json");
    }

    #[test]
    fn topic_binding_key_uses_control_plane_namespace() {
        let key = topic_binding_key(42, "topic-a");
        assert_eq!(key, "users/42/control_plane/topic_bindings/topic-a.json");
    }

    #[test]
    fn topic_context_key_uses_control_plane_namespace() {
        let key = topic_context_key(42, "topic-a");
        assert_eq!(key, "users/42/control_plane/topic_contexts/topic-a.json");
    }

    #[test]
    fn topic_agents_md_key_uses_control_plane_namespace() {
        let key = topic_agents_md_key(42, "topic-a");
        assert_eq!(key, "users/42/control_plane/topic_agents_md/topic-a.json");
    }

    #[test]
    fn topic_infra_config_key_uses_control_plane_namespace() {
        let key = topic_infra_config_key(42, "topic-a");
        assert_eq!(key, "users/42/control_plane/topic_infra/topic-a.json");
    }

    #[test]
    fn parse_reminder_timezone_defaults_to_utc() {
        let timezone = parse_reminder_timezone(None).expect("timezone should parse");
        assert_eq!(timezone.name(), "UTC");
    }

    #[test]
    fn compute_cron_next_run_at_uses_timezone() {
        let after = chrono::Utc
            .with_ymd_and_hms(2026, 6, 1, 6, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp();
        let next = compute_cron_next_run_at("0 0 9 * * * *", Some("Europe/Berlin"), after)
            .expect("cron should resolve");
        let expected = chrono::Utc
            .with_ymd_and_hms(2026, 6, 1, 7, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp();
        assert_eq!(next, expected);
    }

    #[test]
    fn compute_next_reminder_run_at_supports_cron_records() {
        let record = ReminderJobRecord {
            schema_version: 2,
            version: 1,
            reminder_id: "rem-1".to_string(),
            user_id: 1,
            context_key: "ctx".to_string(),
            flow_id: "flow".to_string(),
            chat_id: 1,
            thread_id: None,
            thread_kind: ReminderThreadKind::Dm,
            task_prompt: "Ping".to_string(),
            schedule_kind: ReminderScheduleKind::Cron,
            status: ReminderJobStatus::Scheduled,
            next_run_at: 0,
            interval_secs: None,
            cron_expression: Some("0 0 9 * * * *".to_string()),
            timezone: Some("UTC".to_string()),
            lease_until: None,
            last_run_at: None,
            last_error: None,
            run_count: 0,
            created_at: 0,
            updated_at: 0,
        };
        let after = chrono::Utc
            .with_ymd_and_hms(2026, 6, 1, 8, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp();

        let next = compute_next_reminder_run_at(&record, after).expect("next run should compute");
        let expected = chrono::Utc
            .with_ymd_and_hms(2026, 6, 1, 9, 0, 0)
            .single()
            .expect("valid datetime")
            .timestamp();
        assert_eq!(next, Some(expected));
    }

    #[test]
    fn private_secret_key_uses_private_namespace() {
        let key = private_secret_key(42, "ssh/prod-key");
        assert_eq!(key, "users/42/private/secrets/ssh/prod-key");
    }

    #[test]
    fn audit_events_key_uses_control_plane_namespace() {
        let key = audit_events_key(42);
        assert_eq!(key, "users/42/control_plane/audit/events.json");
    }

    #[test]
    fn normalize_topic_prompt_payload_normalizes_line_endings_and_trailing_spaces() {
        let normalized = normalize_topic_prompt_payload("  line 1  \r\nline 2\t\r\n\r\n");
        assert_eq!(normalized, "line 1\nline 2");
    }

    #[test]
    fn validate_topic_context_rejects_markdown_documents() {
        let error = validate_topic_context_content("# AGENTS\nDo the thing")
            .expect_err("markdown document must be rejected");
        assert!(error
            .to_string()
            .contains("store AGENTS.md-style documents in topic_agents_md"));
    }

    #[test]
    fn validate_topic_context_rejects_oversized_payload() {
        let oversized = vec!["line"; TOPIC_CONTEXT_MAX_LINES + 1].join("\n");
        let error = validate_topic_context_content(&oversized)
            .expect_err("oversized topic context must be rejected");
        assert!(error.to_string().contains(&format!(
            "context must not exceed {TOPIC_CONTEXT_MAX_LINES} lines"
        )));
    }

    #[test]
    fn validate_topic_context_rejects_too_many_characters() {
        let oversized = "x".repeat(TOPIC_CONTEXT_MAX_CHARS + 1);
        let error = validate_topic_context_content(&oversized)
            .expect_err("oversized topic context must be rejected");
        assert!(error.to_string().contains(&format!(
            "context must not exceed {TOPIC_CONTEXT_MAX_CHARS} characters"
        )));
    }

    #[test]
    fn validate_topic_agents_md_normalizes_payload() {
        let normalized =
            validate_topic_agents_md_content("\r\n# Topic AGENTS  \r\nUse checklist\r\n")
                .expect("agents md must normalize");
        assert_eq!(normalized, "# Topic AGENTS\nUse checklist");
    }

    #[test]
    fn validate_topic_agents_md_rejects_oversized_payload() {
        let oversized = vec!["line"; TOPIC_AGENTS_MD_MAX_LINES + 1].join("\n");
        let error = validate_topic_agents_md_content(&oversized)
            .expect_err("oversized agents md must be rejected");
        assert!(error.to_string().contains(&format!(
            "agents_md must not exceed {TOPIC_AGENTS_MD_MAX_LINES} lines"
        )));
    }

    #[test]
    fn next_record_version_starts_at_one() {
        assert_eq!(next_record_version(None), 1);
    }

    #[test]
    fn next_record_version_increments_existing_value() {
        assert_eq!(next_record_version(Some(7)), 8);
    }

    #[test]
    fn next_record_version_saturates_on_overflow_boundary() {
        assert_eq!(next_record_version(Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn upsert_agent_profile_increments_version_and_preserves_created_at() {
        let existing = AgentProfileRecord {
            schema_version: 1,
            version: 3,
            user_id: 7,
            agent_id: "agent-a".to_string(),
            profile: json!({"name": "before"}),
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_agent_profile_record(
            UpsertAgentProfileOptions {
                user_id: 7,
                agent_id: "agent-a".to_string(),
                profile: json!({"name": "after"}),
            },
            Some(existing),
            999,
        );

        assert_eq!(updated.version, 4);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
    }

    #[test]
    fn upsert_agent_profile_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_agent_profile_record(
            UpsertAgentProfileOptions {
                user_id: 7,
                agent_id: "agent-a".to_string(),
                profile: json!({"name": "new"}),
            },
            None,
            777,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 777);
        assert_eq!(created.updated_at, 777);
    }

    #[test]
    fn upsert_topic_context_increments_version_and_preserves_created_at() {
        let existing = TopicContextRecord {
            schema_version: 1,
            version: 3,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            context: "before".to_string(),
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_topic_context_record(
            UpsertTopicContextOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                context: "after".to_string(),
            },
            Some(existing),
            999,
        );

        assert_eq!(updated.version, 4);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
        assert_eq!(updated.context, "after");
    }

    #[test]
    fn upsert_topic_context_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_topic_context_record(
            UpsertTopicContextOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                context: "topic instructions".to_string(),
            },
            None,
            777,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 777);
        assert_eq!(created.updated_at, 777);
        assert_eq!(created.schema_version, 1);
    }

    #[test]
    fn upsert_topic_agents_md_increments_version_and_preserves_created_at() {
        let existing = TopicAgentsMdRecord {
            schema_version: 1,
            version: 3,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agents_md: "before".to_string(),
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_topic_agents_md_record(
            UpsertTopicAgentsMdOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agents_md: "after".to_string(),
            },
            Some(existing),
            999,
        );

        assert_eq!(updated.version, 4);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
        assert_eq!(updated.agents_md, "after");
    }

    #[test]
    fn upsert_topic_agents_md_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_topic_agents_md_record(
            UpsertTopicAgentsMdOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agents_md: "# Topic agent instructions".to_string(),
            },
            None,
            777,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 777);
        assert_eq!(created.updated_at, 777);
        assert_eq!(created.schema_version, 1);
    }

    #[test]
    fn upsert_topic_infra_config_increments_version_and_preserves_created_at() {
        let existing = TopicInfraConfigRecord {
            schema_version: 1,
            version: 2,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            target_name: "prod-app".to_string(),
            host: "prod.example.com".to_string(),
            port: 22,
            remote_user: "deploy".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            secret_ref: Some("storage:ssh/prod-key".to_string()),
            sudo_secret_ref: Some("storage:ssh/prod-sudo".to_string()),
            environment: Some("prod".to_string()),
            tags: vec!["prod".to_string()],
            allowed_tool_modes: vec![TopicInfraToolMode::Exec],
            approval_required_modes: vec![TopicInfraToolMode::SudoExec],
            created_at: 123,
            updated_at: 124,
        };

        let updated = build_topic_infra_config_record(
            UpsertTopicInfraConfigOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                target_name: "prod-app-new".to_string(),
                host: "prod2.example.com".to_string(),
                port: 2222,
                remote_user: "ops".to_string(),
                auth_mode: TopicInfraAuthMode::Password,
                secret_ref: Some("env:SSH_PASSWORD".to_string()),
                sudo_secret_ref: None,
                environment: Some("prod".to_string()),
                tags: vec!["prod".to_string(), "critical".to_string()],
                allowed_tool_modes: vec![TopicInfraToolMode::Exec, TopicInfraToolMode::ReadFile],
                approval_required_modes: vec![TopicInfraToolMode::Exec],
            },
            Some(existing),
            999,
        );

        assert_eq!(updated.version, 3);
        assert_eq!(updated.created_at, 123);
        assert_eq!(updated.updated_at, 999);
        assert_eq!(updated.target_name, "prod-app-new");
        assert_eq!(updated.port, 2222);
    }

    #[test]
    fn upsert_topic_infra_config_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_topic_infra_config_record(
            UpsertTopicInfraConfigOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                target_name: "stage-app".to_string(),
                host: "stage.example.com".to_string(),
                port: 22,
                remote_user: "deploy".to_string(),
                auth_mode: TopicInfraAuthMode::PrivateKey,
                secret_ref: Some("storage:ssh/stage-key".to_string()),
                sudo_secret_ref: None,
                environment: Some("stage".to_string()),
                tags: vec!["stage".to_string()],
                allowed_tool_modes: vec![TopicInfraToolMode::Exec],
                approval_required_modes: vec![TopicInfraToolMode::SudoExec],
            },
            None,
            777,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 777);
        assert_eq!(created.updated_at, 777);
        assert_eq!(created.schema_version, 1);
    }

    #[test]
    fn upsert_topic_binding_increments_version_and_preserves_created_at() {
        let existing = TopicBindingRecord {
            schema_version: 1,
            version: 8,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: TopicBindingKind::Manual,
            chat_id: Some(100),
            thread_id: Some(7),
            expires_at: Some(10_000),
            last_activity_at: Some(501),
            created_at: 500,
            updated_at: 501,
        };

        let updated = build_topic_binding_record(
            UpsertTopicBindingOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-b".to_string(),
                binding_kind: None,
                chat_id: OptionalMetadataPatch::Keep,
                thread_id: OptionalMetadataPatch::Keep,
                expires_at: OptionalMetadataPatch::Keep,
                last_activity_at: None,
            },
            Some(existing),
            1_000,
        );

        assert_eq!(updated.version, 9);
        assert_eq!(updated.created_at, 500);
        assert_eq!(updated.updated_at, 1_000);
        assert_eq!(updated.agent_id, "agent-b");
        assert_eq!(updated.binding_kind, TopicBindingKind::Manual);
        assert_eq!(updated.chat_id, Some(100));
        assert_eq!(updated.thread_id, Some(7));
        assert_eq!(updated.expires_at, Some(10_000));
        assert_eq!(updated.last_activity_at, Some(1_000));
    }

    #[test]
    fn upsert_topic_binding_explicit_clear_resets_optional_metadata_fields() {
        let existing = TopicBindingRecord {
            schema_version: 1,
            version: 8,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: TopicBindingKind::Manual,
            chat_id: Some(100),
            thread_id: Some(7),
            expires_at: Some(10_000),
            last_activity_at: Some(501),
            created_at: 500,
            updated_at: 501,
        };

        let updated = build_topic_binding_record(
            UpsertTopicBindingOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-a".to_string(),
                binding_kind: None,
                chat_id: OptionalMetadataPatch::Clear,
                thread_id: OptionalMetadataPatch::Clear,
                expires_at: OptionalMetadataPatch::Clear,
                last_activity_at: None,
            },
            Some(existing),
            1_000,
        );

        assert_eq!(updated.chat_id, None);
        assert_eq!(updated.thread_id, None);
        assert_eq!(updated.expires_at, None);
    }

    #[test]
    fn upsert_topic_binding_initial_insert_starts_version_and_sets_timestamps() {
        let created = build_topic_binding_record(
            UpsertTopicBindingOptions {
                user_id: 7,
                topic_id: "topic-a".to_string(),
                agent_id: "agent-a".to_string(),
                binding_kind: Some(TopicBindingKind::Runtime),
                chat_id: OptionalMetadataPatch::Set(42),
                thread_id: OptionalMetadataPatch::Set(99),
                expires_at: OptionalMetadataPatch::Set(2_100),
                last_activity_at: None,
            },
            None,
            2_000,
        );

        assert_eq!(created.version, 1);
        assert_eq!(created.created_at, 2_000);
        assert_eq!(created.updated_at, 2_000);
        assert_eq!(created.schema_version, 2);
        assert_eq!(created.binding_kind, TopicBindingKind::Runtime);
        assert_eq!(created.chat_id, Some(42));
        assert_eq!(created.thread_id, Some(99));
        assert_eq!(created.expires_at, Some(2_100));
        assert_eq!(created.last_activity_at, Some(2_000));
    }

    #[test]
    fn topic_binding_record_backward_compatible_deserialization_defaults_new_fields() {
        let raw = r#"{
            "schema_version": 1,
            "version": 3,
            "user_id": 7,
            "topic_id": "topic-a",
            "agent_id": "agent-a",
            "created_at": 100,
            "updated_at": 200
        }"#;

        let record: TopicBindingRecord =
            serde_json::from_str(raw).expect("record must deserialize");
        assert_eq!(record.binding_kind, TopicBindingKind::Manual);
        assert_eq!(record.chat_id, None);
        assert_eq!(record.thread_id, None);
        assert_eq!(record.expires_at, None);
        assert_eq!(record.last_activity_at, None);
    }

    #[test]
    fn topic_binding_record_roundtrip_preserves_runtime_metadata() {
        let record = TopicBindingRecord {
            schema_version: 2,
            version: 1,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: TopicBindingKind::Runtime,
            chat_id: Some(10),
            thread_id: Some(20),
            expires_at: Some(500),
            last_activity_at: Some(400),
            created_at: 100,
            updated_at: 200,
        };

        let encoded = serde_json::to_string(&record).expect("record must encode");
        let decoded_record: TopicBindingRecord =
            serde_json::from_str(&encoded).expect("roundtrip should decode");
        assert_eq!(decoded_record.binding_kind, TopicBindingKind::Runtime);
        assert_eq!(decoded_record.chat_id, Some(10));
        assert_eq!(decoded_record.thread_id, Some(20));
        assert_eq!(decoded_record.expires_at, Some(500));
        assert_eq!(decoded_record.last_activity_at, Some(400));
        assert_eq!(decoded_record.schema_version, 2);
    }

    #[test]
    fn binding_activity_helper_distinguishes_active_and_expired_records() {
        let active_record = TopicBindingRecord {
            schema_version: 2,
            version: 1,
            user_id: 7,
            topic_id: "topic-a".to_string(),
            agent_id: "agent-a".to_string(),
            binding_kind: TopicBindingKind::Runtime,
            chat_id: Some(10),
            thread_id: Some(20),
            expires_at: Some(500),
            last_activity_at: Some(450),
            created_at: 100,
            updated_at: 200,
        };
        let expired_record = TopicBindingRecord {
            expires_at: Some(300),
            ..active_record.clone()
        };

        assert!(binding_is_active(&active_record, 499));
        assert!(!binding_is_active(&expired_record, 300));
        assert!(resolve_active_topic_binding(Some(active_record), 499).is_some());
        assert!(resolve_active_topic_binding(Some(expired_record), 300).is_none());
    }

    #[test]
    fn append_audit_event_versions_are_monotonic() {
        let first = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-a".to_string()),
                action: "created".to_string(),
                payload: json!({"k": 1}),
            },
            None,
            10,
            "event-1".to_string(),
        );

        let second = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: Some("topic-a".to_string()),
                agent_id: Some("agent-a".to_string()),
                action: "updated".to_string(),
                payload: json!({"k": 2}),
            },
            Some(first.version),
            11,
            "event-2".to_string(),
        );

        assert_eq!(first.version, 1);
        assert_eq!(second.version, 2);
    }

    #[test]
    fn append_audit_event_version_saturates_at_upper_bound() {
        let event = build_audit_event_record(
            AppendAuditEventOptions {
                user_id: 9,
                topic_id: None,
                agent_id: None,
                action: "updated".to_string(),
                payload: json!({"k": 2}),
            },
            Some(u64::MAX),
            11,
            "event-2".to_string(),
        );

        assert_eq!(event.version, u64::MAX);
    }

    #[test]
    fn audit_page_cursor_returns_descending_window() {
        let events = vec![
            AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: 9,
                topic_id: None,
                agent_id: None,
                action: "a".to_string(),
                payload: json!({}),
                created_at: 1,
            },
            AuditEventRecord {
                schema_version: 1,
                version: 2,
                event_id: "evt-2".to_string(),
                user_id: 9,
                topic_id: None,
                agent_id: None,
                action: "b".to_string(),
                payload: json!({}),
                created_at: 2,
            },
            AuditEventRecord {
                schema_version: 1,
                version: 3,
                event_id: "evt-3".to_string(),
                user_id: 9,
                topic_id: None,
                agent_id: None,
                action: "c".to_string(),
                payload: json!({}),
                created_at: 3,
            },
        ];

        let first_page: Vec<u64> = select_audit_events_page(events.clone(), None, 2)
            .iter()
            .map(|event| event.version)
            .collect();
        let second_page: Vec<u64> = select_audit_events_page(events, Some(2), 2)
            .iter()
            .map(|event| event.version)
            .collect();

        assert_eq!(first_page, vec![3, 2]);
        assert_eq!(second_page, vec![1]);
    }

    #[test]
    fn control_plane_retry_policy_stops_at_max_attempt() {
        assert!(should_retry_control_plane_rmw(1));
        assert!(should_retry_control_plane_rmw(4));
        assert!(!should_retry_control_plane_rmw(5));
        assert!(!should_retry_control_plane_rmw(6));
    }

    #[tokio::test]
    async fn control_plane_lock_serializes_same_key_updates() {
        let locks = Arc::new(ControlPlaneLocks::new());
        let first_guard = locks
            .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
            .await;

        let locks_for_task = Arc::clone(&locks);
        let (tx, rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            let _second_guard = locks_for_task
                .acquire("users/7/control_plane/topic_bindings/topic-a.json".to_string())
                .await;
            let _ = tx.send(());
        });

        let blocked_result = timeout(Duration::from_millis(50), rx).await;
        assert!(blocked_result.is_err());

        drop(first_guard);

        let join_result = timeout(Duration::from_secs(1), join).await;
        assert!(join_result.is_ok());
    }
}
