//! Storage layer for user data and chat history
//!
//! Provides a persistent storage implementation using Cloudflare R2 / AWS S3.

mod builders;
mod compaction;
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
mod telemetry;
mod user;
mod utils;

pub use compaction::{CompactionBlobBackend, R2ArchiveSink, R2PayloadSink};
#[cfg(test)]
pub(crate) use control_plane::TOPIC_AGENTS_MD_MAX_LINES;
pub use control_plane::{
    binding_is_active, resolve_active_topic_binding, AgentProfileRecord, AppendAuditEventOptions,
    AuditEventRecord, OptionalMetadataPatch, TopicAgentsMdRecord, TopicBindingKind,
    TopicBindingRecord, TopicContextRecord, TopicInfraAuthMode, TopicInfraConfigRecord,
    TopicInfraToolMode, UpsertAgentProfileOptions, UpsertTopicAgentsMdOptions,
    UpsertTopicBindingOptions, UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
};
pub(crate) use control_plane::{
    validate_topic_agents_md_content, validate_topic_context_content, TOPIC_CONTEXT_MAX_CHARS,
    TOPIC_CONTEXT_MAX_LINES,
};
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
pub use r2::{PersistedAgentMemoryRef, R2Storage};
pub use reminder::{
    compute_cron_next_run_at, compute_next_reminder_run_at, format_reminder_unix_in_timezone,
    parse_reminder_timezone, resolve_reminder_local_datetime, CreateReminderJobOptions,
    ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind, ReminderThreadKind,
};
pub use user::{Message, UserConfig, UserContextConfig};

#[cfg(test)]
mod tests;
