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
mod forum_topics;
mod infra;
mod profiles;
mod sandboxes;
mod shared;

use self::audit::AuditStatus;
use self::forum_topics::ForumTopicProvisionSshAgentPlan;

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
        TopicInfraToolMode::Transfer,
    ]
}

fn default_infra_approval_required_modes() -> Vec<TopicInfraToolMode> {
    vec![
        TopicInfraToolMode::SudoExec,
        TopicInfraToolMode::ApplyFileEdit,
    ]
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateSecretProbeArgs {
    secret_ref: String,
    #[serde(default = "default_secret_probe_kind")]
    kind: SecretProbeKind,
}

const TOPIC_AGENT_TODOS_TOOLS: &[&str] = &["write_todos"];
const TOPIC_AGENT_AGENTS_MD_TOOLS: &[&str] = &["agents_md_get", "agents_md_update"];
const TOPIC_AGENT_SANDBOX_TOOLS: &[&str] = &[
    "execute_command",
    "write_file",
    "read_file",
    "send_file_to_user",
    "list_files",
    "recreate_sandbox",
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
#[cfg(feature = "searxng")]
const TOPIC_AGENT_SEARXNG_TOOLS: &[&str] = &["searxng_search"];
#[cfg(feature = "crawl4ai")]
const TOPIC_AGENT_CRAWL4AI_TOOLS: &[&str] = &["deep_crawl", "web_markdown", "web_pdf"];
#[cfg(feature = "browser_use")]
const TOPIC_AGENT_BROWSER_USE_TOOLS: &[&str] = &[
    "browser_use_run_task",
    "browser_use_get_session",
    "browser_use_close_session",
    "browser_use_extract_content",
    "browser_use_screenshot",
];
const TOPIC_AGENT_SSH_TOOLS: &[&str] = &[
    "ssh_exec",
    "ssh_sudo_exec",
    "ssh_read_file",
    "ssh_apply_file_edit",
    "ssh_check_process",
    "ssh_send_file_to_user",
];
#[cfg(feature = "jira")]
const TOPIC_AGENT_JIRA_TOOLS: &[&str] = &["jira_read", "jira_write", "jira_schema"];
#[cfg(feature = "mattermost")]
const TOPIC_AGENT_MATTERMOST_TOOLS: &[&str] = &[
    "mattermost_list_teams",
    "mattermost_get_team",
    "mattermost_get_team_members",
    "mattermost_list_channels",
    "mattermost_get_channel",
    "mattermost_get_channel_by_name",
    "mattermost_create_channel",
    "mattermost_join_channel",
    "mattermost_create_direct_channel",
    "mattermost_post_message",
    "mattermost_get_channel_messages",
    "mattermost_search_messages",
    "mattermost_update_message",
    "mattermost_get_thread",
    "mattermost_get_me",
    "mattermost_get_user",
    "mattermost_get_user_by_username",
    "mattermost_search_users",
    "mattermost_upload_file",
];
const TOPIC_AGENT_MEDIA_FILE_TOOLS: &[&str] = &[
    "transcribe_audio_file",
    "describe_image_file",
    "describe_video_file",
];
const TOPIC_AGENT_TTS_EN_TOOLS: &[&str] = &["text_to_speech_en", "text_to_speech_en_file"];
const TOPIC_AGENT_TTS_RU_TOOLS: &[&str] = &["text_to_speech_ru", "text_to_speech_ru_file"];

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

    fn tools_definitions(&self) -> Vec<ToolDefinition> {
        let mut tools = Self::base_tools_definitions();
        if self.topic_lifecycle.is_some() {
            tools.extend(Self::lifecycle_tools_definitions());
        }

        tools
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
