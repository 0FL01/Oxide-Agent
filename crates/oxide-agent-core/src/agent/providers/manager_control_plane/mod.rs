//! Manager control-plane provider.
//!
//! Exposes user-scoped CRUD tools for topic bindings, topic contexts, and agent profiles.

use super::ssh_mcp::{inspect_topic_infra_config, probe_secret_ref, SecretProbeKind};
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

mod shared;

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

enum AuditStatus {
    Written,
    WriteFailed(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingSetArgs {
    topic_id: String,
    agent_id: String,
    #[serde(default)]
    binding_kind: Option<TopicBindingKind>,
    #[serde(default)]
    chat_id: OptionalMetadataPatch<i64>,
    #[serde(default)]
    thread_id: OptionalMetadataPatch<i64>,
    #[serde(default)]
    expires_at: OptionalMetadataPatch<i64>,
    #[serde(default)]
    last_activity_at: Option<i64>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicBindingRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextUpsertArgs {
    topic_id: String,
    context: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicContextRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdUpsertArgs {
    topic_id: String,
    agents_md: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentsMdRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraUpsertArgs {
    topic_id: String,
    target_name: String,
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
    remote_user: String,
    #[serde(default)]
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
    #[serde(default)]
    approval_required_modes: Vec<TopicInfraToolMode>,
    #[serde(default)]
    dry_run: bool,
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
struct TopicInfraGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraDeleteArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicInfraRollbackArgs {
    topic_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateSecretProbeArgs {
    secret_ref: String,
    #[serde(default = "default_secret_probe_kind")]
    kind: SecretProbeKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileUpsertArgs {
    agent_id: String,
    profile: serde_json::Value,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileGetArgs {
    agent_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileDeleteArgs {
    agent_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentProfileRollbackArgs {
    agent_id: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentToolsGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentToolsMutationArgs {
    topic_id: String,
    tools: Vec<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentToolGroup {
    provider: &'static str,
    aliases: &'static [&'static str],
    tools: &'static [&'static str],
}

#[derive(Clone, Debug)]
struct TopicAgentToolCatalog {
    groups: Vec<TopicAgentToolGroup>,
    tool_names: BTreeSet<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentToolGroupStatus {
    provider: String,
    available_tools: Vec<String>,
    active_tools: Vec<String>,
    blocked_tools: Vec<String>,
    enabled: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentToolSnapshot {
    policy_mode: String,
    available_tools: Vec<String>,
    active_tools: Vec<String>,
    blocked_tools: Vec<String>,
    allowed_tools_raw: Option<Vec<String>>,
    unknown_profile_tools: Vec<String>,
    provider_statuses: Vec<TopicAgentToolGroupStatus>,
}

#[derive(Debug)]
struct TopicAgentToolMutation {
    profile: serde_json::Value,
    changed: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentToolMutationContext {
    topic_id: String,
    agent_id: String,
    requested_tools: Vec<String>,
    previous: Option<AgentProfileRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentHooksGetArgs {
    topic_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicAgentHooksMutationArgs {
    topic_id: String,
    hooks: Vec<String>,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentHookCatalog {
    manageable_hooks: BTreeSet<String>,
    protected_hooks: BTreeSet<String>,
    all_hooks: BTreeSet<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentHookStatus {
    hook: String,
    active: bool,
    manageable: bool,
    protected: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TopicAgentHookSnapshot {
    policy_mode: String,
    available_hooks: Vec<String>,
    active_hooks: Vec<String>,
    disabled_hooks: Vec<String>,
    enabled_hooks_raw: Option<Vec<String>>,
    unknown_profile_hooks: Vec<String>,
    hook_statuses: Vec<TopicAgentHookStatus>,
}

#[derive(Debug)]
struct TopicAgentHookMutation {
    profile: serde_json::Value,
    changed: bool,
}

#[derive(Clone, Debug)]
struct TopicAgentHookMutationContext {
    topic_id: String,
    agent_id: String,
    requested_hooks: Vec<String>,
    previous: Option<AgentProfileRecord>,
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

    fn topic_binding_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_SET.to_string(),
                description: "Low-level binding mutation. For newly created Telegram forum topics prefer forum_topic_provision_ssh_agent or pass the canonical topic_id '<chat_id>:<thread_id>'"
                    .to_string(),
                parameters: Self::topic_binding_set_parameters(),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_GET.to_string(),
                description: "Get topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                description: "Rollback last topic binding mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_BINDING_DELETE.to_string(),
                description: "Delete topic-to-agent binding for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
        ]
    }

    fn topic_context_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                description: format!(
                    "Create or update short topic operational context for current user (max {TOPIC_CONTEXT_MAX_LINES} lines / {TOPIC_CONTEXT_MAX_CHARS} chars; not for AGENTS.md)"
                ),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "context": { "type": "string", "description": "Short operational context injected into the agent prompt; use topic_agents_md_upsert for AGENTS.md documents" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "context"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_GET.to_string(),
                description: "Get topic-specific execution context for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
                description: "Delete topic-specific execution context for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
                description: "Rollback last topic context mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
        ]
    }

    fn topic_agents_md_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                description:
                    "Create or update topic-scoped AGENTS.md for new flows (max 300 lines)"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "agents_md": { "type": "string", "description": "Full AGENTS.md content injected once when a new flow starts" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "agents_md"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_GET.to_string(),
                description: "Get topic-scoped AGENTS.md for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
                description: "Delete topic-scoped AGENTS.md for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
                description: "Rollback last topic-scoped AGENTS.md mutation for current user"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
        ]
    }

    fn topic_infra_tools_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                description: "Low-level infra mutation. For newly created Telegram forum topics prefer forum_topic_provision_ssh_agent or pass the canonical topic_id '<chat_id>:<thread_id>'"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "target_name": { "type": "string", "description": "Human-readable target name" },
                        "host": { "type": "string", "description": "SSH host or DNS name" },
                        "port": { "type": "integer", "description": "SSH port, defaults to 22" },
                        "remote_user": { "type": "string", "description": "Remote SSH username" },
                        "auth_mode": { "type": "string", "enum": ["none", "password", "private_key"], "description": "SSH authentication mode" },
                        "secret_ref": { "type": "string", "description": "Opaque secret reference for SSH auth material" },
                        "sudo_secret_ref": { "type": "string", "description": "Opaque secret reference for sudo password material" },
                        "environment": { "type": "string", "description": "Optional environment label such as prod or stage" },
                        "tags": { "type": "array", "items": { "type": "string" }, "description": "Optional free-form target tags" },
                        "allowed_tool_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Allowlisted SSH tool modes" },
                        "approval_required_modes": { "type": "array", "items": { "type": "string", "enum": ["exec", "sudo_exec", "read_file", "apply_file_edit", "check_process"] }, "description": "Modes that always require operator approval" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "target_name", "host", "remote_user"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_GET.to_string(),
                description: "Get topic-scoped infra target config for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_DELETE.to_string(),
                description: "Delete topic-scoped infra target config for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                description: "Rollback last topic infra config mutation for current user"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["topic_id"]
                }),
            },
        ]
    }

    fn agent_profile_tools_definitions() -> Vec<ToolDefinition> {
        let mut tools = vec![
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                description: "Create or update agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "profile": { "type": "object", "description": "Arbitrary JSON profile payload" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["agent_id", "profile"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_GET.to_string(),
                description: "Get agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" }
                    },
                    "required": ["agent_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_DELETE.to_string(),
                description: "Delete agent profile for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["agent_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                description: "Rollback last agent profile mutation for current user".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "Stable agent identifier" },
                        "dry_run": { "type": "boolean", "description": "Preview rollback without persisting" }
                    },
                    "required": ["agent_id"]
                }),
            },
        ];
        tools.extend(Self::topic_agent_tools_management_definitions());
        tools.extend(Self::topic_agent_hooks_management_definitions());
        tools
    }

    fn topic_agent_tools_management_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_GET.to_string(),
                description: "Inspect the effective tool set for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_ENABLE.to_string(),
                description:
                    "Enable one or more tools or provider groups for the agent bound to a topic"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "tools": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tool names or provider aliases like ytdlp, ssh, sandbox, search, reminder"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "tools"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_TOOLS_DISABLE.to_string(),
                description:
                    "Disable one or more tools or provider groups for the agent bound to a topic"
                        .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "tools": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Tool names or provider aliases like ytdlp, ssh, sandbox, search, reminder"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "tools"]
                }),
            },
        ]
    }

    fn topic_agent_hooks_management_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_GET.to_string(),
                description: "Inspect the effective hook set for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" }
                    },
                    "required": ["topic_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_ENABLE.to_string(),
                description: "Enable one or more manageable hooks for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "hooks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Hook names such as workload_distributor, delegation_guard, search_budget, timeout_report"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "hooks"]
                }),
            },
            ToolDefinition {
                name: TOOL_TOPIC_AGENT_HOOKS_DISABLE.to_string(),
                description: "Disable one or more manageable hooks for the agent bound to a topic"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier or unique forum topic alias" },
                        "hooks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Hook names such as workload_distributor, delegation_guard, search_budget, timeout_report"
                        },
                        "dry_run": { "type": "boolean", "description": "Validate and preview without persisting" }
                    },
                    "required": ["topic_id", "hooks"]
                }),
            },
        ]
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

    fn configured_search_tool_group() -> Option<TopicAgentToolGroup> {
        match crate::config::get_search_provider().as_str() {
            "tavily" => {
                #[cfg(feature = "tavily")]
                {
                    std::env::var("TAVILY_API_KEY")
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .map(|_| TopicAgentToolGroup {
                            provider: "search",
                            aliases: &["search", "tavily"],
                            tools: TOPIC_AGENT_TAVILY_TOOLS,
                        })
                }
                #[cfg(not(feature = "tavily"))]
                {
                    None
                }
            }
            "crawl4ai" => {
                #[cfg(feature = "crawl4ai")]
                {
                    std::env::var("CRAWL4AI_URL")
                        .ok()
                        .filter(|value| !value.trim().is_empty())
                        .map(|_| TopicAgentToolGroup {
                            provider: "search",
                            aliases: &["search", "crawl4ai"],
                            tools: TOPIC_AGENT_CRAWL4AI_TOOLS,
                        })
                }
                #[cfg(not(feature = "crawl4ai"))]
                {
                    None
                }
            }
            _ => None,
        }
    }

    async fn topic_agent_tool_catalog(&self, topic_id: &str) -> Result<TopicAgentToolCatalog> {
        let mut groups = vec![
            TopicAgentToolGroup {
                provider: "todos",
                aliases: &["todos"],
                tools: TOPIC_AGENT_TODOS_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "sandbox",
                aliases: &["sandbox"],
                tools: TOPIC_AGENT_SANDBOX_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "filehoster",
                aliases: &["filehoster", "files"],
                tools: TOPIC_AGENT_FILEHOSTER_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "ytdlp",
                aliases: &["ytdlp", "youtube"],
                tools: TOPIC_AGENT_YTDLP_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "delegation",
                aliases: &["delegation", "delegate"],
                tools: TOPIC_AGENT_DELEGATION_TOOLS,
            },
            TopicAgentToolGroup {
                provider: "reminder",
                aliases: &["reminder", "wakeups", "wakeup"],
                tools: TOPIC_AGENT_REMINDER_TOOLS,
            },
        ];

        if let Some(search_group) = Self::configured_search_tool_group() {
            groups.push(search_group);
        }

        let topic_infra = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.to_string())
            .await
            .map_err(|err| anyhow!("failed to get topic infra config: {err}"))?;
        if topic_infra.is_some() {
            groups.push(TopicAgentToolGroup {
                provider: "ssh",
                aliases: &["ssh"],
                tools: TOPIC_AGENT_SSH_TOOLS,
            });
        }

        let mut tool_names = BTreeSet::new();
        for group in &groups {
            for tool in group.tools {
                tool_names.insert((*tool).to_string());
            }
        }

        Ok(TopicAgentToolCatalog { groups, tool_names })
    }

    fn parse_profile_tool_set(
        profile: &serde_json::Value,
        camel_key: &str,
        snake_key: &str,
    ) -> Option<BTreeSet<String>> {
        let array = profile
            .get(camel_key)
            .and_then(serde_json::Value::as_array)
            .or_else(|| profile.get(snake_key).and_then(serde_json::Value::as_array))?;

        Some(
            array
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    fn write_profile_tool_set(
        profile: &mut serde_json::Value,
        camel_key: &str,
        snake_key: &str,
        values: Option<&BTreeSet<String>>,
        remove_when_empty: bool,
    ) -> Result<()> {
        let object = profile
            .as_object_mut()
            .ok_or_else(|| anyhow!("profile must be a JSON object"))?;
        object.remove(snake_key);

        match values {
            Some(values) if !(remove_when_empty && values.is_empty()) => {
                object.insert(
                    camel_key.to_string(),
                    serde_json::Value::Array(
                        values
                            .iter()
                            .cloned()
                            .map(serde_json::Value::String)
                            .collect(),
                    ),
                );
            }
            _ => {
                object.remove(camel_key);
            }
        }

        Ok(())
    }

    fn profile_tool_snapshot(
        profile: Option<&serde_json::Value>,
    ) -> (Option<Vec<String>>, Vec<String>, ToolAccessPolicy) {
        let Some(profile) = profile else {
            return (None, Vec::new(), ToolAccessPolicy::default());
        };

        let allowed = Self::parse_profile_tool_set(profile, "allowedTools", "allowed_tools")
            .map(|set| set.into_iter().collect::<Vec<_>>());
        let blocked = Self::parse_profile_tool_set(profile, "blockedTools", "blocked_tools")
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let parsed = parse_agent_profile(profile);
        let policy = parsed
            .tool_policy
            .with_additional_allowed_tools(TOPIC_AGENT_REMINDER_TOOLS.iter().copied());

        (allowed, blocked, policy)
    }

    fn topic_agent_tool_snapshot(
        catalog: &TopicAgentToolCatalog,
        profile: Option<&serde_json::Value>,
    ) -> TopicAgentToolSnapshot {
        let (allowed_tools_raw, blocked_tools, policy) = Self::profile_tool_snapshot(profile);
        let available_tools = catalog.tool_names.iter().cloned().collect::<Vec<_>>();
        let active_tools = available_tools
            .iter()
            .filter(|tool| policy.allows(tool))
            .cloned()
            .collect::<Vec<_>>();
        let known_tools = &catalog.tool_names;
        let unknown_profile_tools = allowed_tools_raw
            .iter()
            .flatten()
            .chain(blocked_tools.iter())
            .filter(|tool| !known_tools.contains(*tool))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let blocked_lookup = blocked_tools.iter().cloned().collect::<HashSet<_>>();
        let provider_statuses = catalog
            .groups
            .iter()
            .map(|group| {
                let available_tools = group
                    .tools
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect::<Vec<_>>();
                let active_tools = available_tools
                    .iter()
                    .filter(|tool| policy.allows(tool))
                    .cloned()
                    .collect::<Vec<_>>();
                let blocked_tools = available_tools
                    .iter()
                    .filter(|tool| blocked_lookup.contains(*tool))
                    .cloned()
                    .collect::<Vec<_>>();

                TopicAgentToolGroupStatus {
                    provider: group.provider.to_string(),
                    enabled: !active_tools.is_empty(),
                    available_tools,
                    active_tools,
                    blocked_tools,
                }
            })
            .collect::<Vec<_>>();

        TopicAgentToolSnapshot {
            policy_mode: if allowed_tools_raw.is_some() {
                "allowlist".to_string()
            } else {
                "all_except_blocked".to_string()
            },
            available_tools,
            active_tools,
            blocked_tools,
            allowed_tools_raw,
            unknown_profile_tools,
            provider_statuses,
        }
    }

    fn expand_topic_agent_tools(
        catalog: &TopicAgentToolCatalog,
        requested_tools: Vec<String>,
    ) -> Result<Vec<String>> {
        let mut requested = BTreeSet::new();
        for raw in requested_tools {
            let token = raw.trim().to_ascii_lowercase();
            if token.is_empty() {
                continue;
            }

            if catalog.tool_names.contains(&token) {
                requested.insert(token);
                continue;
            }

            let Some(group) = catalog
                .groups
                .iter()
                .find(|group| group.provider == token || group.aliases.contains(&token.as_str()))
            else {
                bail!("unknown tool or provider alias '{token}' for the topic agent");
            };

            for tool in group.tools {
                requested.insert((*tool).to_string());
            }
        }

        if requested.is_empty() {
            bail!("tools must contain at least one non-empty tool name or provider alias");
        }

        Ok(requested.into_iter().collect())
    }

    fn enable_topic_agent_tools(
        profile: Option<&AgentProfileRecord>,
        tools: &[String],
    ) -> Result<TopicAgentToolMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut allowed =
            Self::parse_profile_tool_set(&next_profile, "allowedTools", "allowed_tools");
        let mut blocked =
            Self::parse_profile_tool_set(&next_profile, "blockedTools", "blocked_tools")
                .unwrap_or_default();

        for tool in tools {
            blocked.remove(tool);
            if let Some(allowed) = allowed.as_mut() {
                allowed.insert(tool.clone());
            }
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "allowedTools",
            "allowed_tools",
            allowed.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "blockedTools",
            "blocked_tools",
            Some(&blocked),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentToolMutation {
            changed,
            profile: next_profile,
        })
    }

    fn disable_topic_agent_tools(
        profile: Option<&AgentProfileRecord>,
        tools: &[String],
    ) -> Result<TopicAgentToolMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut allowed =
            Self::parse_profile_tool_set(&next_profile, "allowedTools", "allowed_tools");
        let mut blocked =
            Self::parse_profile_tool_set(&next_profile, "blockedTools", "blocked_tools")
                .unwrap_or_default();

        for tool in tools {
            if let Some(allowed) = allowed.as_mut() {
                allowed.remove(tool);
            }
            blocked.insert(tool.clone());
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "allowedTools",
            "allowed_tools",
            allowed.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "blockedTools",
            "blocked_tools",
            Some(&blocked),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentToolMutation {
            changed,
            profile: next_profile,
        })
    }

    fn topic_agent_tools_operation_name(action: &str) -> Result<&'static str> {
        match action {
            TOOL_TOPIC_AGENT_TOOLS_ENABLE => Ok("enable"),
            TOOL_TOPIC_AGENT_TOOLS_DISABLE => Ok("disable"),
            _ => bail!("unsupported topic agent tools action: {action}"),
        }
    }

    async fn prepare_topic_agent_tool_mutation(
        &self,
        raw_topic_id: String,
        requested_tools: Vec<String>,
    ) -> Result<(TopicAgentToolMutationContext, TopicAgentToolCatalog)> {
        let topic_id = self.resolve_mutation_topic_id(raw_topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?
            .ok_or_else(|| anyhow!("topic_id '{topic_id}' is not bound to an agent"))?;
        let agent_id = binding.agent_id;
        let catalog = self.topic_agent_tool_catalog(&topic_id).await?;
        let requested_tools = Self::expand_topic_agent_tools(&catalog, requested_tools)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        Ok((
            TopicAgentToolMutationContext {
                topic_id,
                agent_id,
                requested_tools,
                previous,
            },
            catalog,
        ))
    }

    async fn append_topic_agent_tools_audit(
        &self,
        action: &str,
        context: &TopicAgentToolMutationContext,
        changed: bool,
        outcome: &str,
        version: Option<u64>,
        sandbox_cleanup: Option<serde_json::Value>,
    ) -> AuditStatus {
        self.append_audit_with_status(AppendAuditEventOptions {
            user_id: self.user_id,
            topic_id: Some(context.topic_id.clone()),
            agent_id: Some(context.agent_id.clone()),
            action: action.to_string(),
            payload: json!({
                "topic_id": context.topic_id.clone(),
                "agent_id": context.agent_id.clone(),
                "requested": context.requested_tools.clone(),
                "previous": context.previous.clone(),
                "changed": changed,
                "version": version,
                "sandbox_cleanup": sandbox_cleanup,
                "outcome": outcome
            }),
        })
        .await
    }

    fn topic_agent_tools_preview_response(
        operation: &str,
        context: TopicAgentToolMutationContext,
        changed: bool,
        profile: serde_json::Value,
        snapshot: TopicAgentToolSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": operation,
                    "topic_id": context.topic_id,
                    "agent_id": context.agent_id,
                    "requested_tools": context.requested_tools,
                    "changed": changed,
                    "profile": profile,
                    "tools": snapshot
                },
                "previous": context.previous
            }),
            audit_status,
        ))
    }

    fn topic_agent_tools_result_response(
        updated: bool,
        context: TopicAgentToolMutationContext,
        profile: Option<AgentProfileRecord>,
        snapshot: TopicAgentToolSnapshot,
        sandbox_cleanup: Option<serde_json::Value>,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "updated": updated,
                "topic_id": context.topic_id,
                "agent_id": context.agent_id,
                "requested_tools": context.requested_tools,
                "profile": profile,
                "tools": snapshot,
                "sandbox_cleanup": sandbox_cleanup
            }),
            audit_status,
        ))
    }

    fn topic_agent_hook_catalog() -> TopicAgentHookCatalog {
        let manageable_hooks = topic_agent_manageable_hooks()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let protected_hooks = topic_agent_protected_hooks()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let all_hooks = topic_agent_all_hooks().into_iter().collect::<BTreeSet<_>>();

        TopicAgentHookCatalog {
            manageable_hooks,
            protected_hooks,
            all_hooks,
        }
    }

    fn normalize_topic_agent_hook_name(token: &str) -> Option<&'static str> {
        match token {
            "workload" => Some("workload_distributor"),
            "delegation" => Some("delegation_guard"),
            "search" => Some("search_budget"),
            "timeout" => Some("timeout_report"),
            _ => None,
        }
    }

    fn profile_hook_snapshot(
        profile: Option<&serde_json::Value>,
    ) -> (Option<Vec<String>>, Vec<String>, HookAccessPolicy) {
        let Some(profile) = profile else {
            return (None, Vec::new(), HookAccessPolicy::default());
        };

        let enabled = Self::parse_profile_tool_set(profile, "enabledHooks", "enabled_hooks")
            .map(|set| set.into_iter().collect::<Vec<_>>());
        let disabled = Self::parse_profile_tool_set(profile, "disabledHooks", "disabled_hooks")
            .unwrap_or_default()
            .into_iter()
            .collect::<Vec<_>>();
        let parsed = parse_agent_profile(profile);

        (enabled, disabled, parsed.hook_policy)
    }

    fn topic_agent_hook_snapshot(
        catalog: &TopicAgentHookCatalog,
        profile: Option<&serde_json::Value>,
    ) -> TopicAgentHookSnapshot {
        let (enabled_hooks_raw, disabled_hooks_raw, policy) = Self::profile_hook_snapshot(profile);
        let available_hooks = catalog.all_hooks.iter().cloned().collect::<Vec<_>>();
        let active_hooks = available_hooks
            .iter()
            .filter(|hook| {
                catalog.protected_hooks.contains(*hook)
                    || (catalog.manageable_hooks.contains(*hook) && policy.allows(hook))
            })
            .cloned()
            .collect::<Vec<_>>();
        let disabled_hooks = catalog
            .manageable_hooks
            .iter()
            .filter(|hook| !policy.allows(hook))
            .cloned()
            .collect::<Vec<_>>();
        let unknown_profile_hooks = enabled_hooks_raw
            .iter()
            .flatten()
            .chain(disabled_hooks_raw.iter())
            .filter(|hook| !catalog.all_hooks.contains(*hook))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let hook_statuses = available_hooks
            .iter()
            .map(|hook| {
                let protected = catalog.protected_hooks.contains(hook);
                let manageable = catalog.manageable_hooks.contains(hook);
                let active = protected || (manageable && policy.allows(hook));
                TopicAgentHookStatus {
                    hook: hook.clone(),
                    active,
                    manageable,
                    protected,
                }
            })
            .collect::<Vec<_>>();

        TopicAgentHookSnapshot {
            policy_mode: if enabled_hooks_raw.is_some() {
                "allowlist".to_string()
            } else {
                "all_except_disabled".to_string()
            },
            available_hooks,
            active_hooks,
            disabled_hooks,
            enabled_hooks_raw,
            unknown_profile_hooks,
            hook_statuses,
        }
    }

    fn expand_topic_agent_hooks(
        catalog: &TopicAgentHookCatalog,
        requested_hooks: Vec<String>,
    ) -> Result<Vec<String>> {
        let mut requested = BTreeSet::new();
        for raw in requested_hooks {
            let mut token = raw.trim().to_ascii_lowercase();
            if token.is_empty() {
                continue;
            }
            if let Some(alias) = Self::normalize_topic_agent_hook_name(&token) {
                token = alias.to_string();
            }

            if catalog.protected_hooks.contains(&token) {
                bail!("hook '{token}' is system-protected and cannot be toggled");
            }
            if !catalog.manageable_hooks.contains(&token) {
                bail!("unknown manageable hook '{token}' for the topic agent");
            }

            requested.insert(token);
        }

        if requested.is_empty() {
            bail!("hooks must contain at least one non-empty hook name");
        }

        Ok(requested.into_iter().collect())
    }

    fn enable_topic_agent_hooks(
        profile: Option<&AgentProfileRecord>,
        hooks: &[String],
    ) -> Result<TopicAgentHookMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut enabled =
            Self::parse_profile_tool_set(&next_profile, "enabledHooks", "enabled_hooks");
        let mut disabled =
            Self::parse_profile_tool_set(&next_profile, "disabledHooks", "disabled_hooks")
                .unwrap_or_default();

        for hook in hooks {
            disabled.remove(hook);
            if let Some(enabled) = enabled.as_mut() {
                enabled.insert(hook.clone());
            }
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "enabledHooks",
            "enabled_hooks",
            enabled.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "disabledHooks",
            "disabled_hooks",
            Some(&disabled),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentHookMutation {
            profile: next_profile,
            changed,
        })
    }

    fn disable_topic_agent_hooks(
        profile: Option<&AgentProfileRecord>,
        hooks: &[String],
    ) -> Result<TopicAgentHookMutation> {
        let mut next_profile = match profile {
            Some(profile) => Self::validate_profile_object(profile.profile.clone())?,
            None => json!({}),
        };
        let mut enabled =
            Self::parse_profile_tool_set(&next_profile, "enabledHooks", "enabled_hooks");
        let mut disabled =
            Self::parse_profile_tool_set(&next_profile, "disabledHooks", "disabled_hooks")
                .unwrap_or_default();

        for hook in hooks {
            if let Some(enabled) = enabled.as_mut() {
                enabled.remove(hook);
            }
            disabled.insert(hook.clone());
        }

        Self::write_profile_tool_set(
            &mut next_profile,
            "enabledHooks",
            "enabled_hooks",
            enabled.as_ref(),
            false,
        )?;
        Self::write_profile_tool_set(
            &mut next_profile,
            "disabledHooks",
            "disabled_hooks",
            Some(&disabled),
            true,
        )?;

        let changed = match profile {
            Some(profile) => profile.profile != next_profile,
            None => next_profile != json!({}),
        };
        Ok(TopicAgentHookMutation {
            profile: next_profile,
            changed,
        })
    }

    fn topic_agent_hooks_operation_name(action: &str) -> Result<&'static str> {
        match action {
            TOOL_TOPIC_AGENT_HOOKS_ENABLE => Ok("enable"),
            TOOL_TOPIC_AGENT_HOOKS_DISABLE => Ok("disable"),
            _ => bail!("unsupported topic agent hooks action: {action}"),
        }
    }

    async fn prepare_topic_agent_hook_mutation(
        &self,
        raw_topic_id: String,
        requested_hooks: Vec<String>,
    ) -> Result<(TopicAgentHookMutationContext, TopicAgentHookCatalog)> {
        let topic_id = self.resolve_mutation_topic_id(raw_topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?
            .ok_or_else(|| anyhow!("topic_id '{topic_id}' is not bound to an agent"))?;
        let agent_id = binding.agent_id;
        let catalog = Self::topic_agent_hook_catalog();
        let requested_hooks = Self::expand_topic_agent_hooks(&catalog, requested_hooks)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        Ok((
            TopicAgentHookMutationContext {
                topic_id,
                agent_id,
                requested_hooks,
                previous,
            },
            catalog,
        ))
    }

    async fn append_topic_agent_hooks_audit(
        &self,
        action: &str,
        context: &TopicAgentHookMutationContext,
        changed: bool,
        outcome: &str,
        version: Option<u64>,
    ) -> AuditStatus {
        self.append_audit_with_status(AppendAuditEventOptions {
            user_id: self.user_id,
            topic_id: Some(context.topic_id.clone()),
            agent_id: Some(context.agent_id.clone()),
            action: action.to_string(),
            payload: json!({
                "topic_id": context.topic_id.clone(),
                "agent_id": context.agent_id.clone(),
                "requested": context.requested_hooks.clone(),
                "previous": context.previous.clone(),
                "changed": changed,
                "version": version,
                "outcome": outcome
            }),
        })
        .await
    }

    fn topic_agent_hooks_preview_response(
        operation: &str,
        context: TopicAgentHookMutationContext,
        changed: bool,
        profile: serde_json::Value,
        snapshot: TopicAgentHookSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "dry_run": true,
                "preview": {
                    "operation": operation,
                    "topic_id": context.topic_id,
                    "agent_id": context.agent_id,
                    "requested_hooks": context.requested_hooks,
                    "changed": changed,
                    "profile": profile,
                    "hooks": snapshot
                },
                "previous": context.previous
            }),
            audit_status,
        ))
    }

    fn topic_agent_hooks_result_response(
        updated: bool,
        context: TopicAgentHookMutationContext,
        profile: Option<AgentProfileRecord>,
        snapshot: TopicAgentHookSnapshot,
        audit_status: AuditStatus,
    ) -> Result<String> {
        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "updated": updated,
                "topic_id": context.topic_id,
                "agent_id": context.agent_id,
                "requested_hooks": context.requested_hooks,
                "profile": profile,
                "hooks": snapshot
            }),
            audit_status,
        ))
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

    fn sandbox_provider_enabled(snapshot: &TopicAgentToolSnapshot) -> bool {
        snapshot
            .provider_statuses
            .iter()
            .find(|status| status.provider == "sandbox")
            .is_some_and(|status| status.enabled)
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

    fn topic_infra_preview_record_from_plan(
        &self,
        topic_id: String,
        plan: &ForumTopicProvisionSshAgentPlan,
    ) -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version: 0,
            user_id: self.user_id,
            topic_id,
            target_name: plan.target_name.clone(),
            host: plan.host.clone(),
            port: plan.port,
            remote_user: plan.remote_user.clone(),
            auth_mode: plan.auth_mode,
            secret_ref: plan.secret_ref.clone(),
            sudo_secret_ref: plan.sudo_secret_ref.clone(),
            environment: plan.environment.clone(),
            tags: plan.tags.clone(),
            allowed_tool_modes: plan.allowed_tool_modes.clone(),
            approval_required_modes: plan.approval_required_modes.clone(),
            created_at: 0,
            updated_at: 0,
        }
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

    fn previous_from_payload<T: DeserializeOwned>(
        payload: &serde_json::Value,
    ) -> Result<Option<T>> {
        let Some(previous) = payload.get("previous") else {
            return Ok(None);
        };

        if previous.is_null() {
            return Ok(None);
        }

        serde_json::from_value(previous.clone())
            .map(Some)
            .map_err(|err| anyhow!("invalid previous snapshot in audit payload: {err}"))
    }

    fn is_applied_mutation_event(event: &crate::storage::AuditEventRecord) -> bool {
        !matches!(
            event
                .payload
                .get("outcome")
                .and_then(serde_json::Value::as_str),
            Some("dry_run" | "noop")
        )
    }

    fn action_matches(action: &str, candidates: &[&str]) -> bool {
        candidates.contains(&action)
    }

    async fn append_audit_with_status(&self, options: AppendAuditEventOptions) -> AuditStatus {
        match self.storage.append_audit_event(options).await {
            Ok(_) => AuditStatus::Written,
            Err(err) => AuditStatus::WriteFailed(err.to_string()),
        }
    }

    fn attach_audit_status(
        mut response: serde_json::Value,
        status: AuditStatus,
    ) -> serde_json::Value {
        if let Some(response_object) = response.as_object_mut() {
            match status {
                AuditStatus::Written => {
                    response_object.insert("audit_status".to_string(), json!("written"));
                }
                AuditStatus::WriteFailed(error) => {
                    response_object.insert("audit_status".to_string(), json!("write_failed"));
                    response_object.insert("audit_error".to_string(), json!(error));
                }
            }
        }

        response
    }

    async fn find_latest_applied_mutation<F>(
        &self,
        mut predicate: F,
    ) -> Result<Option<crate::storage::AuditEventRecord>>
    where
        F: FnMut(&crate::storage::AuditEventRecord) -> bool,
    {
        let mut cursor = None;

        loop {
            let events = self
                .storage
                .list_audit_events_page(self.user_id, cursor, ROLLBACK_AUDIT_PAGE_SIZE)
                .await
                .map_err(|err| anyhow!("failed to list audit events: {err}"))?;

            if events.is_empty() {
                return Ok(None);
            }

            if let Some(event) = events
                .iter()
                .find(|event| Self::is_applied_mutation_event(event) && predicate(event))
            {
                return Ok(Some(event.clone()));
            }

            cursor = events.last().map(|event| event.version);
            if cursor.is_none() {
                return Ok(None);
            }
        }
    }

    async fn last_topic_binding_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_BINDING_SET,
                        TOOL_TOPIC_BINDING_DELETE,
                        TOOL_TOPIC_BINDING_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn last_agent_profile_mutation(
        &self,
        agent_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.agent_id.as_deref() == Some(agent_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_AGENT_PROFILE_UPSERT,
                        TOOL_AGENT_PROFILE_DELETE,
                        TOOL_TOPIC_AGENT_TOOLS_ENABLE,
                        TOOL_TOPIC_AGENT_TOOLS_DISABLE,
                        TOOL_TOPIC_AGENT_HOOKS_ENABLE,
                        TOOL_TOPIC_AGENT_HOOKS_DISABLE,
                        TOOL_AGENT_PROFILE_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn last_topic_context_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_CONTEXT_UPSERT,
                        TOOL_TOPIC_CONTEXT_DELETE,
                        TOOL_TOPIC_CONTEXT_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn last_topic_agents_md_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_AGENTS_MD_UPSERT,
                        TOOL_TOPIC_AGENTS_MD_DELETE,
                        TOOL_TOPIC_AGENTS_MD_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn last_topic_infra_mutation(
        &self,
        topic_id: &str,
    ) -> Result<Option<crate::storage::AuditEventRecord>> {
        self.find_latest_applied_mutation(|event| {
            event.topic_id.as_deref() == Some(topic_id)
                && Self::action_matches(
                    event.action.as_str(),
                    &[
                        TOOL_TOPIC_INFRA_UPSERT,
                        TOOL_TOPIC_INFRA_DELETE,
                        TOOL_TOPIC_INFRA_ROLLBACK,
                    ],
                )
        })
        .await
    }

    async fn restore_or_delete_topic_infra(
        &self,
        topic_id: &str,
        previous: Option<TopicInfraConfigRecord>,
    ) -> Result<Option<TopicInfraConfigRecord>> {
        if let Some(previous_infra) = previous {
            return self
                .storage
                .upsert_topic_infra_config(UpsertTopicInfraConfigOptions {
                    user_id: self.user_id,
                    topic_id: topic_id.to_string(),
                    target_name: previous_infra.target_name,
                    host: previous_infra.host,
                    port: previous_infra.port,
                    remote_user: previous_infra.remote_user,
                    auth_mode: previous_infra.auth_mode,
                    secret_ref: previous_infra.secret_ref,
                    sudo_secret_ref: previous_infra.sudo_secret_ref,
                    environment: previous_infra.environment,
                    tags: previous_infra.tags,
                    allowed_tool_modes: previous_infra.allowed_tool_modes,
                    approval_required_modes: previous_infra.approval_required_modes,
                })
                .await
                .map(Some)
                .map_err(|err| anyhow!("failed to restore topic infra config: {err}"));
        }

        self.storage
            .delete_topic_infra_config(self.user_id, topic_id.to_string())
            .await
            .map_err(|err| anyhow!("failed to delete topic infra config during rollback: {err}"))?;
        Ok(None)
    }

    async fn execute_topic_binding_set(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingSetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_SET)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let binding_kind = args.binding_kind;
        let chat_id = args.chat_id;
        let thread_id = args.thread_id;
        let expires_at = args.expires_at;
        let chat_id_payload = Self::optional_metadata_payload_value(chat_id);
        let thread_id_payload = Self::optional_metadata_payload_value(thread_id);
        let expires_at_payload = Self::optional_metadata_payload_value(expires_at);
        let last_activity_at = args.last_activity_at;
        let previous = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "agent_id": agent_id,
                        "binding_kind": binding_kind,
                        "chat_id": chat_id_payload,
                        "thread_id": thread_id_payload,
                        "expires_at": expires_at_payload,
                        "last_activity_at": last_activity_at,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "topic_id": topic_id,
                        "agent_id": agent_id,
                        "binding_kind": binding_kind,
                        "chat_id": chat_id_payload,
                        "thread_id": thread_id_payload,
                        "expires_at": expires_at_payload,
                        "last_activity_at": last_activity_at
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_topic_binding(UpsertTopicBindingOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agent_id: agent_id.clone(),
                binding_kind,
                chat_id,
                thread_id,
                expires_at,
                last_activity_at,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic binding: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: Some(agent_id),
                action: TOOL_TOPIC_BINDING_SET.to_string(),
                payload: json!({
                    "topic_id": record.topic_id,
                    "agent_id": record.agent_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "binding": record }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_binding_get(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingGetArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_binding(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "binding": record
        }))
    }

    async fn execute_topic_binding_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_BINDING_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_BINDING_DELETE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "topic_id": topic_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic binding: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_BINDING_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_agent_profile_upsert(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileUpsertArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_UPSERT)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let profile = Self::validate_profile_object(args.profile)?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "profile": profile,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "agent_id": agent_id,
                        "profile": profile
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: agent_id.clone(),
                profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id),
                action: TOOL_AGENT_PROFILE_UPSERT.to_string(),
                payload: json!({
                    "agent_id": record.agent_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "profile": record }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_agent_profile_get(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileGetArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_GET)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;

        let record = self
            .storage
            .get_agent_profile(self.user_id, agent_id)
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "profile": record
        }))
    }

    async fn execute_topic_agent_tools_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentToolsGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let catalog = self.topic_agent_tool_catalog(&topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        let Some(binding) = binding else {
            return Self::to_json_string(json!({
                "ok": true,
                "found": false,
                "topic_id": topic_id
            }));
        };

        let profile = self
            .storage
            .get_agent_profile(self.user_id, binding.agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;
        let snapshot = Self::topic_agent_tool_snapshot(
            &catalog,
            profile.as_ref().map(|profile| &profile.profile),
        );

        Self::to_json_string(json!({
            "ok": true,
            "found": true,
            "topic_id": topic_id,
            "agent_id": binding.agent_id,
            "profile_found": profile.is_some(),
            "tools": snapshot
        }))
    }

    async fn execute_topic_agent_tools_enable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentToolsMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_ENABLE)?;
        self.execute_topic_agent_tools_mutation(args, TOOL_TOPIC_AGENT_TOOLS_ENABLE)
            .await
    }

    async fn execute_topic_agent_tools_disable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentToolsMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_TOOLS_DISABLE)?;
        self.execute_topic_agent_tools_mutation(args, TOOL_TOPIC_AGENT_TOOLS_DISABLE)
            .await
    }

    async fn execute_topic_agent_tools_mutation(
        &self,
        args: TopicAgentToolsMutationArgs,
        action: &str,
    ) -> Result<String> {
        let operation = Self::topic_agent_tools_operation_name(action)?;
        let (context, catalog) = self
            .prepare_topic_agent_tool_mutation(args.topic_id, args.tools)
            .await?;
        let previous_snapshot = Self::topic_agent_tool_snapshot(
            &catalog,
            context.previous.as_ref().map(|profile| &profile.profile),
        );
        let mutation = match action {
            TOOL_TOPIC_AGENT_TOOLS_ENABLE => {
                Self::enable_topic_agent_tools(context.previous.as_ref(), &context.requested_tools)?
            }
            TOOL_TOPIC_AGENT_TOOLS_DISABLE => Self::disable_topic_agent_tools(
                context.previous.as_ref(),
                &context.requested_tools,
            )?,
            _ => bail!("unsupported topic agent tools action: {action}"),
        };
        let snapshot = Self::topic_agent_tool_snapshot(&catalog, Some(&mutation.profile));

        if args.dry_run {
            let audit_status = self
                .append_topic_agent_tools_audit(
                    action,
                    &context,
                    mutation.changed,
                    Self::dry_run_outcome(true),
                    None,
                    None,
                )
                .await;

            return Self::topic_agent_tools_preview_response(
                operation,
                context,
                mutation.changed,
                mutation.profile,
                snapshot,
                audit_status,
            );
        }

        if !mutation.changed {
            let audit_status = self
                .append_topic_agent_tools_audit(action, &context, false, "noop", None, None)
                .await;

            return Self::topic_agent_tools_result_response(
                false,
                context,
                None,
                snapshot,
                None,
                audit_status,
            );
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: context.agent_id.clone(),
                profile: mutation.profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;
        let snapshot = Self::topic_agent_tool_snapshot(&catalog, Some(&record.profile));
        let sandbox_cleanup = if action == TOOL_TOPIC_AGENT_TOOLS_DISABLE
            && Self::sandbox_provider_enabled(&previous_snapshot)
            && !Self::sandbox_provider_enabled(&snapshot)
        {
            Some(
                self.cleanup_topic_sandbox_for_topic_id(&context.topic_id)
                    .await,
            )
        } else {
            None
        };

        let audit_status = self
            .append_topic_agent_tools_audit(
                action,
                &context,
                true,
                Self::dry_run_outcome(false),
                Some(record.version),
                sandbox_cleanup.clone(),
            )
            .await;

        Self::topic_agent_tools_result_response(
            true,
            TopicAgentToolMutationContext {
                agent_id: record.agent_id.clone(),
                ..context
            },
            Some(record),
            snapshot,
            sandbox_cleanup,
            audit_status,
        )
    }

    async fn execute_topic_agent_hooks_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentHooksGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let binding = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get topic binding: {err}"))?;

        let Some(binding) = binding else {
            return Self::to_json_string(json!({
                "ok": true,
                "found": false,
                "topic_id": topic_id
            }));
        };

        let profile = self
            .storage
            .get_agent_profile(self.user_id, binding.agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get agent profile: {err}"))?;
        let snapshot = Self::topic_agent_hook_snapshot(
            &Self::topic_agent_hook_catalog(),
            profile.as_ref().map(|profile| &profile.profile),
        );

        Self::to_json_string(json!({
            "ok": true,
            "found": true,
            "topic_id": topic_id,
            "agent_id": binding.agent_id,
            "profile_found": profile.is_some(),
            "hooks": snapshot
        }))
    }

    async fn execute_topic_agent_hooks_enable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentHooksMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_ENABLE)?;
        self.execute_topic_agent_hooks_mutation(args, TOOL_TOPIC_AGENT_HOOKS_ENABLE)
            .await
    }

    async fn execute_topic_agent_hooks_disable(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentHooksMutationArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENT_HOOKS_DISABLE)?;
        self.execute_topic_agent_hooks_mutation(args, TOOL_TOPIC_AGENT_HOOKS_DISABLE)
            .await
    }

    async fn execute_topic_agent_hooks_mutation(
        &self,
        args: TopicAgentHooksMutationArgs,
        action: &str,
    ) -> Result<String> {
        let operation = Self::topic_agent_hooks_operation_name(action)?;
        let (context, catalog) = self
            .prepare_topic_agent_hook_mutation(args.topic_id, args.hooks)
            .await?;
        let mutation = match action {
            TOOL_TOPIC_AGENT_HOOKS_ENABLE => {
                Self::enable_topic_agent_hooks(context.previous.as_ref(), &context.requested_hooks)?
            }
            TOOL_TOPIC_AGENT_HOOKS_DISABLE => Self::disable_topic_agent_hooks(
                context.previous.as_ref(),
                &context.requested_hooks,
            )?,
            _ => bail!("unsupported topic agent hooks action: {action}"),
        };
        let snapshot = Self::topic_agent_hook_snapshot(&catalog, Some(&mutation.profile));

        if args.dry_run {
            let audit_status = self
                .append_topic_agent_hooks_audit(
                    action,
                    &context,
                    mutation.changed,
                    Self::dry_run_outcome(true),
                    None,
                )
                .await;

            return Self::topic_agent_hooks_preview_response(
                operation,
                context,
                mutation.changed,
                mutation.profile,
                snapshot,
                audit_status,
            );
        }

        if !mutation.changed {
            let audit_status = self
                .append_topic_agent_hooks_audit(action, &context, false, "noop", None)
                .await;

            return Self::topic_agent_hooks_result_response(
                false,
                context,
                None,
                snapshot,
                audit_status,
            );
        }

        let record = self
            .storage
            .upsert_agent_profile(UpsertAgentProfileOptions {
                user_id: self.user_id,
                agent_id: context.agent_id.clone(),
                profile: mutation.profile,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert agent profile: {err}"))?;
        let snapshot = Self::topic_agent_hook_snapshot(&catalog, Some(&record.profile));

        let audit_status = self
            .append_topic_agent_hooks_audit(
                action,
                &context,
                true,
                Self::dry_run_outcome(false),
                Some(record.version),
            )
            .await;

        Self::topic_agent_hooks_result_response(
            true,
            TopicAgentHookMutationContext {
                agent_id: record.agent_id.clone(),
                ..context
            },
            Some(record),
            snapshot,
            audit_status,
        )
    }

    async fn execute_agent_profile_delete(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileDeleteArgs = Self::parse_args(arguments, TOOL_AGENT_PROFILE_DELETE)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let previous = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "agent_id": agent_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete agent profile: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id.clone()),
                action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                payload: json!({
                    "agent_id": agent_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_binding_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicBindingRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_BINDING_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_binding(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic binding: {err}"))?;
        let previous = match self.last_topic_binding_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicBindingRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = previous.as_ref().map_or("delete", |_| "restore");

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: previous.as_ref().map(|record| record.agent_id.clone()),
                    action: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "topic_id": topic_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_binding = if let Some(previous_binding) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_binding(UpsertTopicBindingOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        agent_id: previous_binding.agent_id,
                        binding_kind: Some(previous_binding.binding_kind),
                        chat_id: Self::restore_metadata_patch(previous_binding.chat_id),
                        thread_id: Self::restore_metadata_patch(previous_binding.thread_id),
                        expires_at: Self::restore_metadata_patch(previous_binding.expires_at),
                        last_activity_at: previous_binding.last_activity_at,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic binding: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_binding(self.user_id, topic_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete topic binding during rollback: {err}"))?;
            None
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: rolled_back_binding
                    .as_ref()
                    .map(|record| record.agent_id.clone()),
                action: TOOL_TOPIC_BINDING_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "rolled_back": true,
                "operation": rollback_operation,
                "binding": rolled_back_binding
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }

    async fn execute_topic_context_upsert(&self, arguments: &str) -> Result<String> {
        let args: TopicContextUpsertArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_UPSERT)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let context = Self::validate_topic_context(args.context)?;
        let previous = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "context": context,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "topic_id": topic_id,
                        "context": context
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_topic_context(UpsertTopicContextOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                context,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic context: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                payload: json!({
                    "topic_id": record.topic_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response =
            Self::attach_audit_status(json!({ "ok": true, "topic_context": record }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_context_get(&self, arguments: &str) -> Result<String> {
        let args: TopicContextGetArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_context(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic context: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_context": record
        }))
    }

    async fn execute_topic_context_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicContextDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "topic_id": topic_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic context: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_context_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicContextRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_CONTEXT_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_context(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic context: {err}"))?;
        let previous = match self.last_topic_context_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicContextRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "topic_id": topic_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_context = if let Some(previous_context) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_context(UpsertTopicContextOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        context: previous_context.context,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic context: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_context(self.user_id, topic_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete topic context during rollback: {err}"))?;
            None
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_CONTEXT_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": response_topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "operation": rollback_operation,
                "topic_context": rolled_back_context
            }),
            audit_status,
        );

        Self::to_json_string(response)
    }

    async fn execute_topic_agents_md_upsert(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdUpsertArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_UPSERT)?;
        let topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let agents_md = Self::validate_agents_md(args.agents_md)?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "agents_md": agents_md,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "topic_id": topic_id,
                        "agents_md": agents_md
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let record = self
            .storage
            .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                user_id: self.user_id,
                topic_id: topic_id.clone(),
                agents_md,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic AGENTS.md: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_UPSERT.to_string(),
                payload: json!({
                    "topic_id": record.topic_id,
                    "version": record.version,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({ "ok": true, "topic_agents_md": record }),
            audit_status,
        );
        Self::to_json_string(response)
    }

    async fn execute_topic_agents_md_get(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdGetArgs = Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic AGENTS.md: {err}"))?;

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_agents_md": record
        }))
    }

    async fn execute_topic_agents_md_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdDeleteArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "topic_id": topic_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic AGENTS.md: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_DELETE.to_string(),
                payload: json!({
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_agents_md_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicAgentsMdRollbackArgs =
            Self::parse_args(arguments, TOOL_TOPIC_AGENTS_MD_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_agents_md(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic AGENTS.md: {err}"))?;
        let previous = match self.last_topic_agents_md_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicAgentsMdRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "topic_id": topic_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_agents_md = if let Some(previous_agents_md) = previous.clone() {
            Some(
                self.storage
                    .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                        user_id: self.user_id,
                        topic_id: topic_id.clone(),
                        agents_md: previous_agents_md.agents_md,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore topic AGENTS.md: {err}"))?,
            )
        } else {
            self.storage
                .delete_topic_agents_md(self.user_id, topic_id.clone())
                .await
                .map_err(|err| {
                    anyhow!("failed to delete topic AGENTS.md during rollback: {err}")
                })?;
            None
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_AGENTS_MD_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": response_topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "operation": rollback_operation,
                "topic_agents_md": rolled_back_agents_md
            }),
            audit_status,
        );

        Self::to_json_string(response)
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

    async fn execute_topic_infra_upsert(&self, arguments: &str) -> Result<String> {
        let mut args =
            Self::validate_topic_infra_args(Self::parse_args(arguments, TOOL_TOPIC_INFRA_UPSERT)?)?;
        args.topic_id = self.resolve_mutation_topic_id(args.topic_id).await?;
        let desired = Self::topic_infra_value_from_args(&args);
        let previous = self
            .storage
            .get_topic_infra_config(self.user_id, args.topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(args.topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                    payload: json!({
                        "desired": desired,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "upsert",
                        "desired": desired,
                        "preflight": self
                            .inspect_topic_infra_record(&self.topic_infra_preview_record(&args))
                            .await
                    },
                    "previous": previous
                }),
                audit_status,
            ));
        }

        let record = self
            .storage
            .upsert_topic_infra_config(UpsertTopicInfraConfigOptions {
                user_id: self.user_id,
                topic_id: args.topic_id.clone(),
                target_name: args.target_name,
                host: args.host,
                port: args.port,
                remote_user: args.remote_user,
                auth_mode: args.auth_mode,
                secret_ref: args.secret_ref,
                sudo_secret_ref: args.sudo_secret_ref,
                environment: args.environment,
                tags: args.tags,
                allowed_tool_modes: args.allowed_tool_modes,
                approval_required_modes: args.approval_required_modes,
            })
            .await
            .map_err(|err| anyhow!("failed to upsert topic infra config: {err}"))?;
        let preflight = self.inspect_topic_infra_record(&record).await;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(args.topic_id),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_UPSERT.to_string(),
                payload: json!({
                    "record": Self::topic_infra_value_from_record(&record),
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({ "ok": true, "topic_infra": record, "preflight": preflight }),
            audit_status,
        ))
    }

    async fn execute_topic_infra_get(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraGetArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_GET)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;

        let record = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id)
            .await
            .map_err(|err| anyhow!("failed to get topic infra config: {err}"))?;
        let preflight = match record.as_ref() {
            Some(record) => Some(self.inspect_topic_infra_record(record).await),
            None => None,
        };

        Self::to_json_string(json!({
            "ok": true,
            "found": record.is_some(),
            "topic_infra": record,
            "preflight": preflight
        }))
    }

    async fn execute_topic_infra_delete(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraDeleteArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_DELETE)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let previous = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_INFRA_DELETE.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "previous": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": "delete",
                        "topic_id": topic_id
                    },
                    "previous": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        self.storage
            .delete_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to delete topic infra config: {err}"))?;

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id.clone()),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_DELETE.to_string(),
                payload: json!({
                    "topic_id": topic_id,
                    "previous": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(json!({ "ok": true }), audit_status);
        Self::to_json_string(response)
    }

    async fn execute_topic_infra_rollback(&self, arguments: &str) -> Result<String> {
        let args: TopicInfraRollbackArgs = Self::parse_args(arguments, TOOL_TOPIC_INFRA_ROLLBACK)?;
        let topic_id = self.resolve_lookup_topic_id(args.topic_id).await?;
        let current = self
            .storage
            .get_topic_infra_config(self.user_id, topic_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current topic infra config: {err}"))?;
        let previous = match self.last_topic_infra_mutation(&topic_id).await? {
            Some(event) => Self::previous_from_payload::<TopicInfraConfigRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: Some(topic_id.clone()),
                    agent_id: None,
                    action: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                    payload: json!({
                        "topic_id": topic_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            return Self::to_json_string(Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "topic_id": topic_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            ));
        }

        let rolled_back_infra = self
            .restore_or_delete_topic_infra(&topic_id, previous.clone())
            .await?;
        let preflight = match rolled_back_infra.as_ref() {
            Some(record) => Some(self.inspect_topic_infra_record(record).await),
            None => None,
        };

        let response_topic_id = topic_id.clone();
        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: Some(topic_id),
                agent_id: None,
                action: TOOL_TOPIC_INFRA_ROLLBACK.to_string(),
                payload: json!({
                    "topic_id": response_topic_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        Self::to_json_string(Self::attach_audit_status(
            json!({
                "ok": true,
                "operation": rollback_operation,
                "topic_infra": rolled_back_infra,
                "preflight": preflight
            }),
            audit_status,
        ))
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

    async fn execute_agent_profile_rollback(&self, arguments: &str) -> Result<String> {
        let args: AgentProfileRollbackArgs =
            Self::parse_args(arguments, TOOL_AGENT_PROFILE_ROLLBACK)?;
        let agent_id = Self::validate_non_empty(args.agent_id, "agent_id")?;
        let current = self
            .storage
            .get_agent_profile(self.user_id, agent_id.clone())
            .await
            .map_err(|err| anyhow!("failed to get current agent profile: {err}"))?;
        let previous = match self.last_agent_profile_mutation(&agent_id).await? {
            Some(event) => Self::previous_from_payload::<AgentProfileRecord>(&event.payload)?,
            None => None,
        };

        let rollback_operation = if previous.is_some() {
            "restore"
        } else {
            "delete"
        };

        if args.dry_run {
            let audit_status = self
                .append_audit_with_status(AppendAuditEventOptions {
                    user_id: self.user_id,
                    topic_id: None,
                    agent_id: Some(agent_id.clone()),
                    action: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                    payload: json!({
                        "agent_id": agent_id,
                        "operation": rollback_operation,
                        "previous": current,
                        "restore_to": previous,
                        "outcome": Self::dry_run_outcome(true)
                    }),
                })
                .await;

            let response = Self::attach_audit_status(
                json!({
                    "ok": true,
                    "dry_run": true,
                    "preview": {
                        "operation": rollback_operation,
                        "agent_id": agent_id
                    },
                    "current": current,
                    "restore_to": previous
                }),
                audit_status,
            );

            return Self::to_json_string(response);
        }

        let rolled_back_profile = if let Some(previous_profile) = previous.clone() {
            Some(
                self.storage
                    .upsert_agent_profile(UpsertAgentProfileOptions {
                        user_id: self.user_id,
                        agent_id: agent_id.clone(),
                        profile: previous_profile.profile,
                    })
                    .await
                    .map_err(|err| anyhow!("failed to restore agent profile: {err}"))?,
            )
        } else {
            self.storage
                .delete_agent_profile(self.user_id, agent_id.clone())
                .await
                .map_err(|err| anyhow!("failed to delete agent profile during rollback: {err}"))?;
            None
        };

        let audit_status = self
            .append_audit_with_status(AppendAuditEventOptions {
                user_id: self.user_id,
                topic_id: None,
                agent_id: Some(agent_id.clone()),
                action: TOOL_AGENT_PROFILE_ROLLBACK.to_string(),
                payload: json!({
                    "agent_id": agent_id,
                    "operation": rollback_operation,
                    "previous": current,
                    "restore_to": previous,
                    "outcome": Self::dry_run_outcome(false)
                }),
            })
            .await;

        let response = Self::attach_audit_status(
            json!({
                "ok": true,
                "rolled_back": true,
                "operation": rollback_operation,
                "profile": rolled_back_profile
            }),
            audit_status,
        );

        Self::to_json_string(response)
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
