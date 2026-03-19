//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

mod control_plane;
mod error;
mod flows;
mod keys;
mod provider;
mod r2;
mod r2_base;
mod r2_control_plane;
mod r2_memory;
mod r2_provider;
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
pub use r2::R2Storage;
pub use reminder::{
    compute_cron_next_run_at, compute_next_reminder_run_at, parse_reminder_timezone,
    CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind,
    ReminderThreadKind,
};
pub use user::{Message, UserConfig, UserContextConfig};

#[cfg(test)]
use self::r2_base::ControlPlaneLocks;

use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::put_object::PutObjectError;
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
