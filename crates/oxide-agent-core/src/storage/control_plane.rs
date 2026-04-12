use super::StorageError;
use serde::{Deserialize, Serialize};

pub(crate) const TOPIC_CONTEXT_MAX_LINES: usize = 40;
pub(crate) const TOPIC_CONTEXT_MAX_CHARS: usize = 4_000;
pub(crate) const TOPIC_AGENTS_MD_MAX_LINES: usize = 300;

/// Agent profile record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentProfileRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this profile.
    pub user_id: i64,
    /// Stable agent identifier.
    pub agent_id: String,
    /// Arbitrary profile payload.
    pub profile: serde_json::Value,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Topic-specific prompt context persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicContextRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this topic context.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Free-form prompt context for the topic.
    pub context: String,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Topic-scoped `AGENTS.md` instructions persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicAgentsMdRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this topic configuration.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Full `AGENTS.md` content pinned for new flows in the topic.
    pub agents_md: String,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Authentication mode used by topic-scoped infrastructure access.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TopicInfraAuthMode {
    /// Reuse agent host environment authentication.
    #[default]
    None,
    /// Resolve a password from external secret storage.
    Password,
    /// Resolve a private key from external secret storage.
    PrivateKey,
}

/// SSH-capable tool modes allowed by topic infrastructure configuration.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TopicInfraToolMode {
    /// Remote command execution without sudo.
    Exec,
    /// Remote command execution with sudo privileges.
    SudoExec,
    /// Read a remote file.
    ReadFile,
    /// Apply an in-place file edit on a remote host.
    ApplyFileEdit,
    /// Inspect remote process state.
    CheckProcess,
    /// Transfer remote files to the local host for delivery or inspection.
    Transfer,
}

/// Topic-specific infrastructure configuration persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicInfraConfigRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this infrastructure configuration.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Human-readable target name surfaced to operators.
    pub target_name: String,
    /// Target SSH host or DNS name.
    pub host: String,
    /// Target SSH port.
    pub port: u16,
    /// Remote SSH username.
    pub remote_user: String,
    /// Authentication mode for this target.
    pub auth_mode: TopicInfraAuthMode,
    /// External secret reference for SSH authentication material.
    pub secret_ref: Option<String>,
    /// External secret reference for sudo password material.
    pub sudo_secret_ref: Option<String>,
    /// Optional environment label, e.g. prod/stage.
    pub environment: Option<String>,
    /// Free-form target tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Allowlisted SSH-capable tool modes.
    #[serde(default)]
    pub allowed_tool_modes: Vec<TopicInfraToolMode>,
    /// Tool modes that always require operator approval.
    #[serde(default)]
    pub approval_required_modes: Vec<TopicInfraToolMode>,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Topic binding record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TopicBindingKind {
    /// Manually managed static topic binding.
    #[default]
    Manual,
    /// Runtime-generated dynamic topic binding.
    Runtime,
}

/// Topic binding record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TopicBindingRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Logical record revision incremented on each upsert.
    pub version: u64,
    /// User owning this topic binding.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Agent identifier bound to topic.
    pub agent_id: String,
    /// Binding source kind.
    #[serde(default)]
    pub binding_kind: TopicBindingKind,
    /// Optional transport chat identifier for runtime resolution.
    #[serde(default)]
    pub chat_id: Option<i64>,
    /// Optional transport thread identifier for runtime resolution.
    #[serde(default)]
    pub thread_id: Option<i64>,
    /// Optional binding expiry timestamp (unix seconds).
    #[serde(default)]
    pub expires_at: Option<i64>,
    /// Last activity timestamp (unix seconds) used for future auto-expiry.
    #[serde(default)]
    pub last_activity_at: Option<i64>,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
    /// Last update timestamp (unix seconds).
    pub updated_at: i64,
}

/// Audit event record persisted in control-plane storage.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuditEventRecord {
    /// Record schema version for forward-compatible evolution.
    pub schema_version: u32,
    /// Monotonic sequence per user audit stream.
    pub version: u64,
    /// Stable unique event identifier.
    pub event_id: String,
    /// User associated with the event.
    pub user_id: i64,
    /// Optional topic associated with the event.
    pub topic_id: Option<String>,
    /// Optional agent associated with the event.
    pub agent_id: Option<String>,
    /// Event action name.
    pub action: String,
    /// Arbitrary event payload.
    pub payload: serde_json::Value,
    /// Creation timestamp (unix seconds).
    pub created_at: i64,
}

/// Parameters for agent profile upsert.
#[derive(Debug, Clone)]
pub struct UpsertAgentProfileOptions {
    /// User owning this profile.
    pub user_id: i64,
    /// Stable agent identifier.
    pub agent_id: String,
    /// Arbitrary profile payload.
    pub profile: serde_json::Value,
}

/// Parameters for topic context upsert.
#[derive(Debug, Clone)]
pub struct UpsertTopicContextOptions {
    /// User owning this topic context.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Free-form prompt context for the topic.
    pub context: String,
}

/// Parameters for topic-scoped `AGENTS.md` upsert.
#[derive(Debug, Clone)]
pub struct UpsertTopicAgentsMdOptions {
    /// User owning this topic configuration.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Full `AGENTS.md` content for the topic.
    pub agents_md: String,
}

/// Parameters for topic infrastructure configuration upsert.
#[derive(Debug, Clone)]
pub struct UpsertTopicInfraConfigOptions {
    /// User owning this infrastructure configuration.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Human-readable target name surfaced to operators.
    pub target_name: String,
    /// Target SSH host or DNS name.
    pub host: String,
    /// Target SSH port.
    pub port: u16,
    /// Remote SSH username.
    pub remote_user: String,
    /// Authentication mode for this target.
    pub auth_mode: TopicInfraAuthMode,
    /// External secret reference for SSH authentication material.
    pub secret_ref: Option<String>,
    /// External secret reference for sudo password material.
    pub sudo_secret_ref: Option<String>,
    /// Optional environment label.
    pub environment: Option<String>,
    /// Free-form target tags.
    pub tags: Vec<String>,
    /// Allowlisted SSH-capable tool modes.
    pub allowed_tool_modes: Vec<TopicInfraToolMode>,
    /// Tool modes that always require operator approval.
    pub approval_required_modes: Vec<TopicInfraToolMode>,
}

/// Parameters for topic binding upsert.
#[derive(Debug, Clone)]
pub struct UpsertTopicBindingOptions {
    /// User owning this topic binding.
    pub user_id: i64,
    /// Stable topic identifier.
    pub topic_id: String,
    /// Agent identifier bound to topic.
    pub agent_id: String,
    /// Binding source kind. If omitted, existing value is preserved or defaults to `manual`.
    pub binding_kind: Option<TopicBindingKind>,
    /// Optional transport chat identifier for runtime resolution.
    pub chat_id: OptionalMetadataPatch<i64>,
    /// Optional transport thread identifier for runtime resolution.
    pub thread_id: OptionalMetadataPatch<i64>,
    /// Optional binding expiry timestamp (unix seconds).
    pub expires_at: OptionalMetadataPatch<i64>,
    /// Optional last activity timestamp override (unix seconds).
    pub last_activity_at: Option<i64>,
}

/// Tri-state patch semantics for optional metadata fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionalMetadataPatch<T> {
    /// Keep the currently stored value untouched.
    #[default]
    Keep,
    /// Set a concrete value.
    Set(T),
    /// Clear currently stored value to `None`.
    Clear,
}

impl<T> OptionalMetadataPatch<T> {
    /// Applies patch semantics to an existing optional value.
    #[must_use]
    pub fn apply(self, current: Option<T>) -> Option<T> {
        match self {
            Self::Keep => current,
            Self::Set(value) => Some(value),
            Self::Clear => None,
        }
    }

    /// Resolves patch semantics for a newly created record.
    #[must_use]
    pub fn for_new_record(self) -> Option<T> {
        match self {
            Self::Keep | Self::Clear => None,
            Self::Set(value) => Some(value),
        }
    }
}

impl<'de, T> Deserialize<'de> for OptionalMetadataPatch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Option::<T>::deserialize(deserializer)?;
        Ok(match value {
            Some(inner) => Self::Set(inner),
            None => Self::Clear,
        })
    }
}

/// Parameters for audit append operation.
#[derive(Debug, Clone)]
pub struct AppendAuditEventOptions {
    /// User associated with the event.
    pub user_id: i64,
    /// Optional topic associated with the event.
    pub topic_id: Option<String>,
    /// Optional agent associated with the event.
    pub agent_id: Option<String>,
    /// Event action name.
    pub action: String,
    /// Arbitrary event payload.
    pub payload: serde_json::Value,
}

#[must_use]
pub(crate) fn normalize_topic_prompt_payload(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub(crate) fn validate_topic_context_content(value: &str) -> Result<String, StorageError> {
    let context = normalize_topic_prompt_payload(value);
    if context.is_empty() {
        return Err(StorageError::InvalidInput(
            "context must not be empty".to_string(),
        ));
    }

    let line_count = context.lines().count();
    if line_count > TOPIC_CONTEXT_MAX_LINES {
        return Err(StorageError::InvalidInput(format!(
            "context must not exceed {TOPIC_CONTEXT_MAX_LINES} lines; store AGENTS.md in topic_agents_md via topic_agents_md_upsert"
        )));
    }

    let char_count = context.chars().count();
    if char_count > TOPIC_CONTEXT_MAX_CHARS {
        return Err(StorageError::InvalidInput(format!(
            "context must not exceed {TOPIC_CONTEXT_MAX_CHARS} characters; store AGENTS.md in topic_agents_md via topic_agents_md_upsert"
        )));
    }

    let lower = context.to_ascii_lowercase();
    if lower.contains("agents.md")
        || context
            .lines()
            .any(|line| line.trim_start().starts_with('#'))
    {
        return Err(StorageError::InvalidInput(
            "context is for short operational notes only; store AGENTS.md-style documents in topic_agents_md via topic_agents_md_upsert".to_string(),
        ));
    }

    Ok(context)
}

pub(crate) fn validate_topic_agents_md_content(value: &str) -> Result<String, StorageError> {
    let agents_md = normalize_topic_prompt_payload(value);
    if agents_md.is_empty() {
        return Err(StorageError::InvalidInput(
            "agents_md must not be empty".to_string(),
        ));
    }

    let line_count = agents_md.lines().count();
    if line_count > TOPIC_AGENTS_MD_MAX_LINES {
        return Err(StorageError::InvalidInput(format!(
            "agents_md must not exceed {TOPIC_AGENTS_MD_MAX_LINES} lines (got {line_count})"
        )));
    }

    Ok(agents_md)
}

/// Returns `true` when a topic binding is active at `now`.
#[must_use]
pub fn binding_is_active(record: &TopicBindingRecord, now: i64) -> bool {
    match record.expires_at {
        Some(expires_at) => expires_at > now,
        None => true,
    }
}

/// Returns a binding only when it is active at `now`.
#[must_use]
pub fn resolve_active_topic_binding(
    record: Option<TopicBindingRecord>,
    now: i64,
) -> Option<TopicBindingRecord> {
    record.filter(|binding| binding_is_active(binding, now))
}
