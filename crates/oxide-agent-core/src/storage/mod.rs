//! Storage layer for user, agent, topic, and control-plane data.
//!
//! Provides persistent storage implementations for durable runtime state.

#[cfg(any(feature = "storage-sqlx", test))]
mod builders;
mod control_plane;
mod error;
mod flows;
mod keys;
#[cfg(feature = "storage-sqlx")]
mod modules;
mod provider;
mod reminder;
#[cfg(any(feature = "storage-sqlx", test))]
mod schema;
#[cfg(feature = "storage-sqlx")]
mod sqlx;
#[cfg(feature = "storage-sqlx")]
mod sqlx_config;
mod user;
#[cfg(feature = "storage-sqlx")]
mod utils;

#[cfg(test)]
pub(crate) use control_plane::TOPIC_AGENTS_MD_MAX_LINES;
pub use control_plane::{
    AgentProfileRecord, AppendAuditEventOptions, AuditEventRecord, OptionalMetadataPatch,
    TopicAgentsMdRecord, TopicBindingKind, TopicBindingRecord, TopicContextRecord,
    TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UpsertTopicContextOptions,
    UpsertTopicInfraConfigOptions, binding_is_active, resolve_active_topic_binding,
};
pub(crate) use control_plane::{
    TOPIC_CONTEXT_MAX_CHARS, TOPIC_CONTEXT_MAX_LINES, validate_topic_agents_md_content,
    validate_topic_context_content,
};
pub use error::StorageError;
pub use flows::AgentFlowRecord;
pub use keys::{
    generate_flow_id, wiki_context_inbox_key, wiki_context_key, wiki_context_page_key,
    wiki_context_prefix, wiki_context_raw_key, wiki_global_key,
};
#[cfg(feature = "storage-sqlx")]
pub use modules::{BuiltStorageBackend, StorageBackendModule, build_primary_storage};
#[cfg(test)]
pub use provider::MockStorageProvider;
pub use provider::StorageProvider;
pub use reminder::{
    CreateReminderJobOptions, ReminderJobRecord, ReminderJobStatus, ReminderScheduleKind,
    ReminderThreadKind, compute_cron_next_run_at, compute_next_reminder_run_at,
    format_reminder_unix_in_timezone, parse_reminder_timezone, resolve_reminder_local_datetime,
};
#[cfg(feature = "storage-sqlx")]
pub use sqlx::SqlxStorage;
#[cfg(feature = "storage-sqlx")]
pub use sqlx_config::{SQLX_STORAGE_MODULE_ID, SqlxStorageConfig};
pub use user::{UserConfig, UserContextConfig};

#[cfg(test)]
mod tests;
