//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

mod control_plane;
mod error;
mod flows;
mod keys;
mod provider;
mod r2_base;
mod r2_control_plane;
mod r2_memory;
mod r2_reminder;
mod r2_user;
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

use self::r2_base::ControlPlaneLocks;

use crate::agent::memory::AgentMemory;
use async_trait::async_trait;
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::Client;
use tracing::{error, info};

use moka::future::Cache;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

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
        self.get_user_config_inner(user_id).await
    }

    /// Update user configuration
    async fn update_user_config(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        self.update_user_config_inner(user_id, config).await
    }

    /// Update user system prompt
    async fn update_user_prompt(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        self.update_user_prompt_inner(user_id, system_prompt).await
    }

    /// Get user system prompt
    async fn get_user_prompt(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        self.get_user_prompt_inner(user_id).await
    }

    /// Update user model
    async fn update_user_model(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        self.update_user_model_inner(user_id, model_name).await
    }

    /// Get user model
    async fn get_user_model(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        self.get_user_model_inner(user_id).await
    }

    /// Update user state
    async fn update_user_state(&self, user_id: i64, state: String) -> Result<(), StorageError> {
        self.update_user_state_inner(user_id, state).await
    }

    /// Get user state
    async fn get_user_state(&self, user_id: i64) -> Result<Option<String>, StorageError> {
        self.get_user_state_inner(user_id).await
    }

    /// Save message to chat history
    async fn save_message(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        self.save_message_inner(user_id, role, content).await
    }

    /// Get chat history for a user
    async fn get_chat_history(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        self.get_chat_history_inner(user_id, limit).await
    }

    /// Clear chat history for a user
    async fn clear_chat_history(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_chat_history_inner(user_id).await
    }

    /// Save message to chat history for a specific chat UUID
    async fn save_message_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        self.save_message_for_chat_inner(user_id, chat_uuid, role, content)
            .await
    }

    /// Get chat history for a specific chat UUID
    async fn get_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        self.get_chat_history_for_chat_inner(user_id, chat_uuid, limit)
            .await
    }

    /// Clear chat history for a specific chat UUID
    async fn clear_chat_history_for_chat(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        self.clear_chat_history_for_chat_inner(user_id, chat_uuid)
            .await
    }

    async fn clear_chat_history_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.clear_chat_history_for_context_inner(user_id, context_key)
            .await
    }

    /// Save agent memory to storage
    async fn save_agent_memory(
        &self,
        user_id: i64,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_inner(user_id, memory).await
    }

    async fn save_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_for_context_inner(user_id, context_key, memory)
            .await
    }

    /// Load agent memory from storage
    async fn load_agent_memory(&self, user_id: i64) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_inner(user_id).await
    }

    async fn load_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_for_context_inner(user_id, context_key)
            .await
    }

    /// Clear agent memory for a user
    async fn clear_agent_memory(&self, user_id: i64) -> Result<(), StorageError> {
        self.clear_agent_memory_inner(user_id).await
    }

    async fn clear_agent_memory_for_context(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        self.clear_agent_memory_for_context_inner(user_id, context_key)
            .await
    }

    async fn save_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
        memory: &AgentMemory,
    ) -> Result<(), StorageError> {
        self.save_agent_memory_for_flow_inner(user_id, context_key, flow_id, memory)
            .await
    }

    async fn load_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentMemory>, StorageError> {
        self.load_agent_memory_for_flow_inner(user_id, context_key, flow_id)
            .await
    }

    async fn clear_agent_memory_for_flow(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<(), StorageError> {
        self.clear_agent_memory_for_flow_inner(user_id, context_key, flow_id)
            .await
    }

    async fn get_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<Option<AgentFlowRecord>, StorageError> {
        self.get_agent_flow_record_inner(user_id, context_key, flow_id)
            .await
    }

    async fn upsert_agent_flow_record(
        &self,
        user_id: i64,
        context_key: String,
        flow_id: String,
    ) -> Result<AgentFlowRecord, StorageError> {
        self.upsert_agent_flow_record_inner(user_id, context_key, flow_id)
            .await
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
        self.get_agent_profile_inner(user_id, agent_id).await
    }

    async fn upsert_agent_profile(
        &self,
        options: UpsertAgentProfileOptions,
    ) -> Result<AgentProfileRecord, StorageError> {
        self.upsert_agent_profile_inner(options).await
    }

    async fn delete_agent_profile(
        &self,
        user_id: i64,
        agent_id: String,
    ) -> Result<(), StorageError> {
        self.delete_agent_profile_inner(user_id, agent_id).await
    }

    async fn get_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicContextRecord>, StorageError> {
        self.get_topic_context_inner(user_id, topic_id).await
    }

    async fn upsert_topic_context(
        &self,
        options: UpsertTopicContextOptions,
    ) -> Result<TopicContextRecord, StorageError> {
        self.upsert_topic_context_inner(options).await
    }

    async fn delete_topic_context(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_topic_context_inner(user_id, topic_id).await
    }

    async fn get_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicAgentsMdRecord>, StorageError> {
        self.get_topic_agents_md_inner(user_id, topic_id).await
    }

    async fn upsert_topic_agents_md(
        &self,
        options: UpsertTopicAgentsMdOptions,
    ) -> Result<TopicAgentsMdRecord, StorageError> {
        self.upsert_topic_agents_md_inner(options).await
    }

    async fn delete_topic_agents_md(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_topic_agents_md_inner(user_id, topic_id).await
    }

    async fn get_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicInfraConfigRecord>, StorageError> {
        self.get_topic_infra_config_inner(user_id, topic_id).await
    }

    async fn upsert_topic_infra_config(
        &self,
        options: UpsertTopicInfraConfigOptions,
    ) -> Result<TopicInfraConfigRecord, StorageError> {
        self.upsert_topic_infra_config_inner(options).await
    }

    async fn delete_topic_infra_config(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_topic_infra_config_inner(user_id, topic_id)
            .await
    }

    async fn get_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<Option<String>, StorageError> {
        self.get_secret_value_inner(user_id, secret_ref).await
    }

    async fn put_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
        value: String,
    ) -> Result<(), StorageError> {
        self.put_secret_value_inner(user_id, secret_ref, value)
            .await
    }

    async fn delete_secret_value(
        &self,
        user_id: i64,
        secret_ref: String,
    ) -> Result<(), StorageError> {
        self.delete_secret_value_inner(user_id, secret_ref).await
    }

    async fn get_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<Option<TopicBindingRecord>, StorageError> {
        self.get_topic_binding_inner(user_id, topic_id).await
    }

    async fn upsert_topic_binding(
        &self,
        options: UpsertTopicBindingOptions,
    ) -> Result<TopicBindingRecord, StorageError> {
        self.upsert_topic_binding_inner(options).await
    }

    async fn delete_topic_binding(
        &self,
        user_id: i64,
        topic_id: String,
    ) -> Result<(), StorageError> {
        self.delete_topic_binding_inner(user_id, topic_id).await
    }

    async fn append_audit_event(
        &self,
        options: AppendAuditEventOptions,
    ) -> Result<AuditEventRecord, StorageError> {
        self.append_audit_event_inner(options).await
    }

    async fn list_audit_events(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        self.list_audit_events_inner(user_id, limit).await
    }

    async fn list_audit_events_page(
        &self,
        user_id: i64,
        before_version: Option<u64>,
        limit: usize,
    ) -> Result<Vec<AuditEventRecord>, StorageError> {
        self.list_audit_events_page_inner(user_id, before_version, limit)
            .await
    }

    async fn create_reminder_job(
        &self,
        options: CreateReminderJobOptions,
    ) -> Result<ReminderJobRecord, StorageError> {
        self.create_reminder_job_inner(options).await
    }

    async fn get_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.get_reminder_job_inner(user_id, reminder_id).await
    }

    async fn list_reminder_jobs(
        &self,
        user_id: i64,
        context_key: Option<String>,
        statuses: Option<Vec<ReminderJobStatus>>,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        self.list_reminder_jobs_inner(user_id, context_key, statuses, limit)
            .await
    }

    async fn list_due_reminder_jobs(
        &self,
        user_id: i64,
        now: i64,
        limit: usize,
    ) -> Result<Vec<ReminderJobRecord>, StorageError> {
        self.list_due_reminder_jobs_inner(user_id, now, limit).await
    }

    async fn claim_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        lease_until: i64,
        now: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.claim_reminder_job_inner(user_id, reminder_id, lease_until, now)
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
        self.reschedule_reminder_job_inner(
            user_id,
            reminder_id,
            next_run_at,
            last_run_at,
            last_error,
            increment_run_count,
        )
        .await
    }

    async fn complete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        completed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.complete_reminder_job_inner(user_id, reminder_id, completed_at)
            .await
    }

    async fn fail_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        failed_at: i64,
        error: String,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.fail_reminder_job_inner(user_id, reminder_id, failed_at, error)
            .await
    }

    async fn cancel_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        cancelled_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.cancel_reminder_job_inner(user_id, reminder_id, cancelled_at)
            .await
    }

    async fn pause_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        paused_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.pause_reminder_job_inner(user_id, reminder_id, paused_at)
            .await
    }

    async fn resume_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        resumed_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.resume_reminder_job_inner(user_id, reminder_id, next_run_at, resumed_at)
            .await
    }

    async fn retry_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
        next_run_at: i64,
        retried_at: i64,
    ) -> Result<Option<ReminderJobRecord>, StorageError> {
        self.retry_reminder_job_inner(user_id, reminder_id, next_run_at, retried_at)
            .await
    }

    async fn delete_reminder_job(
        &self,
        user_id: i64,
        reminder_id: String,
    ) -> Result<(), StorageError> {
        self.delete_reminder_job_inner(user_id, reminder_id).await
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
mod tests;
