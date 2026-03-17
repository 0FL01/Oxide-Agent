//! Manager control-plane provider.
//!
//! Exposes user-scoped CRUD tools for topic bindings, topic contexts, and agent profiles.

use super::ssh_mcp::{probe_secret_ref, SecretProbeKind};
use crate::agent::profile::{
    parse_agent_profile, topic_agent_all_hooks, topic_agent_default_blocked_tools,
    topic_agent_manageable_hooks, topic_agent_protected_hooks, HookAccessPolicy, ToolAccessPolicy,
};
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxContainerRecord, SandboxManager, SandboxScope};
use crate::storage::{
    validate_topic_agents_md_content, validate_topic_context_content, AgentProfileRecord,
    AppendAuditEventOptions, OptionalMetadataPatch, StorageProvider, TopicAgentsMdRecord,
    TopicBindingKind, TopicBindingRecord, TopicContextRecord, TopicInfraAuthMode,
    TopicInfraConfigRecord, TopicInfraToolMode, UpsertAgentProfileOptions,
    UpsertTopicAgentsMdOptions, UpsertTopicBindingOptions, UpsertTopicContextOptions,
    UpsertTopicInfraConfigOptions, UserConfig, TOPIC_CONTEXT_MAX_CHARS, TOPIC_CONTEXT_MAX_LINES,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

mod agent_controls;
mod agents_md;
mod audit;
mod bindings;
mod contexts;
mod infra;
mod profiles;
mod shared;

use self::audit::AuditStatus;

const TOOL_TOPIC_BINDING_SET: &str = "topic_binding_set";
const TOOL_TOPIC_BINDING_GET: &str = "topic_binding_get";
const TOOL_TOPIC_BINDING_DELETE: &str = "topic_binding_delete";
const TOOL_TOPIC_BINDING_ROLLBACK: &str = "topic_binding_rollback";
const TOOL_TOPIC_CONTEXT_UPSERT: &str = "topic_context_upsert";
const TOOL_TOPIC_CONTEXT_GET: &str = "topic_context_get";
const TOOL_TOPIC_CONTEXT_DELETE: &str = "topic_context_delete";
const TOOL_TOPIC_CONTEXT_ROLLBACK: &str = "topic_context_rollback";
const TOOL_TOPIC_AGENTS_MD_UPSERT: &str = "topic_agents_md_upsert";
const TOOL_TOPIC_AGENTS_MD_GET: &str = "topic_agents_md_get";
const TOOL_TOPIC_AGENTS_MD_DELETE: &str = "topic_agents_md_delete";
const TOOL_TOPIC_AGENTS_MD_ROLLBACK: &str = "topic_agents_md_rollback";
const TOOL_TOPIC_INFRA_UPSERT: &str = "topic_infra_upsert";
const TOOL_TOPIC_INFRA_GET: &str = "topic_infra_get";
const TOOL_TOPIC_INFRA_DELETE: &str = "topic_infra_delete";
const TOOL_TOPIC_INFRA_ROLLBACK: &str = "topic_infra_rollback";
const TOOL_PRIVATE_SECRET_PROBE: &str = "private_secret_probe";
const TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT: &str = "forum_topic_provision_ssh_agent";
const TOOL_AGENT_PROFILE_UPSERT: &str = "agent_profile_upsert";
const TOOL_AGENT_PROFILE_GET: &str = "agent_profile_get";
const TOOL_AGENT_PROFILE_DELETE: &str = "agent_profile_delete";
const TOOL_AGENT_PROFILE_ROLLBACK: &str = "agent_profile_rollback";
const TOOL_TOPIC_AGENT_TOOLS_GET: &str = "topic_agent_tools_get";
const TOOL_TOPIC_AGENT_TOOLS_ENABLE: &str = "topic_agent_tools_enable";
const TOOL_TOPIC_AGENT_TOOLS_DISABLE: &str = "topic_agent_tools_disable";
const TOOL_TOPIC_AGENT_HOOKS_GET: &str = "topic_agent_hooks_get";
const TOOL_TOPIC_AGENT_HOOKS_ENABLE: &str = "topic_agent_hooks_enable";
const TOOL_TOPIC_AGENT_HOOKS_DISABLE: &str = "topic_agent_hooks_disable";
const TOOL_TOPIC_SANDBOX_LIST: &str = "topic_sandbox_list";
const TOOL_TOPIC_SANDBOX_GET: &str = "topic_sandbox_get";
const TOOL_TOPIC_SANDBOX_CREATE: &str = "topic_sandbox_create";
const TOOL_TOPIC_SANDBOX_RECREATE: &str = "topic_sandbox_recreate";
const TOOL_TOPIC_SANDBOX_DELETE: &str = "topic_sandbox_delete";
const TOOL_TOPIC_SANDBOX_PRUNE: &str = "topic_sandbox_prune";
const TOOL_FORUM_TOPIC_CREATE: &str = "forum_topic_create";
const TOOL_FORUM_TOPIC_EDIT: &str = "forum_topic_edit";
const TOOL_FORUM_TOPIC_CLOSE: &str = "forum_topic_close";
const TOOL_FORUM_TOPIC_REOPEN: &str = "forum_topic_reopen";
const TOOL_FORUM_TOPIC_DELETE: &str = "forum_topic_delete";
const TOOL_FORUM_TOPIC_LIST: &str = "forum_topic_list";
const ROLLBACK_AUDIT_PAGE_SIZE: usize = 200;

const BASE_TOOL_NAMES: &[&str] = &[
    TOOL_TOPIC_BINDING_SET,
    TOOL_TOPIC_BINDING_GET,
    TOOL_TOPIC_BINDING_DELETE,
    TOOL_TOPIC_BINDING_ROLLBACK,
    TOOL_TOPIC_CONTEXT_UPSERT,
    TOOL_TOPIC_CONTEXT_GET,
    TOOL_TOPIC_CONTEXT_DELETE,
    TOOL_TOPIC_CONTEXT_ROLLBACK,
    TOOL_TOPIC_AGENTS_MD_UPSERT,
    TOOL_TOPIC_AGENTS_MD_GET,
    TOOL_TOPIC_AGENTS_MD_DELETE,
    TOOL_TOPIC_AGENTS_MD_ROLLBACK,
    TOOL_TOPIC_INFRA_UPSERT,
    TOOL_TOPIC_INFRA_GET,
    TOOL_TOPIC_INFRA_DELETE,
    TOOL_TOPIC_INFRA_ROLLBACK,
    TOOL_PRIVATE_SECRET_PROBE,
    TOOL_AGENT_PROFILE_UPSERT,
    TOOL_AGENT_PROFILE_GET,
    TOOL_AGENT_PROFILE_DELETE,
    TOOL_AGENT_PROFILE_ROLLBACK,
    TOOL_TOPIC_AGENT_TOOLS_GET,
    TOOL_TOPIC_AGENT_TOOLS_ENABLE,
    TOOL_TOPIC_AGENT_TOOLS_DISABLE,
    TOOL_TOPIC_AGENT_HOOKS_GET,
    TOOL_TOPIC_AGENT_HOOKS_ENABLE,
    TOOL_TOPIC_AGENT_HOOKS_DISABLE,
    TOOL_TOPIC_SANDBOX_LIST,
    TOOL_TOPIC_SANDBOX_GET,
    TOOL_TOPIC_SANDBOX_CREATE,
    TOOL_TOPIC_SANDBOX_RECREATE,
    TOOL_TOPIC_SANDBOX_DELETE,
    TOOL_TOPIC_SANDBOX_PRUNE,
];

const LIFECYCLE_TOOL_NAMES: &[&str] = &[
    TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT,
    TOOL_FORUM_TOPIC_CREATE,
    TOOL_FORUM_TOPIC_EDIT,
    TOOL_FORUM_TOPIC_CLOSE,
    TOOL_FORUM_TOPIC_REOPEN,
    TOOL_FORUM_TOPIC_DELETE,
    TOOL_FORUM_TOPIC_LIST,
];

/// Returns the manager control-plane tool names that must remain available
/// in manager-enabled sessions even when an agent profile uses an allowlist.
#[must_use]
pub fn manager_control_plane_tool_names() -> Vec<String> {
    BASE_TOOL_NAMES
        .iter()
        .chain(LIFECYCLE_TOOL_NAMES.iter())
        .map(|tool| (*tool).to_string())
        .collect()
}
const TELEGRAM_FORUM_ICON_COLORS: [u32; 6] = [
    7_322_096, 16_766_590, 13_338_331, 9_367_192, 16_749_490, 16_478_047,
];

const fn default_ssh_port() -> u16 {
    22
}

const fn default_secret_probe_kind() -> SecretProbeKind {
    SecretProbeKind::Opaque
}

fn default_infra_allowed_tool_modes() -> Vec<TopicInfraToolMode> {
    vec![
        TopicInfraToolMode::Exec,
        TopicInfraToolMode::SudoExec,
        TopicInfraToolMode::ReadFile,
        TopicInfraToolMode::ApplyFileEdit,
        TopicInfraToolMode::CheckProcess,
    ]
}

fn default_infra_approval_required_modes() -> Vec<TopicInfraToolMode> {
    vec![
        TopicInfraToolMode::SudoExec,
        TopicInfraToolMode::ApplyFileEdit,
    ]
}

fn default_ssh_agent_allowed_tools() -> Vec<String> {
    vec![
        "write_todos".to_string(),
        "ssh_exec".to_string(),
        "ssh_sudo_exec".to_string(),
        "ssh_read_file".to_string(),
        "ssh_apply_file_edit".to_string(),
        "ssh_check_process".to_string(),
        "reminder_schedule".to_string(),
        "reminder_list".to_string(),
        "reminder_cancel".to_string(),
        "reminder_pause".to_string(),
        "reminder_resume".to_string(),
        "reminder_retry".to_string(),
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForumTopicProvisionSshAgentArgs {
    name: String,
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    icon_color: Option<u32>,
    #[serde(default)]
    icon_custom_emoji_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    topic_context: Option<String>,
    #[serde(default)]
    target_name: Option<String>,
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
    remote_user: String,
    auth_mode: TopicInfraAuthMode,
    #[serde(default)]
    secret_ref: Option<String>,
    #[serde(default)]
    sudo_secret_ref: Option<String>,
    #[serde(default)]
    environment: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_infra_allowed_tool_modes")]
    allowed_tool_modes: Vec<TopicInfraToolMode>,
    #[serde(default = "default_infra_approval_required_modes")]
    approval_required_modes: Vec<TopicInfraToolMode>,
    #[serde(default)]
    dry_run: bool,
}

struct ForumTopicProvisionSshAgentPlan {
    request: ForumTopicCreateRequest,
    agent_id: String,
    profile: serde_json::Value,
    topic_context: Option<String>,
    target_name: String,
    host: String,
    port: u16,
    remote_user: String,
    auth_mode: TopicInfraAuthMode,
    secret_ref: Option<String>,
    sudo_secret_ref: Option<String>,
    environment: Option<String>,
    tags: Vec<String>,
    allowed_tool_modes: Vec<TopicInfraToolMode>,
    approval_required_modes: Vec<TopicInfraToolMode>,
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateSecretProbeArgs {
    secret_ref: String,
    #[serde(default = "default_secret_probe_kind")]
    kind: SecretProbeKind,
}

const TOPIC_AGENT_TODOS_TOOLS: &[&str] = &["write_todos"];
const TOPIC_AGENT_SANDBOX_TOOLS: &[&str] = &[
    "execute_command",
    "write_file",
    "read_file",
    "send_file_to_user",
    "list_files",
];
const TOPIC_AGENT_FILEHOSTER_TOOLS: &[&str] = &["upload_file"];
const TOPIC_AGENT_YTDLP_TOOLS: &[&str] = &[
    "ytdlp_get_video_metadata",
    "ytdlp_download_transcript",
    "ytdlp_search_videos",
    "ytdlp_download_video",
    "ytdlp_download_audio",
];
const TOPIC_AGENT_DELEGATION_TOOLS: &[&str] = &["delegate_to_sub_agent"];
const TOPIC_AGENT_REMINDER_TOOLS: &[&str] = &[
    "reminder_schedule",
    "reminder_list",
    "reminder_cancel",
    "reminder_pause",
    "reminder_resume",
    "reminder_retry",
];
#[cfg(feature = "tavily")]
const TOPIC_AGENT_TAVILY_TOOLS: &[&str] = &["web_search", "web_extract"];
#[cfg(feature = "crawl4ai")]
const TOPIC_AGENT_CRAWL4AI_TOOLS: &[&str] = &["deep_crawl", "web_markdown", "web_pdf"];
const TOPIC_AGENT_SSH_TOOLS: &[&str] = &[
    "ssh_exec",
    "ssh_sudo_exec",
    "ssh_read_file",
    "ssh_apply_file_edit",
    "ssh_check_process",
];

/// Transport-agnostic request for forum topic creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicCreateRequest {
    /// Explicit chat identifier. If omitted, implementation may use injected context.
    pub chat_id: Option<i64>,
    /// Forum topic title.
    pub name: String,
    /// Optional topic icon color in RGB integer format.
    pub icon_color: Option<u32>,
    /// Optional custom emoji identifier used as topic icon.
    pub icon_custom_emoji_id: Option<String>,
}

/// Transport-agnostic request for forum topic updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicEditRequest {
    /// Explicit chat identifier. If omitted, implementation may use injected context.
    pub chat_id: Option<i64>,
    /// Forum topic thread identifier.
    pub thread_id: i64,
    /// Optional new title.
    pub name: Option<String>,
    /// Optional custom emoji identifier. Empty string may clear icon depending on transport.
    pub icon_custom_emoji_id: Option<String>,
}

/// Transport-agnostic request targeting an existing forum topic thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicThreadRequest {
    /// Explicit chat identifier. If omitted, implementation may use injected context.
    pub chat_id: Option<i64>,
    /// Forum topic thread identifier.
    pub thread_id: i64,
}

/// Result returned by forum topic creation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicCreateResult {
    /// Effective chat identifier used by transport.
    pub chat_id: i64,
    /// Created forum topic thread identifier.
    pub thread_id: i64,
    /// Created topic title.
    pub name: String,
    /// Created topic icon color in RGB integer format.
    pub icon_color: u32,
    /// Created topic icon emoji identifier.
    pub icon_custom_emoji_id: Option<String>,
}

/// Result returned by forum topic edit operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicEditResult {
    /// Effective chat identifier used by transport.
    pub chat_id: i64,
    /// Target forum topic thread identifier.
    pub thread_id: i64,
    /// Applied topic title.
    pub name: Option<String>,
    /// Applied topic icon emoji identifier.
    pub icon_custom_emoji_id: Option<String>,
}

/// Result returned by thread-scoped forum topic actions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ForumTopicActionResult {
    /// Effective chat identifier used by transport.
    pub chat_id: i64,
    /// Target forum topic thread identifier.
    pub thread_id: i64,
}

/// Abstraction over transport-specific forum topic lifecycle operations.
#[async_trait]
pub trait ManagerTopicLifecycle: Send + Sync {
    /// Returns default chat identifier for the current transport context when available.
    fn default_forum_chat_id(&self) -> Option<i64> {
        None
    }

    /// Creates a new forum topic.
    async fn forum_topic_create(
        &self,
        request: ForumTopicCreateRequest,
    ) -> Result<ForumTopicCreateResult>;

    /// Edits an existing forum topic.
    async fn forum_topic_edit(
        &self,
        request: ForumTopicEditRequest,
    ) -> Result<ForumTopicEditResult>;

    /// Closes a forum topic.
    async fn forum_topic_close(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult>;

    /// Reopens a forum topic.
    async fn forum_topic_reopen(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult>;

    /// Deletes a forum topic.
    async fn forum_topic_delete(
        &self,
        request: ForumTopicThreadRequest,
    ) -> Result<ForumTopicActionResult>;
}

/// Abstraction over sandbox cleanup for deleted transport topics.
#[async_trait]
pub trait ManagerTopicSandboxCleanup: Send + Sync {
    /// Remove sandbox resources associated with a deleted topic.
    async fn cleanup_topic_sandbox(
        &self,
        user_id: i64,
        topic: &ForumTopicActionResult,
    ) -> Result<()>;
}

/// Abstraction over sandbox inventory and lifecycle controls exposed via manager tools.
#[async_trait]
pub trait ManagerTopicSandboxControl: Send + Sync {
    /// List all user-owned sandbox containers.
    async fn list_topic_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>>;

    /// Get a user-owned sandbox container by Docker name.
    async fn get_topic_sandbox(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>>;

    /// Ensure a topic sandbox exists for the given scope.
    async fn ensure_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord>;

    /// Recreate a topic sandbox for the given scope.
    async fn recreate_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord>;

    /// Delete a topic sandbox by its logical scope.
    async fn delete_topic_sandbox_by_scope(&self, scope: SandboxScope) -> Result<bool>;

    /// Delete a topic sandbox by Docker container name.
    async fn delete_topic_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<bool>;
}

#[derive(Default)]
struct DockerTopicSandboxCleanup;

#[derive(Default)]
struct DockerTopicSandboxControl;

#[async_trait]
impl ManagerTopicSandboxCleanup for DockerTopicSandboxCleanup {
    async fn cleanup_topic_sandbox(
        &self,
        user_id: i64,
        topic: &ForumTopicActionResult,
    ) -> Result<()> {
        let scope = SandboxScope::new(
            user_id,
            ManagerControlPlaneProvider::forum_topic_context_key(topic.chat_id, topic.thread_id),
        )
        .with_transport_metadata(Some(topic.chat_id), Some(topic.thread_id));
        let mut sandbox = SandboxManager::new(scope).await?;
        sandbox.destroy().await
    }
}

#[async_trait]
impl ManagerTopicSandboxControl for DockerTopicSandboxControl {
    async fn list_topic_sandboxes(&self, user_id: i64) -> Result<Vec<SandboxContainerRecord>> {
        SandboxManager::list_user_sandboxes(user_id).await
    }

    async fn get_topic_sandbox(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<Option<SandboxContainerRecord>> {
        SandboxManager::inspect_sandbox_by_name(user_id, container_name).await
    }

    async fn ensure_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        SandboxManager::ensure_scope_sandbox(scope).await
    }

    async fn recreate_topic_sandbox(&self, scope: SandboxScope) -> Result<SandboxContainerRecord> {
        SandboxManager::recreate_scope_sandbox(scope).await
    }

    async fn delete_topic_sandbox_by_scope(&self, scope: SandboxScope) -> Result<bool> {
        SandboxManager::delete_sandbox_by_name(scope.owner_id(), &scope.container_name()).await
    }

    async fn delete_topic_sandbox_by_name(
        &self,
        user_id: i64,
        container_name: &str,
    ) -> Result<bool> {
        SandboxManager::delete_sandbox_by_name(user_id, container_name).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForumTopicCreateArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    name: String,
    #[serde(default)]
    icon_color: Option<u32>,
    #[serde(default)]
    icon_custom_emoji_id: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForumTopicEditArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    thread_id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    icon_custom_emoji_id: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForumTopicThreadArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    thread_id: i64,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForumTopicListArgs {
    #[serde(default)]
    chat_id: Option<i64>,
    #[serde(default)]
    include_closed: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TopicSandboxPruneReason {
    TopicMissing,
    BindingMissing,
    SandboxDisabled,
    #[default]
    All,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxListArgs {
    #[serde(default)]
    orphaned_only: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxGetArgs {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default)]
    container_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxCreateArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxRecreateArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxDeleteArgs {
    #[serde(default)]
    topic_id: Option<String>,
    #[serde(default)]
    container_name: Option<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSandboxPruneArgs {
    #[serde(default)]
    reason: TopicSandboxPruneReason,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct TopicSandboxInventoryRecord {
    container_id: String,
    container_name: String,
    image: Option<String>,
    created_at: Option<i64>,
    state: Option<String>,
    status: Option<String>,
    running: bool,
    topic_id: Option<String>,
    chat_id: Option<i64>,
    thread_id: Option<i64>,
    labels: std::collections::HashMap<String, String>,
    bound_topic_exists: bool,
    binding_found: bool,
    sandbox_tools_enabled: Option<bool>,
    orphan_reason: Option<String>,
}

#[derive(Debug)]
enum TopicSandboxTarget {
    TopicId(String),
    ContainerName(String),
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
struct ForumTopicCatalogEntry {
    topic_id: String,
    chat_id: i64,
    thread_id: i64,
    name: Option<String>,
    icon_color: Option<u32>,
    icon_custom_emoji_id: Option<String>,
    closed: bool,
}

/// Tool provider that manages user-scoped control-plane records.
pub struct ManagerControlPlaneProvider {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
    sandbox_cleanup: Arc<dyn ManagerTopicSandboxCleanup>,
    sandbox_control: Arc<dyn ManagerTopicSandboxControl>,
}

impl ManagerControlPlaneProvider {
    /// Creates a manager control-plane provider bound to a specific user.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, user_id: i64) -> Self {
        Self {
            storage,
            user_id,
            topic_lifecycle: None,
            sandbox_cleanup: Arc::new(DockerTopicSandboxCleanup),
            sandbox_control: Arc::new(DockerTopicSandboxControl),
        }
    }

    /// Attaches a transport lifecycle implementation for forum topic tools.
    #[must_use]
    pub fn with_topic_lifecycle(mut self, topic_lifecycle: Arc<dyn ManagerTopicLifecycle>) -> Self {
        self.topic_lifecycle = Some(topic_lifecycle);
        self
    }

    /// Overrides sandbox cleanup strategy for forum topic deletion flows.
    #[must_use]
    pub fn with_topic_sandbox_cleanup(
        mut self,
        sandbox_cleanup: Arc<dyn ManagerTopicSandboxCleanup>,
    ) -> Self {
        self.sandbox_cleanup = sandbox_cleanup;
        self
    }

    /// Overrides sandbox inventory/control strategy for manager sandbox tools.
    #[must_use]
    pub fn with_topic_sandbox_control(
        mut self,
        sandbox_control: Arc<dyn ManagerTopicSandboxControl>,
    ) -> Self {
        self.sandbox_control = sandbox_control;
        self
    }

    async fn persist_forum_topic_catalog_entry(
        &self,
        entry: &ForumTopicCatalogEntry,
    ) -> Result<()> {
        let mut config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for {}: {err}", entry.topic_id))?;
        Self::upsert_forum_topic_catalog_entry(&mut config, entry);
        self.storage
            .update_user_config(self.user_id, config)
            .await
            .map_err(|err| anyhow!("failed to update user config for {}: {err}", entry.topic_id))
    }

    async fn list_forum_topic_catalog_entries(
        &self,
        requested_chat_id: Option<i64>,
        include_closed: bool,
    ) -> Result<Vec<ForumTopicCatalogEntry>> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for forum topic listing: {err}"))?;
        let effective_chat_id = requested_chat_id.or_else(|| self.resolve_default_forum_chat_id());
        let mut topics = config
            .contexts
            .iter()
            .filter_map(|(context_key, context)| {
                Self::forum_topic_catalog_entry_from_context(context_key, context)
            })
            .filter(|entry| effective_chat_id.is_none_or(|chat_id| entry.chat_id == chat_id))
            .filter(|entry| include_closed || !entry.closed)
            .collect::<Vec<_>>();
        topics.sort_by_key(|entry| (entry.chat_id, entry.thread_id));
        Ok(topics)
    }

    async fn cleanup_forum_topic_artifacts(
        &self,
        topic: &ForumTopicActionResult,
    ) -> (serde_json::Value, Option<String>) {
        let context_key = Self::forum_topic_context_key(topic.chat_id, topic.thread_id);
        let binding_keys = Self::forum_topic_binding_keys(topic.chat_id, topic.thread_id);
        let mut errors = Vec::new();
        let deleted_agent_memory = self
            .clear_forum_topic_agent_memory(&context_key, &mut errors)
            .await;
        let deleted_chat_history_for_context = self
            .clear_forum_topic_chat_history(&context_key, &mut errors)
            .await;
        let deleted_chat_history = deleted_chat_history_for_context;
        let deleted_topic_context = self
            .delete_forum_topic_context_record(&context_key, &mut errors)
            .await;
        let deleted_topic_agents_md = self
            .delete_forum_topic_agents_md_record(&context_key, &mut errors)
            .await;
        let deleted_topic_infra = self
            .delete_forum_topic_infra_record(&context_key, &mut errors)
            .await;
        self.delete_forum_topic_bindings(&binding_keys, &mut errors)
            .await;
        let removed_context_config = self
            .remove_forum_topic_context_config(&context_key, &mut errors)
            .await;
        let deleted_container = self
            .cleanup_forum_topic_sandbox(topic, &context_key, &mut errors)
            .await;

        let cleanup = json!({
            "context_key": context_key,
            "deleted_chat_history": deleted_chat_history,
            "deleted_chat_history_for_context": deleted_chat_history_for_context,
            "deleted_agent_memory": deleted_agent_memory,
            "deleted_topic_context": deleted_topic_context,
            "deleted_topic_agents_md": deleted_topic_agents_md,
            "deleted_topic_infra": deleted_topic_infra,
            "deleted_topic_binding_keys": binding_keys,
            "removed_context_config": removed_context_config,
            "deleted_container": deleted_container,
            "errors": errors,
        });

        let error = cleanup
            .get("errors")
            .and_then(|value| value.as_array())
            .filter(|errors| !errors.is_empty())
            .map(|errors| {
                errors
                    .iter()
                    .filter_map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            });

        (cleanup, error)
    }

    async fn clear_forum_topic_agent_memory(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .clear_agent_memory_for_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to clear agent memory for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn clear_forum_topic_chat_history(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .clear_chat_history_for_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to clear chat history for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_context_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_context(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic context for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_agents_md_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_agents_md(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic AGENTS.md for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_infra_record(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .storage
            .delete_topic_infra_config(self.user_id, context_key.to_string())
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to delete topic infra config for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn delete_forum_topic_bindings(&self, binding_keys: &[String], errors: &mut Vec<String>) {
        for topic_binding_key in binding_keys {
            if let Err(err) = self
                .storage
                .delete_topic_binding(self.user_id, topic_binding_key.clone())
                .await
            {
                errors.push(format!(
                    "failed to delete topic binding {topic_binding_key}: {err}"
                ));
            }
        }
    }

    async fn remove_forum_topic_context_config(
        &self,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self.storage.get_user_config(self.user_id).await {
            Ok(mut config) => {
                let removed_context_config = config.contexts.remove(context_key).is_some();
                if let Err(err) = self.storage.update_user_config(self.user_id, config).await {
                    errors.push(format!(
                        "failed to update user config for {context_key}: {err}"
                    ));
                }
                removed_context_config
            }
            Err(err) => {
                errors.push(format!(
                    "failed to load user config for {context_key}: {err}"
                ));
                false
            }
        }
    }

    async fn cleanup_forum_topic_sandbox(
        &self,
        topic: &ForumTopicActionResult,
        context_key: &str,
        errors: &mut Vec<String>,
    ) -> bool {
        match self
            .sandbox_cleanup
            .cleanup_topic_sandbox(self.user_id, topic)
            .await
        {
            Ok(()) => true,
            Err(err) => {
                errors.push(format!(
                    "failed to destroy sandbox for {context_key}: {err}"
                ));
                false
            }
        }
    }

    fn topic_binding_set_parameters() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "topic_id": { "type": "string", "description": "Stable topic identifier. For Telegram forum topics use the canonical '<chat_id>:<thread_id>' value returned by forum_topic_create or forum_topic_provision_ssh_agent; topic names are resolved only as a convenience alias." },
                "agent_id": { "type": "string", "description": "Target agent identifier" },
                "binding_kind": { "type": "string", "enum": ["manual", "runtime"], "description": "Binding source kind" },
                "chat_id": { "type": ["integer", "null"], "description": "Optional transport chat identifier; null clears stored value" },
                "thread_id": { "type": ["integer", "null"], "description": "Optional transport thread identifier; null clears stored value" },
                "expires_at": { "type": ["integer", "null"], "description": "Optional expiry unix timestamp; null clears stored value" },
                "last_activity_at": { "type": "integer", "description": "Optional last activity unix timestamp" },
                "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
            },
            "required": ["topic_id", "agent_id"]
        })
    }

    fn forum_topic_icon_color_schema() -> serde_json::Value {
        json!({
            "type": "integer",
            "enum": TELEGRAM_FORUM_ICON_COLORS,
            "description": "Optional Telegram forum icon color"
        })
    }

    fn base_tools_definitions() -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        tools.extend(Self::topic_binding_tools_definitions());
        tools.extend(Self::topic_context_tools_definitions());
        tools.extend(Self::topic_agents_md_tools_definitions());
        tools.extend(Self::topic_infra_tools_definitions());
        tools.extend(Self::private_secret_tools_definitions());
        tools.extend(Self::topic_sandbox_tools_definitions());
        tools.extend(Self::agent_profile_tools_definitions());
        tools
    }

    fn topic_sandbox_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_LIST.to_string(),
                description: "List user-owned topic sandbox containers and orphan status"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "orphaned_only": { "type": "boolean", "description": "Return only containers that look orphaned or disabled" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_GET.to_string(),
                description: "Inspect a topic sandbox by topic_id or Docker container name"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "container_name": { "type": "string", "description": "Exact Docker container name" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                description: "Ensure a sandbox container exists for a tracked forum topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                description: "Recreate a topic sandbox container, wiping previous workspace state"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                description:
                    "Delete a topic sandbox container by topic_id or Docker container name"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Canonical topic identifier or unique forum topic alias" },
                        "container_name": { "type": "string", "description": "Exact Docker container name" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    }
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                description:
                    "Delete orphaned or disabled topic sandbox containers for the current user"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "reason": { "type": "string", "enum": ["topic_missing", "binding_missing", "sandbox_disabled", "all"], "description": "Which orphan class to delete; defaults to all" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Docker" }
                    }
                }),
            },
        ]
    }

    fn private_secret_tools_definitions() -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: TOOL_PRIVATE_SECRET_PROBE.to_string(),
            description: "Probe a private secret ref without exposing its content".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "secret_ref": { "type": "string", "description": "Opaque secret reference (for example storage:vds or env:SSH_KEY)" },
                    "kind": { "type": "string", "enum": ["opaque", "ssh_private_key"], "description": "Optional probe mode; defaults to opaque" }
                },
                "required": ["secret_ref"]
            }),
        }]
    }

    fn forum_topic_provision_ssh_agent_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
            description: "Atomically create a Telegram forum topic, derive the canonical topic_id, create an SSH-ready agent profile, bind the topic, and attach topic-scoped SSH infra"
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "chat_id": { "type": "integer", "description": "Optional target forum chat id; omit to use the current manager forum chat" },
                    "name": { "type": "string", "description": "Forum topic name; also used as default agent_id and target_name when omitted" },
                    "icon_color": Self::forum_topic_icon_color_schema(),
                    "icon_custom_emoji_id": { "type": "string", "description": "Optional custom emoji icon id" },
                    "agent_id": { "type": "string", "description": "Optional explicit agent id; defaults to the topic name" },
                    "system_prompt": { "type": "string", "description": "Optional agent system prompt instructions" },
                    "description": { "type": "string", "description": "Optional human-readable profile description" },
                    "topic_context": { "type": "string", "description": "Optional persistent topic context" },
                    "target_name": { "type": "string", "description": "Optional infra target name; defaults to the topic name" },
                    "host": { "type": "string", "description": "SSH host or DNS name" },
                    "port": { "type": "integer", "description": "SSH port, defaults to 22" },
                    "remote_user": { "type": "string", "description": "Remote SSH username" },
                    "auth_mode": { "type": "string", "enum": ["none", "password", "private_key"], "description": "SSH authentication mode" },
                    "secret_ref": { "type": "string", "description": "Opaque secret reference for SSH auth material" },
                    "sudo_secret_ref": { "type": "string", "description": "Opaque secret reference for sudo password material" },
                    "environment": { "type": "string", "description": "Optional environment label such as prod or stage" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional free-form target tags" },
                    "allowed_tool_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Allowlisted SSH tool modes; defaults to all SSH modes" },
                    "approval_required_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Modes that always require approval; defaults to sudo_exec and apply_file_edit" },
                    "dry_run": { "type": "boolean", "description": "Validate and preview without mutating Telegram or storage" }
                },
                "required": ["name", "host", "remote_user", "auth_mode"]
            }),
        }
    }

    fn lifecycle_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            Self::forum_topic_provision_ssh_agent_definition(),
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_CREATE.to_string(),
                description: "Create Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "name": { "type": "string", "description": "Forum topic name" },
                        "icon_color": Self::forum_topic_icon_color_schema(),
                        "icon_custom_emoji_id": { "type": "string", "description": "Optional custom emoji icon id" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["name"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_EDIT.to_string(),
                description: "Edit Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "name": { "type": "string", "description": "Optional new topic name" },
                        "icon_custom_emoji_id": { "type": "string", "description": "Optional icon emoji id; empty clears icon" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_CLOSE.to_string(),
                description: "Close Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_REOPEN.to_string(),
                description: "Reopen Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_DELETE.to_string(),
                description: "Delete Telegram forum topic; omit chat_id to use current forum chat"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "thread_id": { "type": "integer", "description": "Forum topic thread identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without mutation" }
                    },
                    "required": ["thread_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_FORUM_TOPIC_LIST.to_string(),
                description:
                    "List active Telegram forum topics tracked in persisted S3 topic records"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "chat_id": { "type": "integer", "description": "Optional target chat identifier; omit to use the current forum chat when available" },
                        "include_closed": { "type": "boolean", "description": "Include closed topics in the result" }
                    }
                }),
            },
        ]
    }

    fn tools_definitions(&self) -> Vec<ToolDefinition> {
        let mut tools = Self::base_tools_definitions();
        if self.topic_lifecycle.is_some() {
            tools.extend(Self::lifecycle_tools_definitions());
        }

        tools
    }

    fn topic_sandbox_scope(&self, topic_id: &str) -> Result<SandboxScope> {
        let (chat_id, thread_id) = Self::parse_canonical_forum_topic_id(topic_id).ok_or_else(|| {
            anyhow!(
                "topic_id '{topic_id}' is not a canonical Telegram forum topic id. Use '<chat_id>:<thread_id>'"
            )
        })?;

        Ok(SandboxScope::new(self.user_id, topic_id.to_string())
            .with_transport_metadata(Some(chat_id), Some(thread_id)))
    }

    fn forum_topic_action_from_topic_id(topic_id: &str) -> Option<ForumTopicActionResult> {
        let (chat_id, thread_id) = Self::parse_canonical_forum_topic_id(topic_id)?;
        Some(ForumTopicActionResult { chat_id, thread_id })
    }

    fn prune_reason_matches(
        reason: TopicSandboxPruneReason,
        record: &TopicSandboxInventoryRecord,
    ) -> bool {
        match reason {
            TopicSandboxPruneReason::TopicMissing => {
                record.orphan_reason.as_deref() == Some("topic_missing")
            }
            TopicSandboxPruneReason::BindingMissing => {
                record.orphan_reason.as_deref() == Some("binding_missing")
            }
            TopicSandboxPruneReason::SandboxDisabled => {
                record.orphan_reason.as_deref() == Some("sandbox_disabled")
            }
            TopicSandboxPruneReason::All => matches!(
                record.orphan_reason.as_deref(),
                Some("topic_missing" | "binding_missing" | "sandbox_disabled")
            ),
        }
    }

    async fn ensure_tracked_forum_topic(&self, topic_id: &str) -> Result<()> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for topic sandbox: {err}"))?;
        if config.contexts.contains_key(topic_id) {
            return Ok(());
        }

        bail!("topic_id '{topic_id}' is not tracked in the user topic catalog")
    }

    async fn build_topic_sandbox_inventory(
        &self,
        containers: Vec<SandboxContainerRecord>,
    ) -> Result<Vec<TopicSandboxInventoryRecord>> {
        let config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| {
                anyhow!("failed to load user config for topic sandbox inventory: {err}")
            })?;

        let mut records = Vec::with_capacity(containers.len());
        for container in containers {
            let topic_id = container.scope.clone();
            let canonical_topic_id = topic_id
                .as_deref()
                .filter(|topic_id| Self::is_canonical_forum_topic_id(topic_id))
                .map(str::to_string);
            let bound_topic_exists = canonical_topic_id
                .as_ref()
                .is_some_and(|topic_id| config.contexts.contains_key(topic_id));

            let (binding_found, sandbox_tools_enabled) =
                if let Some(topic_id) = canonical_topic_id.as_ref() {
                    let binding = self
                        .storage
                        .get_topic_binding(self.user_id, topic_id.clone())
                        .await
                        .map_err(|err| {
                            anyhow!("failed to get topic binding for topic sandbox: {err}")
                        })?;

                    if let Some(binding) = binding {
                        let catalog = self.topic_agent_tool_catalog(topic_id).await?;
                        let profile = self
                            .storage
                            .get_agent_profile(self.user_id, binding.agent_id)
                            .await
                            .map_err(|err| {
                                anyhow!("failed to get agent profile for topic sandbox: {err}")
                            })?;
                        let snapshot = Self::topic_agent_tool_snapshot(
                            &catalog,
                            profile.as_ref().map(|profile| &profile.profile),
                        );
                        (true, Some(Self::sandbox_provider_enabled(&snapshot)))
                    } else {
                        (false, None)
                    }
                } else {
                    (false, None)
                };

            let orphan_reason = if canonical_topic_id.is_none() {
                Some("non_topic_scope".to_string())
            } else if !bound_topic_exists {
                Some("topic_missing".to_string())
            } else if !binding_found {
                Some("binding_missing".to_string())
            } else if sandbox_tools_enabled == Some(false) {
                Some("sandbox_disabled".to_string())
            } else {
                None
            };

            records.push(TopicSandboxInventoryRecord {
                container_id: container.container_id,
                container_name: container.container_name,
                image: container.image,
                created_at: container.created_at,
                state: container.state,
                status: container.status,
                running: container.running,
                topic_id,
                chat_id: container.chat_id,
                thread_id: container.thread_id,
                labels: container.labels,
                bound_topic_exists,
                binding_found,
                sandbox_tools_enabled,
                orphan_reason,
            });
        }

        records.sort_by(|left, right| left.container_name.cmp(&right.container_name));
        Ok(records)
    }

    async fn get_topic_sandbox_inventory_by_name(
        &self,
        container_name: &str,
    ) -> Result<Option<TopicSandboxInventoryRecord>> {
        let Some(container) = self
            .sandbox_control
            .get_topic_sandbox(self.user_id, container_name)
            .await?
        else {
            return Ok(None);
        };

        Ok(self
            .build_topic_sandbox_inventory(vec![container])
            .await?
            .into_iter()
            .next())
    }

    async fn get_topic_sandbox_inventory_by_topic(
        &self,
        topic_id: &str,
    ) -> Result<Option<TopicSandboxInventoryRecord>> {
        let scope = self.topic_sandbox_scope(topic_id)?;
        self.get_topic_sandbox_inventory_by_name(&scope.container_name())
            .await
    }

    async fn resolve_topic_sandbox_target(
        &self,
        topic_id: Option<String>,
        container_name: Option<String>,
        mutation: bool,
    ) -> Result<TopicSandboxTarget> {
        match (topic_id, container_name) {
            (Some(topic_id), None) => {
                let topic_id = if mutation {
                    self.resolve_mutation_topic_id(topic_id).await?
                } else {
                    self.resolve_lookup_topic_id(topic_id).await?
                };
                Ok(TopicSandboxTarget::TopicId(topic_id))
            }
            (None, Some(container_name)) => Ok(TopicSandboxTarget::ContainerName(
                Self::validate_non_empty(container_name, "container_name")?,
            )),
            (Some(_), Some(_)) => {
                bail!("provide either topic_id or container_name, not both")
            }
            (None, None) => bail!("either topic_id or container_name is required"),
        }
    }

    async fn cleanup_topic_sandbox_for_topic_id(&self, topic_id: &str) -> serde_json::Value {
        let Some(topic) = Self::forum_topic_action_from_topic_id(topic_id) else {
            return json!({
                "skipped": true,
                "reason": "topic_id is not a canonical Telegram forum topic id"
            });
        };

        match self
            .sandbox_cleanup
            .cleanup_topic_sandbox(self.user_id, &topic)
            .await
        {
            Ok(()) => json!({
                "skipped": false,
                "deleted_container": true,
                "topic_id": topic_id,
            }),
            Err(err) => json!({
                "skipped": false,
                "deleted_container": false,
                "topic_id": topic_id,
                "error": err.to_string(),
            }),
        }
    }

    fn forum_topic_payload(result: &ForumTopicCreateResult) -> serde_json::Value {
        json!({
            "chat_id": result.chat_id,
            "thread_id": result.thread_id,
            "topic_id": Self::forum_topic_context_key(result.chat_id, result.thread_id),
            "name": result.name,
            "icon_color": result.icon_color,
            "icon_custom_emoji_id": result.icon_custom_emoji_id,
        })
    }

    fn build_default_ssh_agent_profile(
        agent_id: &str,
        topic_name: &str,
        system_prompt: Option<String>,
        description: Option<String>,
        host: &str,
    ) -> serde_json::Value {
        let default_description = format!("SSH agent for managing server at {host}");
        json!({
            "name": topic_name,
            "agentId": agent_id,
            "description": description.unwrap_or(default_description),
            "systemPrompt": system_prompt,
            "allowedTools": default_ssh_agent_allowed_tools(),
            "blockedTools": topic_agent_default_blocked_tools(),
        })
    }

    fn build_forum_topic_provision_plan(
        &self,
        args: ForumTopicProvisionSshAgentArgs,
    ) -> Result<ForumTopicProvisionSshAgentPlan> {
        let name = Self::validate_non_empty(args.name, "name")?;
        let icon_custom_emoji_id =
            Self::validate_optional_non_empty(args.icon_custom_emoji_id, "icon_custom_emoji_id")?;
        let icon_color = Self::validate_forum_icon_color(args.icon_color)?;
        let agent_id = Self::validate_optional_non_empty(args.agent_id, "agent_id")?
            .unwrap_or_else(|| name.clone());
        let system_prompt = Self::validate_optional_non_empty(args.system_prompt, "system_prompt")?;
        let description = Self::validate_optional_non_empty(args.description, "description")?;
        let topic_context = Self::validate_optional_non_empty(args.topic_context, "topic_context")?;
        let target_name = Self::validate_optional_non_empty(args.target_name, "target_name")?
            .unwrap_or_else(|| name.clone());
        let host = Self::validate_non_empty(args.host, "host")?;
        let remote_user = Self::validate_non_empty(args.remote_user, "remote_user")?;
        if args.port == 0 {
            bail!("port must be a positive integer");
        }

        let secret_ref = Self::validate_optional_non_empty(args.secret_ref, "secret_ref")?;
        let sudo_secret_ref =
            Self::validate_optional_non_empty(args.sudo_secret_ref, "sudo_secret_ref")?;
        let environment = Self::validate_optional_non_empty(args.environment, "environment")?;
        let tags = Self::normalize_tags(args.tags);
        let allowed_tool_modes = Self::normalize_tool_modes(args.allowed_tool_modes);
        if allowed_tool_modes.is_empty() {
            bail!("allowed_tool_modes must not be empty");
        }
        let approval_required_modes = Self::normalize_tool_modes(args.approval_required_modes);
        let profile = Self::validate_profile_object(Self::build_default_ssh_agent_profile(
            &agent_id,
            &name,
            system_prompt,
            description,
            &host,
        ))?;

        Ok(ForumTopicProvisionSshAgentPlan {
            request: ForumTopicCreateRequest {
                chat_id: args.chat_id,
                name,
                icon_color,
                icon_custom_emoji_id,
            },
            agent_id,
            profile,
            topic_context,
            target_name,
            host,
            port: args.port,
            remote_user,
            auth_mode: args.auth_mode,
            secret_ref,
            sudo_secret_ref,
            environment,
            tags,
            allowed_tool_modes,
            approval_required_modes,
            dry_run: args.dry_run,
        })
    }

    async fn dry_run_forum_topic_provision_ssh_agent(
        &self,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> Result<String> {
        let preview_infra =
            self.topic_infra_preview_record_from_plan("<created_topic_id>".to_string(), plan);
        let preview_preflight = self.inspect_topic_infra_record(&preview_infra).await;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(plan.agent_id.clone()),
                action: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
                payload: json!({
                    "name": plan.request.name,
                    "agent_id": plan.agent_id,
                    "host": plan.host,
                    "port": plan.port,
                    "remote_user": plan.remote_user,
                    "auth_mode": plan.auth_mode,
                    "secret_ref": plan.secret_ref,
                    "sudo_secret_ref": plan.sudo_secret_ref,
                    "topic_context": plan.topic_context,
                    "outcome": Self::dry_run_outcome(true)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "forum_topic_request": plan.request,
                    "agent_id": plan.agent_id,
                    "profile": plan.profile,
                    "topic_context": plan.topic_context,
                    "topic_infra": Self::topic_infra_value_from_record(&preview_infra),
                    "preflight": preview_preflight,
                    "canonical_topic_id_note": "topic_id will be derived automatically as '<chat_id>:<thread_id>' after Telegram creates the topic"
                }
            }),
            audit_status,
        ))
    }

    async fn execute_forum_topic_provision_substeps(
        &self,
        topic_id: &str,
        created_topic: &ForumTopicCreateResult,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> Result<(String, Option<String>, String, String)> {
        let profile_response = self
            .execute_agent_profile_upsert(&Self::to_json_string(json!({
                "agent_id": plan.agent_id,
                "profile": plan.profile,
            }))?)
            .await?;
        let topic_context_response = match plan.topic_context.as_ref() {
            Some(context) => Some(
                self.execute_topic_context_upsert(&Self::to_json_string(json!({
                    "topic_id": topic_id,
                    "context": context,
                }))?)
                .await?,
            ),
            None => None,
        };
        let binding_response = self
            .execute_topic_binding_set(&Self::to_json_string(json!({
                "topic_id": topic_id,
                "agent_id": plan.agent_id,
                "binding_kind": "manual",
                "chat_id": created_topic.chat_id,
                "thread_id": created_topic.thread_id,
            }))?)
            .await?;
        let infra_response = self
            .execute_topic_infra_upsert(&Self::to_json_string(json!({
                "topic_id": topic_id,
                "target_name": plan.target_name,
                "host": plan.host,
                "port": plan.port,
                "remote_user": plan.remote_user,
                "auth_mode": plan.auth_mode,
                "secret_ref": plan.secret_ref,
                "sudo_secret_ref": plan.sudo_secret_ref,
                "environment": plan.environment,
                "tags": plan.tags,
                "allowed_tool_modes": plan.allowed_tool_modes,
                "approval_required_modes": plan.approval_required_modes,
            }))?)
            .await?;

        Ok((
            profile_response,
            topic_context_response,
            binding_response,
            infra_response,
        ))
    }

    async fn cleanup_failed_forum_topic_provision(&self, created_topic: &ForumTopicCreateResult) {
        if let Some(lifecycle) = &self.topic_lifecycle {
            let _ = lifecycle
                .forum_topic_delete(ForumTopicThreadRequest {
                    chat_id: Some(created_topic.chat_id),
                    thread_id: created_topic.thread_id,
                })
                .await;
        }
    }

    async fn execute_private_secret_probe(&self, arguments: &str) -> Result<String> {
        let args: PrivateSecretProbeArgs = Self::parse_args(arguments, TOOL_PRIVATE_SECRET_PROBE)?;
        let secret_ref = Self::validate_non_empty(args.secret_ref, "secret_ref")?;
        let report = probe_secret_ref(&self.storage, self.user_id, &secret_ref, args.kind).await;

        Self::to_json_string(json!({
            "ok": true,
            "secret_probe": report
        }))
    }

    async fn execute_forum_topic_provision_ssh_agent(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicProvisionSshAgentArgs =
            Self::parse_args(arguments, TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT)?;
        let plan = self.build_forum_topic_provision_plan(args)?;
        if plan.dry_run {
            return self.dry_run_forum_topic_provision_ssh_agent(&plan).await;
        }

        let created_topic = self
            .topic_lifecycle()?
            .forum_topic_create(plan.request.clone())
            .await?;
        let topic_id =
            Self::forum_topic_context_key(created_topic.chat_id, created_topic.thread_id);
        self.persist_forum_topic_catalog_entry(&ForumTopicCatalogEntry {
            topic_id: topic_id.clone(),
            chat_id: created_topic.chat_id,
            thread_id: created_topic.thread_id,
            name: Some(created_topic.name.clone()),
            icon_color: Some(created_topic.icon_color),
            icon_custom_emoji_id: created_topic.icon_custom_emoji_id.clone(),
            closed: false,
        })
        .await?;

        let (profile_response, topic_context_response, binding_response, infra_response) =
            match self
                .execute_forum_topic_provision_substeps(&topic_id, &created_topic, &plan)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    self.cleanup_failed_forum_topic_provision(&created_topic)
                        .await;
                    return Err(error);
                }
            };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: Some(plan.agent_id.clone()),
                action: TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "agent_id": plan.agent_id,
                    "host": plan.host,
                    "port": plan.port,
                    "remote_user": plan.remote_user,
                    "auth_mode": plan.auth_mode,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let parsed_profile: serde_json::Value = serde_json::from_str(&profile_response)
            .map_err(|err| anyhow!("failed to parse profile response: {err}"))?;
        let parsed_binding: serde_json::Value = serde_json::from_str(&binding_response)
            .map_err(|err| anyhow!("failed to parse binding response: {err}"))?;
        let parsed_infra: serde_json::Value = serde_json::from_str(&infra_response)
            .map_err(|err| anyhow!("failed to parse infra response: {err}"))?;
        let parsed_context = match topic_context_response {
            Some(response) => Some(
                serde_json::from_str::<serde_json::Value>(&response)
                    .map_err(|err| anyhow!("failed to parse topic context response: {err}"))?,
            ),
            None => None,
        };

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "provisioned": true,
                "topic": Self::forum_topic_payload(&created_topic),
                "binding": parsed_binding.get("binding").cloned().unwrap_or(serde_json::Value::Null),
                "profile": parsed_profile.get("profile").cloned().unwrap_or(serde_json::Value::Null),
                "topic_context": parsed_context.as_ref().and_then(|value| value.get("topic_context")).cloned(),
                "topic_infra": parsed_infra.get("topic_infra").cloned().unwrap_or(serde_json::Value::Null),
                "preflight": parsed_infra.get("preflight").cloned().unwrap_or(serde_json::Value::Null),
            }),
            audit_status,
        ))
    }

    async fn execute_forum_topic_create(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicCreateArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_CREATE)?;
        let name = Self::validate_non_empty(args.name, "name")?;
        let icon_custom_emoji_id =
            Self::validate_optional_non_empty(args.icon_custom_emoji_id, "icon_custom_emoji_id")?;
        let icon_color = Self::validate_forum_icon_color(args.icon_color)?;
        let request = ForumTopicCreateRequest {
            chat_id: args.chat_id,
            name,
            icon_color,
            icon_custom_emoji_id,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_FORUM_TOPIC_CREATE.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": TOOL_FORUM_TOPIC_CREATE,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let result = self
            .topic_lifecycle()?
            .forum_topic_create(request.clone())
            .await?;
        self.persist_forum_topic_catalog_entry(&ForumTopicCatalogEntry {
            topic_id: Self::forum_topic_context_key(result.chat_id, result.thread_id),
            chat_id: result.chat_id,
            thread_id: result.thread_id,
            name: Some(result.name.clone()),
            icon_color: Some(result.icon_color),
            icon_custom_emoji_id: result.icon_custom_emoji_id.clone(),
            closed: false,
        })
        .await?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(Self::forum_topic_context_key(
                    result.chat_id,
                    result.thread_id,
                )),
                agent_id: None,
                action: TOOL_FORUM_TOPIC_CREATE.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({ "ok": true, "topic": Self::forum_topic_payload(&result) }),
            audit_status,
        );
        Self::to_json_string(response)
    }

    async fn execute_forum_topic_edit(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicEditArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_EDIT)?;
        let thread_id = Self::validate_thread_id(args.thread_id)?;
        let name = Self::validate_optional_non_empty(args.name, "name")?;
        if name.is_none() && args.icon_custom_emoji_id.is_none() {
            bail!("forum_topic_edit requires at least one mutable field");
        }
        let request = ForumTopicEditRequest {
            chat_id: args.chat_id,
            thread_id,
            name,
            icon_custom_emoji_id: args.icon_custom_emoji_id,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_FORUM_TOPIC_EDIT.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": TOOL_FORUM_TOPIC_EDIT,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let result = self
            .topic_lifecycle()?
            .forum_topic_edit(request.clone())
            .await?;
        let topic_id = Self::forum_topic_context_key(result.chat_id, result.thread_id);
        let mut config = self
            .storage
            .get_user_config(self.user_id)
            .await
            .map_err(|err| anyhow!("failed to load user config for {topic_id}: {err}"))?;
        let mut entry = Self::existing_forum_topic_catalog_entry(&config, &topic_id).unwrap_or(
            ForumTopicCatalogEntry {
                topic_id: topic_id.clone(),
                chat_id: result.chat_id,
                thread_id: result.thread_id,
                name: None,
                icon_color: None,
                icon_custom_emoji_id: None,
                closed: false,
            },
        );
        if let Some(name) = result.name.clone() {
            entry.name = Some(name);
        }
        if result.icon_custom_emoji_id.is_some() {
            entry.icon_custom_emoji_id = result.icon_custom_emoji_id.clone();
        }
        Self::upsert_forum_topic_catalog_entry(&mut config, &entry);
        self.storage
            .update_user_config(self.user_id, config)
            .await
            .map_err(|err| anyhow!("failed to update user config for {topic_id}: {err}"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_FORUM_TOPIC_EDIT.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "topic": result }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_forum_topic_thread_action(
        &self,
        arguments: &str,
        tool_name: &str,
    ) -> Result<String> {
        let args: ForumTopicThreadArgs = Self::parse_args(arguments, tool_name)?;
        let request = ForumTopicThreadRequest {
            chat_id: args.chat_id,
            thread_id: Self::validate_thread_id(args.thread_id)?,
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: tool_name.to_string(),
                    payload: json!({
                        "request": request,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": tool_name,
                        "request": request
                    }
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let lifecycle = self.topic_lifecycle()?;
        let result = match tool_name {
            TOOL_FORUM_TOPIC_CLOSE => lifecycle.forum_topic_close(request.clone()).await?,
            TOOL_FORUM_TOPIC_REOPEN => lifecycle.forum_topic_reopen(request.clone()).await?,
            TOOL_FORUM_TOPIC_DELETE => lifecycle.forum_topic_delete(request.clone()).await?,
            _ => bail!("unsupported forum topic thread action: {tool_name}"),
        };
        let derived_topic_id = Self::forum_topic_context_key(result.chat_id, result.thread_id);
        if tool_name != TOOL_FORUM_TOPIC_DELETE {
            let mut config = self
                .storage
                .get_user_config(self.user_id)
                .await
                .map_err(|err| {
                    anyhow!("failed to load user config for {derived_topic_id}: {err}")
                })?;
            let mut entry = Self::existing_forum_topic_catalog_entry(&config, &derived_topic_id)
                .unwrap_or(ForumTopicCatalogEntry {
                    topic_id: derived_topic_id.clone(),
                    chat_id: result.chat_id,
                    thread_id: result.thread_id,
                    name: None,
                    icon_color: None,
                    icon_custom_emoji_id: None,
                    closed: tool_name == TOOL_FORUM_TOPIC_CLOSE,
                });
            entry.closed = tool_name == TOOL_FORUM_TOPIC_CLOSE;
            Self::upsert_forum_topic_catalog_entry(&mut config, &entry);
            self.storage
                .update_user_config(self.user_id, config)
                .await
                .map_err(|err| {
                    anyhow!("failed to update user config for {derived_topic_id}: {err}")
                })?;
        }
        let (cleanup, cleanup_error) = if tool_name == TOOL_FORUM_TOPIC_DELETE {
            self.cleanup_forum_topic_artifacts(&result).await
        } else {
            (json!({ "skipped": true }), None)
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(derived_topic_id),
                agent_id: None,
                action: tool_name.to_string(),
                payload: json!({
                    "request": request,
                    "result": result,
                    "cleanup": cleanup,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        if let Some(cleanup_error) = cleanup_error {
            bail!("forum topic deleted but cleanup failed: {cleanup_error}");
        }

        let response = Self::attach_audit_status(
            json!({ "ok": true, "topic": result, "cleanup": cleanup }),
            audit_status,
        );
        Self::to_json_string(response)
    }

    async fn execute_forum_topic_list(&self, arguments: &str) -> Result<String> {
        let args: ForumTopicListArgs = Self::parse_args(arguments, TOOL_FORUM_TOPIC_LIST)?;
        let effective_chat_id = args
            .chat_id
            .or_else(|| self.resolve_default_forum_chat_id());
        let topics = self
            .list_forum_topic_catalog_entries(args.chat_id, args.include_closed)
            .await?;
        Self::to_json_string(json!({
            "ok": true,
            "chat_id": effective_chat_id,
            "include_closed": args.include_closed,
            "count": topics.len(),
            "topics": topics,
        }))
    }

    async fn execute_topic_sandbox_list(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxListArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_LIST)?;
        let sandboxes = self
            .build_topic_sandbox_inventory(
                self.sandbox_control
                    .list_topic_sandboxes(self.user_id)
                    .await?,
            )
            .await?;
        let sandboxes = if args.orphaned_only {
            sandboxes
                .into_iter()
                .filter(|record| Self::prune_reason_matches(TopicSandboxPruneReason::All, record))
                .collect::<Vec<_>>()
        } else {
            sandboxes
        };

        Self::to_json_string(json!({
            "ok": true,
            "orphaned_only": args.orphaned_only,
            "count": sandboxes.len(),
            "sandboxes": sandboxes,
        }))
    }

    async fn execute_topic_sandbox_get(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxGetArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_GET)?;
        let target = self
            .resolve_topic_sandbox_target(args.topic_id, args.container_name, false)
            .await?;
        let sandbox = match &target {
            TopicSandboxTarget::TopicId(topic_id) => {
                self.get_topic_sandbox_inventory_by_topic(topic_id).await?
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                self.get_topic_sandbox_inventory_by_name(container_name)
                    .await?
            }
        };

        let response = match target {
            TopicSandboxTarget::TopicId(topic_id) => json!({
                "ok": true,
                "found": sandbox.is_some(),
                "topic_id": topic_id,
                "sandbox": sandbox,
            }),
            TopicSandboxTarget::ContainerName(container_name) => json!({
                "ok": true,
                "found": sandbox.is_some(),
                "container_name": container_name,
                "sandbox": sandbox,
            }),
        };

        Self::to_json_string(response)
    }

    async fn execute_topic_sandbox_create(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxCreateArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_CREATE)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        self.ensure_tracked_forum_topic(&topic_id).await?;
        let scope = self.topic_sandbox_scope(&topic_id)?;
        let previous = self.get_topic_sandbox_inventory_by_topic(&topic_id).await?;
        let container_name = scope.container_name();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "container_name": container_name,
                        "previous": previous.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "create",
                        "topic_id": topic_id,
                        "container_name": container_name,
                    },
                    "previous": previous,
                }),
                audit_status,
            ));
        }

        let sandbox = self
            .build_topic_sandbox_inventory(vec![
                self.sandbox_control.ensure_topic_sandbox(scope).await?,
            ])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("topic sandbox inventory is empty after create"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_CREATE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": sandbox.container_name.clone(),
                    "previous": previous,
                    "sandbox": sandbox.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "sandbox": sandbox,
            }),
            audit_status,
        ))
    }

    async fn execute_topic_sandbox_recreate(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxRecreateArgs =
            Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_RECREATE)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let scope = self.topic_sandbox_scope(&topic_id)?;
        let previous = self.get_topic_sandbox_inventory_by_topic(&topic_id).await?;
        let container_name = scope.container_name();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "container_name": container_name,
                        "previous": previous.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "recreate",
                        "topic_id": topic_id,
                        "container_name": container_name,
                    },
                    "previous": previous,
                }),
                audit_status,
            ));
        }

        let sandbox = self
            .build_topic_sandbox_inventory(vec![
                self.sandbox_control.recreate_topic_sandbox(scope).await?,
            ])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("topic sandbox inventory is empty after recreate"))?;
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_RECREATE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": sandbox.container_name.clone(),
                    "previous": previous,
                    "sandbox": sandbox.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "sandbox": sandbox,
            }),
            audit_status,
        ))
    }

    async fn topic_sandbox_delete_preview(
        &self,
        target: &TopicSandboxTarget,
        previous: Option<TopicSandboxInventoryRecord>,
    ) -> Result<String> {
        let (topic_id, container_name, preview_target) = match target {
            TopicSandboxTarget::TopicId(topic_id) => {
                let scope = self.topic_sandbox_scope(topic_id)?;
                (
                    Some(topic_id.clone()),
                    scope.container_name(),
                    json!({ "topic_id": topic_id }),
                )
            }
            TopicSandboxTarget::ContainerName(container_name) => (
                previous
                    .as_ref()
                    .and_then(|sandbox| sandbox.topic_id.clone()),
                container_name.clone(),
                json!({ "container_name": container_name }),
            ),
        };
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": container_name,
                    "previous": previous.clone(),
                    "outcome": Self::dry_run_outcome(true)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": "delete",
                    "target": preview_target,
                },
                "previous": previous,
            }),
            audit_status,
        ))
    }

    async fn apply_topic_sandbox_delete(
        &self,
        target: TopicSandboxTarget,
        previous: Option<TopicSandboxInventoryRecord>,
    ) -> Result<String> {
        let (deleted, topic_id, container_name) = match target {
            TopicSandboxTarget::TopicId(topic_id) => {
                let scope = self.topic_sandbox_scope(&topic_id)?;
                let deleted = self
                    .sandbox_control
                    .delete_topic_sandbox_by_scope(scope.clone())
                    .await?;
                (deleted, Some(topic_id), scope.container_name())
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                let deleted = self
                    .sandbox_control
                    .delete_topic_sandbox_by_name(self.user_id, &container_name)
                    .await?;
                (
                    deleted,
                    previous
                        .as_ref()
                        .and_then(|sandbox| sandbox.topic_id.clone()),
                    container_name,
                )
            }
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "container_name": container_name,
                    "previous": previous.clone(),
                    "deleted": deleted,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "deleted": deleted,
                "container_name": container_name,
                "sandbox": previous,
            }),
            audit_status,
        ))
    }

    async fn execute_topic_sandbox_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_DELETE)?;
        let target = self
            .resolve_topic_sandbox_target(args.topic_id, args.container_name, true)
            .await?;
        let previous = match &target {
            TopicSandboxTarget::TopicId(topic_id) => {
                self.get_topic_sandbox_inventory_by_topic(topic_id).await?
            }
            TopicSandboxTarget::ContainerName(container_name) => {
                self.get_topic_sandbox_inventory_by_name(container_name)
                    .await?
            }
        };

        if args.dry_run {
            return self.topic_sandbox_delete_preview(&target, previous).await;
        }

        self.apply_topic_sandbox_delete(target, previous).await
    }

    async fn execute_topic_sandbox_prune(&self, arguments: &str) -> Result<String> {
        let args: TopicSandboxPruneArgs = Self::parse_args(arguments, TOOL_TOPIC_SANDBOX_PRUNE)?;
        let candidates = self
            .build_topic_sandbox_inventory(
                self.sandbox_control
                    .list_topic_sandboxes(self.user_id)
                    .await?,
            )
            .await?
            .into_iter()
            .filter(|record| Self::prune_reason_matches(args.reason, record))
            .collect::<Vec<_>>();

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: None,
                    action: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                    payload: json!({
                        "reason": args.reason,
                        "count": candidates.len(),
                        "candidates": candidates.clone(),
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "reason": args.reason,
                    "count": candidates.len(),
                    "candidates": candidates,
                }),
                audit_status,
            ));
        }

        let mut deleted = Vec::new();
        let mut errors = Vec::new();
        for candidate in &candidates {
            match self
                .sandbox_control
                .delete_topic_sandbox_by_name(self.user_id, &candidate.container_name)
                .await
            {
                Ok(true) => deleted.push(candidate.container_name.clone()),
                Ok(false) => {}
                Err(err) => errors.push(format!(
                    "failed to delete {}: {err}",
                    candidate.container_name
                )),
            }
        }

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: None,
                action: TOOL_TOPIC_SANDBOX_PRUNE.to_string(),
                payload: json!({
                    "reason": args.reason,
                    "count": candidates.len(),
                    "candidates": candidates.clone(),
                    "deleted": deleted.clone(),
                    "errors": errors.clone(),
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "reason": args.reason,
                "count": candidates.len(),
                "candidates": candidates,
                "deleted": deleted,
                "errors": errors,
            }),
            audit_status,
        ))
    }
}

#[async_trait]
impl ToolProvider for ManagerControlPlaneProvider {
    fn name(&self) -> &'static str {
        "manager_control_plane"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        self.tools_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        BASE_TOOL_NAMES.contains(&tool_name)
            || (self.topic_lifecycle.is_some() && LIFECYCLE_TOOL_NAMES.contains(&tool_name))
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_TOPIC_BINDING_SET => self.execute_topic_binding_set(arguments).await,
            TOOL_TOPIC_BINDING_GET => self.execute_topic_binding_get(arguments).await,
            TOOL_TOPIC_BINDING_DELETE => self.execute_topic_binding_delete(arguments).await,
            TOOL_TOPIC_BINDING_ROLLBACK => self.execute_topic_binding_rollback(arguments).await,
            TOOL_TOPIC_CONTEXT_UPSERT => self.execute_topic_context_upsert(arguments).await,
            TOOL_TOPIC_CONTEXT_GET => self.execute_topic_context_get(arguments).await,
            TOOL_TOPIC_CONTEXT_DELETE => self.execute_topic_context_delete(arguments).await,
            TOOL_TOPIC_CONTEXT_ROLLBACK => self.execute_topic_context_rollback(arguments).await,
            TOOL_TOPIC_AGENTS_MD_UPSERT => self.execute_topic_agents_md_upsert(arguments).await,
            TOOL_TOPIC_AGENTS_MD_GET => self.execute_topic_agents_md_get(arguments).await,
            TOOL_TOPIC_AGENTS_MD_DELETE => self.execute_topic_agents_md_delete(arguments).await,
            TOOL_TOPIC_AGENTS_MD_ROLLBACK => self.execute_topic_agents_md_rollback(arguments).await,
            TOOL_PRIVATE_SECRET_PROBE => self.execute_private_secret_probe(arguments).await,
            TOOL_TOPIC_INFRA_UPSERT => self.execute_topic_infra_upsert(arguments).await,
            TOOL_TOPIC_INFRA_GET => self.execute_topic_infra_get(arguments).await,
            TOOL_TOPIC_INFRA_DELETE => self.execute_topic_infra_delete(arguments).await,
            TOOL_TOPIC_INFRA_ROLLBACK => self.execute_topic_infra_rollback(arguments).await,
            TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT => {
                self.execute_forum_topic_provision_ssh_agent(arguments)
                    .await
            }
            TOOL_AGENT_PROFILE_UPSERT => self.execute_agent_profile_upsert(arguments).await,
            TOOL_AGENT_PROFILE_GET => self.execute_agent_profile_get(arguments).await,
            TOOL_AGENT_PROFILE_DELETE => self.execute_agent_profile_delete(arguments).await,
            TOOL_AGENT_PROFILE_ROLLBACK => self.execute_agent_profile_rollback(arguments).await,
            TOOL_TOPIC_AGENT_TOOLS_GET => self.execute_topic_agent_tools_get(arguments).await,
            TOOL_TOPIC_AGENT_TOOLS_ENABLE => self.execute_topic_agent_tools_enable(arguments).await,
            TOOL_TOPIC_AGENT_TOOLS_DISABLE => {
                self.execute_topic_agent_tools_disable(arguments).await
            }
            TOOL_TOPIC_AGENT_HOOKS_GET => self.execute_topic_agent_hooks_get(arguments).await,
            TOOL_TOPIC_AGENT_HOOKS_ENABLE => self.execute_topic_agent_hooks_enable(arguments).await,
            TOOL_TOPIC_AGENT_HOOKS_DISABLE => {
                self.execute_topic_agent_hooks_disable(arguments).await
            }
            TOOL_TOPIC_SANDBOX_LIST => self.execute_topic_sandbox_list(arguments).await,
            TOOL_TOPIC_SANDBOX_GET => self.execute_topic_sandbox_get(arguments).await,
            TOOL_TOPIC_SANDBOX_CREATE => self.execute_topic_sandbox_create(arguments).await,
            TOOL_TOPIC_SANDBOX_RECREATE => self.execute_topic_sandbox_recreate(arguments).await,
            TOOL_TOPIC_SANDBOX_DELETE => self.execute_topic_sandbox_delete(arguments).await,
            TOOL_TOPIC_SANDBOX_PRUNE => self.execute_topic_sandbox_prune(arguments).await,
            TOOL_FORUM_TOPIC_CREATE => self.execute_forum_topic_create(arguments).await,
            TOOL_FORUM_TOPIC_EDIT => self.execute_forum_topic_edit(arguments).await,
            TOOL_FORUM_TOPIC_CLOSE => {
                self.execute_forum_topic_thread_action(arguments, TOOL_FORUM_TOPIC_CLOSE)
                    .await
            }
            TOOL_FORUM_TOPIC_REOPEN => {
                self.execute_forum_topic_thread_action(arguments, TOOL_FORUM_TOPIC_REOPEN)
                    .await
            }
            TOOL_FORUM_TOPIC_DELETE => {
                self.execute_forum_topic_thread_action(arguments, TOOL_FORUM_TOPIC_DELETE)
                    .await
            }
            TOOL_FORUM_TOPIC_LIST => self.execute_forum_topic_list(arguments).await,
            _ => Err(anyhow!("Unknown manager control-plane tool: {tool_name}")),
        }
    }
}

#[cfg(test)]
mod tests;
