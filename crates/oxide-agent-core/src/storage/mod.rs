//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

mod builders;
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
mod schema;
mod user;

#[cfg(test)]
pub(crate) use builders::next_record_version;
pub(crate) use builders::{
    build_agent_flow_record, build_agent_profile_record, build_audit_event_record,
    build_reminder_job_record, build_topic_agents_md_record, build_topic_binding_record,
    build_topic_context_record, build_topic_infra_config_record, with_next_reminder_version,
};
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
