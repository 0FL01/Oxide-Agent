//! Manager control-plane provider.
//!
//! Exposes user-scoped CRUD tools for topic bindings, topic contexts, and agent profiles.

use super::ssh_mcp::{inspect_topic_infra_config, probe_secret_ref, SecretProbeKind};
use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxManager, SandboxScope};
use crate::storage::{
    AgentProfileRecord, AppendAuditEventOptions, OptionalMetadataPatch, StorageProvider,
    TopicBindingKind, TopicBindingRecord, TopicContextRecord, TopicInfraAuthMode,
    TopicInfraConfigRecord, TopicInfraToolMode, UpsertAgentProfileOptions,
    UpsertTopicBindingOptions, UpsertTopicContextOptions, UpsertTopicInfraConfigOptions,
    UserConfig,
};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

const TOOL_TOPIC_BINDING_SET: &str = "topic_binding_set";
const TOOL_TOPIC_BINDING_GET: &str = "topic_binding_get";
const TOOL_TOPIC_BINDING_DELETE: &str = "topic_binding_delete";
const TOOL_TOPIC_BINDING_ROLLBACK: &str = "topic_binding_rollback";
const TOOL_TOPIC_CONTEXT_UPSERT: &str = "topic_context_upsert";
const TOOL_TOPIC_CONTEXT_GET: &str = "topic_context_get";
const TOOL_TOPIC_CONTEXT_DELETE: &str = "topic_context_delete";
const TOOL_TOPIC_CONTEXT_ROLLBACK: &str = "topic_context_rollback";
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
const TOOL_FORUM_TOPIC_CREATE: &str = "forum_topic_create";
const TOOL_FORUM_TOPIC_EDIT: &str = "forum_topic_edit";
const TOOL_FORUM_TOPIC_CLOSE: &str = "forum_topic_close";
const TOOL_FORUM_TOPIC_REOPEN: &str = "forum_topic_reopen";
const TOOL_FORUM_TOPIC_DELETE: &str = "forum_topic_delete";
const TOOL_FORUM_TOPIC_LIST: &str = "forum_topic_list";
const ROLLBACK_AUDIT_PAGE_SIZE: usize = 200;
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
    ]
}

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

#[derive(Default)]
struct DockerTopicSandboxCleanup;

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

    fn forum_topic_context_key(chat_id: i64, thread_id: i64) -> String {
        format!("{chat_id}:{thread_id}")
    }

    fn forum_topic_binding_keys(chat_id: i64, thread_id: i64) -> Vec<String> {
        let context_key = Self::forum_topic_context_key(chat_id, thread_id);
        let raw_thread_key = thread_id.to_string();
        if raw_thread_key == context_key {
            vec![context_key]
        } else {
            vec![context_key, raw_thread_key]
        }
    }

    fn resolve_default_forum_chat_id(&self) -> Option<i64> {
        self.topic_lifecycle
            .as_ref()
            .and_then(|lifecycle| lifecycle.default_forum_chat_id())
    }

    fn forum_topic_catalog_entry_from_context(
        context_key: &str,
        context: &crate::storage::UserContextConfig,
    ) -> Option<ForumTopicCatalogEntry> {
        let chat_id = context.chat_id?;
        let thread_id = context.thread_id?;
        if chat_id >= 0 || thread_id <= 0 {
            return None;
        }

        let expected_key = Self::forum_topic_context_key(chat_id, thread_id);
        if context_key != expected_key {
            return None;
        }

        Some(ForumTopicCatalogEntry {
            topic_id: expected_key,
            chat_id,
            thread_id,
            name: context.forum_topic_name.clone(),
            icon_color: context.forum_topic_icon_color,
            icon_custom_emoji_id: context.forum_topic_icon_custom_emoji_id.clone(),
            closed: context.forum_topic_closed,
        })
    }

    fn upsert_forum_topic_catalog_entry(config: &mut UserConfig, entry: &ForumTopicCatalogEntry) {
        let context = config.contexts.entry(entry.topic_id.clone()).or_default();
        context.chat_id = Some(entry.chat_id);
        context.thread_id = Some(entry.thread_id);
        context.forum_topic_name = entry.name.clone();
        context.forum_topic_icon_color = entry.icon_color;
        context.forum_topic_icon_custom_emoji_id = entry.icon_custom_emoji_id.clone();
        context.forum_topic_closed = entry.closed;
    }

    fn existing_forum_topic_catalog_entry(
        config: &UserConfig,
        topic_id: &str,
    ) -> Option<ForumTopicCatalogEntry> {
        config
            .contexts
            .get(topic_id)
            .and_then(|context| Self::forum_topic_catalog_entry_from_context(topic_id, context))
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
        tools.extend(Self::topic_infra_tools_definitions());
        tools.extend(Self::private_secret_tools_definitions());
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
                description: "Create or update topic-specific execution context for current user"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic_id": { "type": "string", "description": "Stable topic identifier" },
                        "context": { "type": "string", "description": "Free-form topic instructions injected into the agent prompt" },
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
        vec![
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

    fn validate_non_empty(value: String, field_name: &str) -> Result<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("{field_name} must not be empty");
        }
        Ok(trimmed.to_string())
    }

    fn validate_thread_id(thread_id: i64) -> Result<i64> {
        if thread_id <= 0 {
            bail!("thread_id must be a positive integer");
        }
        Ok(thread_id)
    }

    fn validate_optional_non_empty(
        value: Option<String>,
        field_name: &str,
    ) -> Result<Option<String>> {
        value
            .map(|inner| Self::validate_non_empty(inner, field_name))
            .transpose()
    }

    fn normalize_tags(tags: Vec<String>) -> Vec<String> {
        let mut tags = tags
            .into_iter()
            .map(|tag| tag.trim().to_string())
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        tags
    }

    fn normalize_tool_modes(modes: Vec<TopicInfraToolMode>) -> Vec<TopicInfraToolMode> {
        let mut modes = modes;
        modes.sort_by_key(|mode| match mode {
            TopicInfraToolMode::Exec => 0,
            TopicInfraToolMode::SudoExec => 1,
            TopicInfraToolMode::ReadFile => 2,
            TopicInfraToolMode::ApplyFileEdit => 3,
            TopicInfraToolMode::CheckProcess => 4,
        });
        modes.dedup();
        modes
    }

    fn validate_topic_infra_args(args: TopicInfraUpsertArgs) -> Result<TopicInfraUpsertArgs> {
        let topic_id = Self::validate_non_empty(args.topic_id, "topic_id")?;
        let target_name = Self::validate_non_empty(args.target_name, "target_name")?;
        let host = Self::validate_non_empty(args.host, "host")?;
        let remote_user = Self::validate_non_empty(args.remote_user, "remote_user")?;
        if args.port == 0 {
            bail!("port must be a positive integer");
        }

        let secret_ref = Self::validate_optional_non_empty(args.secret_ref, "secret_ref")?;
        let sudo_secret_ref =
            Self::validate_optional_non_empty(args.sudo_secret_ref, "sudo_secret_ref")?;
        let environment = Self::validate_optional_non_empty(args.environment, "environment")?;
        let allowed_tool_modes = Self::normalize_tool_modes(args.allowed_tool_modes);
        if allowed_tool_modes.is_empty() {
            bail!("allowed_tool_modes must not be empty");
        }
        let approval_required_modes = Self::normalize_tool_modes(args.approval_required_modes);

        Ok(TopicInfraUpsertArgs {
            topic_id,
            target_name,
            host,
            port: args.port,
            remote_user,
            auth_mode: args.auth_mode,
            secret_ref,
            sudo_secret_ref,
            environment,
            tags: Self::normalize_tags(args.tags),
            allowed_tool_modes,
            approval_required_modes,
            dry_run: args.dry_run,
        })
    }

    fn topic_infra_value_from_args(args: &TopicInfraUpsertArgs) -> serde_json::Value {
        json!({
            "topic_id": args.topic_id,
            "target_name": args.target_name,
            "host": args.host,
            "port": args.port,
            "remote_user": args.remote_user,
            "auth_mode": args.auth_mode,
            "secret_ref": args.secret_ref,
            "sudo_secret_ref": args.sudo_secret_ref,
            "environment": args.environment,
            "tags": args.tags,
            "allowed_tool_modes": args.allowed_tool_modes,
            "approval_required_modes": args.approval_required_modes,
        })
    }

    fn topic_infra_value_from_record(record: &TopicInfraConfigRecord) -> serde_json::Value {
        json!({
            "topic_id": record.topic_id,
            "version": record.version,
            "target_name": record.target_name,
            "host": record.host,
            "port": record.port,
            "remote_user": record.remote_user,
            "auth_mode": record.auth_mode,
            "secret_ref": record.secret_ref,
            "sudo_secret_ref": record.sudo_secret_ref,
            "environment": record.environment,
            "tags": record.tags,
            "allowed_tool_modes": record.allowed_tool_modes,
            "approval_required_modes": record.approval_required_modes,
        })
    }

    fn topic_infra_preview_record(&self, args: &TopicInfraUpsertArgs) -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version: 0,
            user_id: self.user_id,
            topic_id: args.topic_id.clone(),
            target_name: args.target_name.clone(),
            host: args.host.clone(),
            port: args.port,
            remote_user: args.remote_user.clone(),
            auth_mode: args.auth_mode,
            secret_ref: args.secret_ref.clone(),
            sudo_secret_ref: args.sudo_secret_ref.clone(),
            environment: args.environment.clone(),
            tags: args.tags.clone(),
            allowed_tool_modes: args.allowed_tool_modes.clone(),
            approval_required_modes: args.approval_required_modes.clone(),
            created_at: 0,
            updated_at: 0,
        }
    }

    async fn inspect_topic_infra_record(
        &self,
        record: &TopicInfraConfigRecord,
    ) -> crate::agent::providers::TopicInfraPreflightReport {
        inspect_topic_infra_config(&self.storage, self.user_id, &record.topic_id, record).await
    }

    fn topic_lifecycle(&self) -> Result<&Arc<dyn ManagerTopicLifecycle>> {
        self.topic_lifecycle
            .as_ref()
            .ok_or_else(|| anyhow!("forum topic lifecycle service is unavailable"))
    }

    fn validate_forum_icon_color(color: Option<u32>) -> Result<Option<u32>> {
        if let Some(value) = color {
            if !TELEGRAM_FORUM_ICON_COLORS.contains(&value) {
                bail!("icon_color is not one of Telegram allowed values");
            }
            return Ok(Some(value));
        }

        Ok(None)
    }

    fn validate_profile_object(profile: serde_json::Value) -> Result<serde_json::Value> {
        if !profile.is_object() {
            bail!("profile must be a JSON object");
        }
        if profile.get("tools").is_some() {
            bail!(
                "profile.tools is not supported; use allowedTools/blockedTools or forum_topic_provision_ssh_agent"
            );
        }
        Ok(profile)
    }

    fn is_canonical_forum_topic_id(value: &str) -> bool {
        let Some((chat_id, thread_id)) = value.split_once(':') else {
            return false;
        };
        chat_id.parse::<i64>().is_ok() && thread_id.parse::<i64>().ok().is_some_and(|id| id > 0)
    }

    async fn resolve_mutation_topic_id(&self, topic_id: String) -> Result<String> {
        let topic_id = Self::validate_non_empty(topic_id, "topic_id")?;
        if Self::is_canonical_forum_topic_id(&topic_id) || self.topic_lifecycle.is_none() {
            return Ok(topic_id);
        }

        match self.resolve_forum_topic_id_alias(&topic_id).await? {
            Some(resolved) => Ok(resolved),
            None => bail!(
                "topic_id '{topic_id}' is not a canonical Telegram forum topic id. Use '<chat_id>:<thread_id>' from forum_topic_create / forum_topic_provision_ssh_agent results."
            ),
        }
    }

    async fn resolve_lookup_topic_id(&self, topic_id: String) -> Result<String> {
        let topic_id = Self::validate_non_empty(topic_id, "topic_id")?;
        if Self::is_canonical_forum_topic_id(&topic_id) || self.topic_lifecycle.is_none() {
            return Ok(topic_id);
        }

        Ok(self
            .resolve_forum_topic_id_alias(&topic_id)
            .await?
            .unwrap_or(topic_id))
    }

    async fn resolve_forum_topic_id_alias(&self, alias: &str) -> Result<Option<String>> {
        if self.topic_lifecycle.is_none() {
            return Ok(None);
        }

        let mut matches = self
            .list_forum_topic_catalog_entries(None, true)
            .await?
            .into_iter()
            .filter(|entry| entry.name.as_deref() == Some(alias))
            .collect::<Vec<_>>();

        matches.sort_by(|left, right| left.topic_id.cmp(&right.topic_id));
        matches.dedup_by(|left, right| left.topic_id == right.topic_id);

        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop().map(|entry| entry.topic_id)),
            _ => bail!(
                "topic alias '{alias}' is ambiguous across multiple forum topics; use canonical '<chat_id>:<thread_id>'"
            ),
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

    fn to_json_string(value: serde_json::Value) -> Result<String> {
        serde_json::to_string(&value)
            .map_err(|err| anyhow!("failed to serialize tool response: {err}"))
    }

    fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str, tool_name: &str) -> Result<T> {
        serde_json::from_str(arguments).map_err(|err| anyhow!("invalid {tool_name} args: {err}"))
    }

    fn dry_run_outcome(dry_run: bool) -> &'static str {
        if dry_run {
            "dry_run"
        } else {
            "applied"
        }
    }

    fn optional_metadata_payload_value(value: OptionalMetadataPatch<i64>) -> Option<i64> {
        match value {
            OptionalMetadataPatch::Set(inner) => Some(inner),
            OptionalMetadataPatch::Keep | OptionalMetadataPatch::Clear => None,
        }
    }

    fn restore_metadata_patch(value: Option<i64>) -> OptionalMetadataPatch<i64> {
        value
            .map(OptionalMetadataPatch::Set)
            .unwrap_or(OptionalMetadataPatch::Clear)
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
        event.payload.get("outcome") != Some(&json!("dry_run"))
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
        let context = Self::validate_non_empty(args.context, "context")?;
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
        let base_tools = matches!(
            tool_name,
            TOOL_TOPIC_BINDING_SET
                | TOOL_TOPIC_BINDING_GET
                | TOOL_TOPIC_BINDING_DELETE
                | TOOL_TOPIC_BINDING_ROLLBACK
                | TOOL_TOPIC_CONTEXT_UPSERT
                | TOOL_TOPIC_CONTEXT_GET
                | TOOL_TOPIC_CONTEXT_DELETE
                | TOOL_TOPIC_CONTEXT_ROLLBACK
                | TOOL_TOPIC_INFRA_UPSERT
                | TOOL_TOPIC_INFRA_GET
                | TOOL_TOPIC_INFRA_DELETE
                | TOOL_TOPIC_INFRA_ROLLBACK
                | TOOL_PRIVATE_SECRET_PROBE
                | TOOL_AGENT_PROFILE_UPSERT
                | TOOL_AGENT_PROFILE_GET
                | TOOL_AGENT_PROFILE_DELETE
                | TOOL_AGENT_PROFILE_ROLLBACK
        );

        base_tools
            || (self.topic_lifecycle.is_some()
                && matches!(
                    tool_name,
                    TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT
                        | TOOL_FORUM_TOPIC_CREATE
                        | TOOL_FORUM_TOPIC_EDIT
                        | TOOL_FORUM_TOPIC_CLOSE
                        | TOOL_FORUM_TOPIC_REOPEN
                        | TOOL_FORUM_TOPIC_DELETE
                        | TOOL_FORUM_TOPIC_LIST
                ))
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
mod tests {
    use super::*;
    use crate::agent::registry::ToolRegistry;
    use crate::storage::{
        AgentProfileRecord, AppendAuditEventOptions, TopicBindingRecord, TopicContextRecord,
        TopicInfraAuthMode, TopicInfraConfigRecord, TopicInfraToolMode,
    };
    use mockall::{predicate::eq, Sequence};

    fn binding(user_id: i64, topic_id: &str, agent_id: &str, version: u64) -> TopicBindingRecord {
        TopicBindingRecord {
            schema_version: 1,
            version,
            user_id,
            topic_id: topic_id.to_string(),
            agent_id: agent_id.to_string(),
            binding_kind: TopicBindingKind::Manual,
            chat_id: None,
            thread_id: None,
            expires_at: None,
            last_activity_at: Some(20),
            created_at: 10,
            updated_at: 20,
        }
    }

    fn topic_infra(user_id: i64, topic_id: &str, version: u64) -> TopicInfraConfigRecord {
        TopicInfraConfigRecord {
            schema_version: 1,
            version,
            user_id,
            topic_id: topic_id.to_string(),
            target_name: "prod-app".to_string(),
            host: "prod.example.com".to_string(),
            port: 22,
            remote_user: "deploy".to_string(),
            auth_mode: TopicInfraAuthMode::PrivateKey,
            secret_ref: Some("storage:ssh/prod-key".to_string()),
            sudo_secret_ref: Some("storage:ssh/prod-sudo".to_string()),
            environment: Some("prod".to_string()),
            tags: vec!["prod".to_string()],
            allowed_tool_modes: vec![TopicInfraToolMode::Exec, TopicInfraToolMode::ReadFile],
            approval_required_modes: vec![TopicInfraToolMode::SudoExec],
            created_at: 10,
            updated_at: 20,
        }
    }

    fn mock_storage_for_forum_topic_provision() -> crate::storage::MockStorageProvider {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_user_config()
            .times(1)
            .returning(|_| Ok(crate::storage::UserConfig::default()));
        mock.expect_update_user_config()
            .times(1)
            .withf(|user_id, config| *user_id == 77 && config.contexts.contains_key("-100777:313"))
            .returning(|_, _| Ok(()));
        mock.expect_get_agent_profile()
            .with(eq(77_i64), eq("n-ru1".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_agent_profile()
            .withf(|options| {
                options.agent_id == "n-ru1"
                    && options
                        .profile
                        .get("allowedTools")
                        .and_then(|value| value.as_array())
                        .is_some()
            })
            .returning(|options| {
                Ok(AgentProfileRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: options.user_id,
                    agent_id: options.agent_id,
                    profile: options.profile,
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("-100777:313".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.topic_id == "-100777:313"
                    && options.agent_id == "n-ru1"
                    && options.chat_id == OptionalMetadataPatch::Set(-100777)
                    && options.thread_id == OptionalMetadataPatch::Set(313)
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_get_topic_infra_config()
            .with(eq(77_i64), eq("-100777:313".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_infra_config()
            .withf(|options| {
                options.topic_id == "-100777:313"
                    && options.target_name == "n-ru1"
                    && options.auth_mode == TopicInfraAuthMode::None
            })
            .returning(|options| {
                Ok(TopicInfraConfigRecord {
                    schema_version: 1,
                    version: 1,
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
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_append_audit_event()
            .times(4)
            .returning(|options| {
                Ok(audit_event(
                    1,
                    options.topic_id.as_deref(),
                    options.agent_id.as_deref(),
                    &options.action,
                    options.payload,
                ))
            });
        mock
    }

    fn audit_event(
        version: u64,
        topic_id: Option<&str>,
        agent_id: Option<&str>,
        action: &str,
        payload: serde_json::Value,
    ) -> crate::storage::AuditEventRecord {
        crate::storage::AuditEventRecord {
            schema_version: 1,
            version,
            event_id: format!("evt-{version}"),
            user_id: 77,
            topic_id: topic_id.map(str::to_string),
            agent_id: agent_id.map(str::to_string),
            action: action.to_string(),
            payload,
            created_at: 100,
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum LifecycleCall {
        Create(ForumTopicCreateRequest),
        Edit(ForumTopicEditRequest),
        Close(ForumTopicThreadRequest),
        Reopen(ForumTopicThreadRequest),
        Delete(ForumTopicThreadRequest),
    }

    struct FakeTopicLifecycle {
        calls: std::sync::Mutex<Vec<LifecycleCall>>,
    }

    impl FakeTopicLifecycle {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<LifecycleCall> {
            self.calls.lock().expect("mutex poisoned").clone()
        }
    }

    #[async_trait]
    impl ManagerTopicLifecycle for FakeTopicLifecycle {
        fn default_forum_chat_id(&self) -> Option<i64> {
            Some(-100_777)
        }

        async fn forum_topic_create(
            &self,
            request: ForumTopicCreateRequest,
        ) -> Result<ForumTopicCreateResult> {
            self.calls
                .lock()
                .expect("mutex poisoned")
                .push(LifecycleCall::Create(request.clone()));
            Ok(ForumTopicCreateResult {
                chat_id: request.chat_id.unwrap_or(-100_777),
                thread_id: 313,
                name: request.name,
                icon_color: request.icon_color.unwrap_or(9_367_192),
                icon_custom_emoji_id: request.icon_custom_emoji_id,
            })
        }

        async fn forum_topic_edit(
            &self,
            request: ForumTopicEditRequest,
        ) -> Result<ForumTopicEditResult> {
            self.calls
                .lock()
                .expect("mutex poisoned")
                .push(LifecycleCall::Edit(request.clone()));
            Ok(ForumTopicEditResult {
                chat_id: request.chat_id.unwrap_or(-100_777),
                thread_id: request.thread_id,
                name: request.name,
                icon_custom_emoji_id: request.icon_custom_emoji_id,
            })
        }

        async fn forum_topic_close(
            &self,
            request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            self.calls
                .lock()
                .expect("mutex poisoned")
                .push(LifecycleCall::Close(request.clone()));
            Ok(ForumTopicActionResult {
                chat_id: request.chat_id.unwrap_or(-100_777),
                thread_id: request.thread_id,
            })
        }

        async fn forum_topic_reopen(
            &self,
            request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            self.calls
                .lock()
                .expect("mutex poisoned")
                .push(LifecycleCall::Reopen(request.clone()));
            Ok(ForumTopicActionResult {
                chat_id: request.chat_id.unwrap_or(-100_777),
                thread_id: request.thread_id,
            })
        }

        async fn forum_topic_delete(
            &self,
            request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            self.calls
                .lock()
                .expect("mutex poisoned")
                .push(LifecycleCall::Delete(request.clone()));
            Ok(ForumTopicActionResult {
                chat_id: request.chat_id.unwrap_or(-100_777),
                thread_id: request.thread_id,
            })
        }
    }

    struct FakeTopicSandboxCleanup {
        calls: std::sync::Mutex<Vec<(i64, i64, i64)>>,
    }

    impl FakeTopicSandboxCleanup {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<(i64, i64, i64)> {
            self.calls.lock().expect("mutex poisoned").clone()
        }
    }

    #[async_trait]
    impl ManagerTopicSandboxCleanup for FakeTopicSandboxCleanup {
        async fn cleanup_topic_sandbox(
            &self,
            user_id: i64,
            topic: &ForumTopicActionResult,
        ) -> Result<()> {
            self.calls.lock().expect("mutex poisoned").push((
                user_id,
                topic.chat_id,
                topic.thread_id,
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn forum_topic_tools_unavailable_without_lifecycle_service() {
        let provider = ManagerControlPlaneProvider::new(
            Arc::new(crate::storage::MockStorageProvider::new()),
            77,
        );
        let tool_names: Vec<String> = provider.tools().into_iter().map(|tool| tool.name).collect();

        assert!(!tool_names
            .iter()
            .any(|name| name == TOOL_FORUM_TOPIC_CREATE));
        assert!(!tool_names.iter().any(|name| name == TOOL_FORUM_TOPIC_LIST));
        assert!(!provider.can_handle(TOOL_FORUM_TOPIC_CREATE));
        assert!(!provider.can_handle(TOOL_FORUM_TOPIC_LIST));
    }

    #[tokio::test]
    async fn forum_topic_dry_run_mutations_do_not_call_lifecycle_service() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_append_audit_event()
            .times(3)
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-1".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 100,
                })
            });

        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
            .with_topic_lifecycle(lifecycle.clone());

        provider
            .execute(
                TOOL_FORUM_TOPIC_CREATE,
                r#"{"name":"topic-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("create dry-run should succeed");
        provider
            .execute(
                TOOL_FORUM_TOPIC_EDIT,
                r#"{"thread_id":42,"name":"topic-b","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("edit dry-run should succeed");
        provider
            .execute(
                TOOL_FORUM_TOPIC_DELETE,
                r#"{"thread_id":42,"dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("delete dry-run should succeed");

        assert!(lifecycle.calls().is_empty());
    }

    #[tokio::test]
    async fn forum_topic_create_invokes_lifecycle_and_audits_success() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_user_config()
            .times(1)
            .returning(|_| Ok(crate::storage::UserConfig::default()));
        mock.expect_update_user_config()
            .withf(|user_id, config| {
                *user_id == 77
                    && config.contexts.get("-100999:313").is_some_and(|context| {
                        context.chat_id == Some(-100999)
                            && context.thread_id == Some(313)
                            && context.forum_topic_name.as_deref() == Some("topic-a")
                            && context.forum_topic_icon_color == Some(9_367_192)
                            && !context.forum_topic_closed
                    })
            })
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.topic_id.as_deref() == Some("-100999:313")
                    && options.action == TOOL_FORUM_TOPIC_CREATE
                    && options.payload.get("outcome") == Some(&json!("applied"))
                    && options
                        .payload
                        .get("result")
                        .and_then(|result| result.get("thread_id"))
                        == Some(&json!(313))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-topic-create".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 100,
                })
            });

        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
            .with_topic_lifecycle(lifecycle.clone());

        let response = provider
            .execute(
                TOOL_FORUM_TOPIC_CREATE,
                r#"{"chat_id":-100999,"name":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("forum topic create should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["topic"]["thread_id"], 313);
        assert_eq!(parsed["audit_status"], "written");
        assert_eq!(lifecycle.calls().len(), 1);
    }

    #[tokio::test]
    async fn forum_topic_list_returns_persisted_topics_for_current_chat() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_user_config().times(1).returning(|_| {
            Ok(crate::storage::UserConfig {
                contexts: std::collections::HashMap::from([
                    (
                        "-100777:12".to_string(),
                        crate::storage::UserContextConfig {
                            state: None,
                            current_chat_uuid: None,
                            current_agent_flow_id: None,
                            chat_id: Some(-100777),
                            thread_id: Some(12),
                            forum_topic_name: Some("Alfa".to_string()),
                            forum_topic_icon_color: Some(16_766_590),
                            forum_topic_icon_custom_emoji_id: Some("emoji-1".to_string()),
                            forum_topic_closed: false,
                        },
                    ),
                    (
                        "-100777:20".to_string(),
                        crate::storage::UserContextConfig {
                            state: None,
                            current_chat_uuid: None,
                            current_agent_flow_id: None,
                            chat_id: Some(-100777),
                            thread_id: Some(20),
                            forum_topic_name: Some("Beta".to_string()),
                            forum_topic_icon_color: Some(7_322_096),
                            forum_topic_icon_custom_emoji_id: None,
                            forum_topic_closed: true,
                        },
                    ),
                    (
                        "-100888:7".to_string(),
                        crate::storage::UserContextConfig {
                            state: None,
                            current_chat_uuid: None,
                            current_agent_flow_id: None,
                            chat_id: Some(-100888),
                            thread_id: Some(7),
                            forum_topic_name: Some("Gamma".to_string()),
                            forum_topic_icon_color: None,
                            forum_topic_icon_custom_emoji_id: None,
                            forum_topic_closed: false,
                        },
                    ),
                ]),
                ..crate::storage::UserConfig::default()
            })
        });

        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let provider =
            ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);

        let response = provider
            .execute(TOOL_FORUM_TOPIC_LIST, r#"{}"#, None, None)
            .await
            .expect("forum topic list should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["chat_id"], -100777);
        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["topics"][0]["topic_id"], "-100777:12");
        assert_eq!(parsed["topics"][0]["name"], "Alfa");
        assert_eq!(parsed["topics"][0]["closed"], false);
    }

    #[tokio::test]
    async fn forum_topic_delete_cleans_topic_storage_and_sandbox() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_clear_agent_memory_for_context()
            .with(eq(77_i64), eq("-100999:42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_clear_chat_history_for_context()
            .with(eq(77_i64), eq("-100999:42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_delete_topic_context()
            .with(eq(77_i64), eq("-100999:42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_delete_topic_infra_config()
            .with(eq(77_i64), eq("-100999:42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_delete_topic_binding()
            .with(eq(77_i64), eq("-100999:42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_delete_topic_binding()
            .with(eq(77_i64), eq("42".to_string()))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_get_user_config().times(1).returning(|_| {
            Ok(crate::storage::UserConfig {
                contexts: std::collections::HashMap::from([(
                    "-100999:42".to_string(),
                    crate::storage::UserContextConfig {
                        state: Some("agent_mode".to_string()),
                        current_chat_uuid: Some("chat-1".to_string()),
                        current_agent_flow_id: Some("flow-1".to_string()),
                        chat_id: Some(-100999),
                        thread_id: Some(42),
                        forum_topic_name: Some("topic-42".to_string()),
                        forum_topic_icon_color: Some(7_322_096),
                        forum_topic_icon_custom_emoji_id: None,
                        forum_topic_closed: false,
                    },
                )]),
                ..crate::storage::UserConfig::default()
            })
        });
        mock.expect_update_user_config()
            .withf(|user_id, config| *user_id == 77 && !config.contexts.contains_key("-100999:42"))
            .times(1)
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.topic_id.as_deref() == Some("-100999:42")
                    && options.action == TOOL_FORUM_TOPIC_DELETE
                    && options
                        .payload
                        .get("cleanup")
                        .and_then(|cleanup| cleanup.get("deleted_container"))
                        == Some(&json!(true))
            })
            .times(1)
            .returning(|options| {
                Ok(audit_event(
                    1,
                    options.topic_id.as_deref(),
                    None,
                    &options.action,
                    options.payload,
                ))
            });

        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let sandbox_cleanup = Arc::new(FakeTopicSandboxCleanup::new());
        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77)
            .with_topic_lifecycle(lifecycle.clone())
            .with_topic_sandbox_cleanup(sandbox_cleanup.clone());

        let response = provider
            .execute(
                TOOL_FORUM_TOPIC_DELETE,
                r#"{"chat_id":-100999,"thread_id":42}"#,
                None,
                None,
            )
            .await
            .expect("forum topic delete should clean topic artifacts");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["topic"]["thread_id"], 42);
        assert_eq!(parsed["cleanup"]["context_key"], "-100999:42");
        assert_eq!(parsed["cleanup"]["deleted_container"], true);
        assert_eq!(parsed["audit_status"], "written");
        assert_eq!(
            lifecycle.calls(),
            vec![LifecycleCall::Delete(ForumTopicThreadRequest {
                chat_id: Some(-100999),
                thread_id: 42
            })]
        );
        assert_eq!(sandbox_cleanup.calls(), vec![(77, -100999, 42)]);
    }

    #[tokio::test]
    async fn topic_binding_set_rejects_empty_topic_id() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"   ","agent_id":"agent-1"}"#,
                None,
                None,
            )
            .await
            .expect_err("expected validation error");

        assert!(err.to_string().contains("topic_id must not be empty"));
    }

    #[tokio::test]
    async fn topic_binding_get_rejects_unknown_fields() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_TOPIC_BINDING_GET,
                r#"{"topic_id":"topic-a","extra":true}"#,
                None,
                None,
            )
            .await
            .expect_err("expected strict serde validation error");

        assert!(err.to_string().contains("unknown field"));
    }

    #[tokio::test]
    async fn agent_profile_upsert_rejects_non_object_profile() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_AGENT_PROFILE_UPSERT,
                r#"{"agent_id":"agent-a","profile":[1,2,3]}"#,
                None,
                None,
            )
            .await
            .expect_err("expected profile validation error");

        assert!(err.to_string().contains("profile must be a JSON object"));
    }

    #[tokio::test]
    async fn agent_profile_upsert_rejects_legacy_tools_shorthand() {
        let storage = Arc::new(crate::storage::MockStorageProvider::new());
        let provider = ManagerControlPlaneProvider::new(storage, 77);
        let err = provider
            .execute(
                TOOL_AGENT_PROFILE_UPSERT,
                r#"{"agent_id":"agent-a","profile":{"tools":["ssh"]}}"#,
                None,
                None,
            )
            .await
            .expect_err("expected unsupported profile.tools validation error");

        assert!(err.to_string().contains("allowedTools/blockedTools"));
    }

    #[tokio::test]
    async fn topic_infra_upsert_resolves_unique_forum_topic_name_alias() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_user_config().returning(|_| {
            Ok(crate::storage::UserConfig {
                contexts: std::collections::HashMap::from([(
                    "-100777:240".to_string(),
                    crate::storage::UserContextConfig {
                        chat_id: Some(-100777),
                        thread_id: Some(240),
                        forum_topic_name: Some("n-ru1".to_string()),
                        forum_topic_icon_color: Some(9_367_192),
                        ..crate::storage::UserContextConfig::default()
                    },
                )]),
                ..crate::storage::UserConfig::default()
            })
        });
        mock.expect_get_topic_infra_config()
            .with(eq(77_i64), eq("-100777:240".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_infra_config()
            .withf(|options| options.topic_id == "-100777:240")
            .returning(|options| {
                Ok(TopicInfraConfigRecord {
                    schema_version: 1,
                    version: 1,
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
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_append_audit_event().returning(|options| {
            Ok(audit_event(
                1,
                options.topic_id.as_deref(),
                options.agent_id.as_deref(),
                &options.action,
                options.payload,
            ))
        });

        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let provider =
            ManagerControlPlaneProvider::new(Arc::new(mock), 77).with_topic_lifecycle(lifecycle);
        let response = provider
            .execute(
                TOOL_TOPIC_INFRA_UPSERT,
                r#"{"topic_id":"n-ru1","target_name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none","allowed_tool_modes":["exec"]}"#,
                None,
                None,
            )
            .await
            .expect("alias resolution should canonicalize forum topic name");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be valid json");
        assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:240");
    }

    #[tokio::test]
    async fn forum_topic_provision_ssh_agent_creates_canonical_binding_and_infra() {
        let lifecycle = Arc::new(FakeTopicLifecycle::new());
        let provider = ManagerControlPlaneProvider::new(
            Arc::new(mock_storage_for_forum_topic_provision()),
            77,
        )
        .with_topic_lifecycle(lifecycle.clone());
        let response = provider
            .execute(
                TOOL_FORUM_TOPIC_PROVISION_SSH_AGENT,
                r#"{"name":"n-ru1","host":"213.171.27.211","port":31924,"remote_user":"user1","auth_mode":"none"}"#,
                None,
                None,
            )
            .await
            .expect("atomic ssh topic provisioning should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be valid json");
        assert_eq!(parsed["topic"]["topic_id"], "-100777:313");
        assert_eq!(parsed["binding"]["topic_id"], "-100777:313");
        assert_eq!(parsed["topic_infra"]["topic_id"], "-100777:313");
        assert_eq!(parsed["preflight"]["provider_enabled"], true);

        let calls = lifecycle.calls();
        assert!(matches!(calls.first(), Some(LifecycleCall::Create(_))));
    }

    #[tokio::test]
    async fn topic_binding_set_persists_and_audits() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
                    && options.binding_kind.is_none()
                    && options.chat_id == OptionalMetadataPatch::Keep
                    && options.thread_id == OptionalMetadataPatch::Keep
                    && options.expires_at == OptionalMetadataPatch::Keep
                    && options.last_activity_at.is_none()
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 100,
                    updated_at: 200,
                })
            });

        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_SET
                    && options.topic_id.as_deref() == Some("topic-a")
                    && options.agent_id.as_deref() == Some("agent-a")
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-1".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 300,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic binding set should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(parsed["binding"]["topic_id"], "topic-a");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_set_supports_explicit_null_to_clear_metadata() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: 77,
                    topic_id: "topic-a".to_string(),
                    agent_id: "agent-a".to_string(),
                    binding_kind: TopicBindingKind::Runtime,
                    chat_id: Some(100),
                    thread_id: Some(7),
                    expires_at: Some(10_000),
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
                    && options.chat_id == OptionalMetadataPatch::Clear
                    && options.thread_id == OptionalMetadataPatch::Clear
                    && options.expires_at == OptionalMetadataPatch::Clear
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 10,
                    updated_at: 30,
                })
            });

        mock.expect_append_audit_event().returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                schema_version: 1,
                version: 1,
                event_id: "evt-1".to_string(),
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                created_at: 300,
            })
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","chat_id":null,"thread_id":null,"expires_at":null}"#,
                None,
                None,
            )
            .await
            .expect("topic binding set should support null clears");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed.get("ok"), Some(&serde_json::Value::Bool(true)));
        assert_eq!(parsed["binding"]["chat_id"], serde_json::Value::Null);
        assert_eq!(parsed["binding"]["thread_id"], serde_json::Value::Null);
        assert_eq!(parsed["binding"]["expires_at"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn topic_binding_set_succeeds_when_audit_write_fails() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));

        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-a"
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 100,
                    updated_at: 100,
                })
            });

        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("mutation should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["binding"]["topic_id"], "topic-a");
        assert_eq!(parsed["audit_status"], "write_failed");
        assert!(parsed["audit_error"].as_str().is_some());
    }

    #[tokio::test]
    async fn topic_binding_set_dry_run_does_not_persist() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_SET
                    && options.payload.get("outcome") == Some(&json!("dry_run"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-dry-run".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 300,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run set should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["preview"]["operation"], "upsert");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_set_dry_run_reports_audit_write_failure() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_SET,
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "write_failed");
    }

    #[tokio::test]
    async fn topic_binding_rollback_restores_previous_snapshot() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 4))));
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 9,
                    event_id: "evt-9".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-new".to_string()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "agent_id": "agent-new",
                        "previous": {
                            "schema_version": 1,
                            "version": 3,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 100,
                }])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-old"
            })
            .returning(|options| {
                Ok(binding(
                    options.user_id,
                    &options.topic_id,
                    &options.agent_id,
                    5,
                ))
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_BINDING_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 10,
                    event_id: "evt-10".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 110,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-old");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_binding_rollback_succeeds_when_audit_write_fails() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 4,
                    user_id: 77,
                    topic_id,
                    agent_id: "agent-new".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 9,
                    event_id: "evt-9".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-new".to_string()),
                    action: TOOL_TOPIC_BINDING_SET.to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 3,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 100,
                }])
            });
        mock.expect_upsert_topic_binding().returning(|options| {
            Ok(TopicBindingRecord {
                schema_version: 1,
                version: 5,
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                chat_id: options.chat_id.for_new_record(),
                thread_id: options.thread_id.for_new_record(),
                expires_at: options.expires_at.for_new_record(),
                last_activity_at: options.last_activity_at,
                created_at: 10,
                updated_at: 30,
            })
        });
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback should succeed even when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["audit_status"], "write_failed");
    }

    #[tokio::test]
    async fn topic_binding_rollback_scans_multiple_audit_pages() {
        let mut mock = crate::storage::MockStorageProvider::new();
        let mut sequence = Sequence::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| Ok(Some(binding(77, &topic_id, "agent-new", 8))));
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _, _| {
                Ok(vec![audit_event(
                    500,
                    Some("other-topic"),
                    Some("agent-z"),
                    TOOL_TOPIC_BINDING_SET,
                    json!({"outcome":"applied"}),
                )])
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(Some(500_u64)), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _, _| {
                Ok(vec![audit_event(
                    499,
                    Some("topic-a"),
                    Some("agent-new"),
                    TOOL_TOPIC_BINDING_SET,
                    json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 7,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-old",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                )])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-old"
            })
            .returning(|options| {
                Ok(binding(
                    options.user_id,
                    &options.topic_id,
                    &options.agent_id,
                    9,
                ))
            });
        mock.expect_append_audit_event().returning(|options| {
            Ok(crate::storage::AuditEventRecord {
                user_id: options.user_id,
                topic_id: options.topic_id,
                agent_id: options.agent_id,
                action: options.action,
                payload: options.payload,
                ..audit_event(501, None, None, TOOL_TOPIC_BINDING_ROLLBACK, json!({}))
            })
        });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_BINDING_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback should search across audit pages");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-old");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn agent_profile_rollback_deletes_when_previous_snapshot_absent() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_agent_profile()
            .with(eq(77_i64), eq("agent-a".to_string()))
            .returning(|_, agent_id| {
                Ok(Some(AgentProfileRecord {
                    schema_version: 1,
                    version: 2,
                    user_id: 77,
                    agent_id,
                    profile: json!({"mode":"current"}),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 3,
                    event_id: "evt-3".to_string(),
                    user_id: 77,
                    topic_id: None,
                    agent_id: Some("agent-a".to_string()),
                    action: TOOL_AGENT_PROFILE_DELETE.to_string(),
                    payload: json!({"agent_id":"agent-a","previous":null,"outcome":"applied"}),
                    created_at: 30,
                }])
            });
        mock.expect_delete_agent_profile()
            .with(eq(77_i64), eq("agent-a".to_string()))
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_AGENT_PROFILE_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("delete"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 40,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_AGENT_PROFILE_ROLLBACK,
                r#"{"agent_id":"agent-a"}"#,
                None,
                None,
            )
            .await
            .expect("agent rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "delete");
        assert!(parsed["profile"].is_null());
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_context_upsert_persists_and_audits() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_context()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_context()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.context == "Use maintenance window rules"
            })
            .returning(|options| {
                Ok(TopicContextRecord {
                    schema_version: 1,
                    version: 1,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    context: options.context,
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.topic_id.as_deref() == Some("topic-a")
                    && options.action == TOOL_TOPIC_CONTEXT_UPSERT
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 1,
                    event_id: "evt-1".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 11,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_CONTEXT_UPSERT,
                r#"{"topic_id":"topic-a","context":"Use maintenance window rules"}"#,
                None,
                None,
            )
            .await
            .expect("topic context upsert should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["topic_context"]["topic_id"], "topic-a");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_context_get_reports_missing_record() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_context()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_CONTEXT_GET,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic context get should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["found"], false);
        assert!(parsed["topic_context"].is_null());
    }

    #[tokio::test]
    async fn topic_context_rollback_restores_previous_snapshot() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_context()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| {
                Ok(Some(TopicContextRecord {
                    schema_version: 1,
                    version: 3,
                    user_id: 77,
                    topic_id,
                    context: "current context".to_string(),
                    created_at: 10,
                    updated_at: 30,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 2,
                    event_id: "evt-2".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: None,
                    action: TOOL_TOPIC_CONTEXT_UPSERT.to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 1,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "context": "previous context",
                            "created_at": 5,
                            "updated_at": 6
                        },
                        "outcome": "applied"
                    }),
                    created_at: 20,
                }])
            });
        mock.expect_upsert_topic_context()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.context == "previous context"
            })
            .returning(|options| {
                Ok(TopicContextRecord {
                    schema_version: 1,
                    version: 4,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    context: options.context,
                    created_at: 5,
                    updated_at: 40,
                })
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_CONTEXT_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| {
                Ok(crate::storage::AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    action: options.action,
                    payload: options.payload,
                    created_at: 50,
                })
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_CONTEXT_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic context rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["topic_context"]["context"], "previous context");
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_infra_upsert_persists_and_audits() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_infra_config()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_get_secret_value().returning(|_, _| Ok(None));
        mock.expect_upsert_topic_infra_config()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.target_name == "prod-app"
                    && options.host == "prod.example.com"
                    && options.remote_user == "deploy"
            })
            .returning(|options| {
                Ok(TopicInfraConfigRecord {
                    schema_version: 1,
                    version: 1,
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
                    created_at: 10,
                    updated_at: 10,
                })
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.topic_id.as_deref() == Some("topic-a")
                    && options.action == TOOL_TOPIC_INFRA_UPSERT
            })
            .returning(|options| {
                Ok(audit_event(
                    1,
                    options.topic_id.as_deref(),
                    None,
                    &options.action,
                    options.payload,
                ))
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_INFRA_UPSERT,
                r#"{"topic_id":"topic-a","target_name":"prod-app","host":"prod.example.com","remote_user":"deploy","auth_mode":"private_key","secret_ref":"storage:ssh/prod-key","sudo_secret_ref":"storage:ssh/prod-sudo","environment":"prod","tags":["prod"],"allowed_tool_modes":["exec","read_file"],"approval_required_modes":["sudo_exec"]}"#,
                None,
                None,
            )
            .await
            .expect("topic infra upsert should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["topic_infra"]["topic_id"], "topic-a");
        assert_eq!(parsed["topic_infra"]["target_name"], "prod-app");
        assert_eq!(parsed["preflight"]["provider_enabled"], false);
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn topic_infra_rollback_restores_previous_snapshot() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_topic_infra_config()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, topic_id| Ok(Some(topic_infra(77, &topic_id, 3))));
        mock.expect_get_secret_value().returning(|_, _| Ok(None));
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(ROLLBACK_AUDIT_PAGE_SIZE))
            .returning(|_, _, _| {
                Ok(vec![audit_event(
                    2,
                    Some("topic-a"),
                    None,
                    TOOL_TOPIC_INFRA_UPSERT,
                    json!({
                        "topic_id": "topic-a",
                        "previous": topic_infra(77, "topic-a", 1),
                        "outcome": "applied"
                    }),
                )])
            });
        mock.expect_upsert_topic_infra_config()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.target_name == "prod-app"
                    && options.host == "prod.example.com"
            })
            .returning(|options| {
                Ok(TopicInfraConfigRecord {
                    schema_version: 1,
                    version: 4,
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
                    created_at: 10,
                    updated_at: 40,
                })
            });
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == TOOL_TOPIC_INFRA_ROLLBACK
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| {
                Ok(audit_event(
                    4,
                    options.topic_id.as_deref(),
                    None,
                    &options.action,
                    options.payload,
                ))
            });

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_TOPIC_INFRA_ROLLBACK,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("topic infra rollback should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["topic_infra"]["host"], "prod.example.com");
        assert_eq!(parsed["preflight"]["provider_enabled"], false);
        assert_eq!(parsed["audit_status"], "written");
    }

    #[tokio::test]
    async fn private_secret_probe_reports_presence_without_exposing_content() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_secret_value()
            .with(eq(77_i64), eq("vds".to_string()))
            .returning(|_, _| Ok(Some("-----BEGIN OPENSSH PRIVATE KEY-----\ninvalid\n-----END OPENSSH PRIVATE KEY-----\n".to_string())));

        let provider = ManagerControlPlaneProvider::new(Arc::new(mock), 77);
        let response = provider
            .execute(
                TOOL_PRIVATE_SECRET_PROBE,
                r#"{"secret_ref":"storage:vds","kind":"ssh_private_key"}"#,
                None,
                None,
            )
            .await
            .expect("private secret probe should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["secret_probe"]["secret_ref"], "storage:vds");
        assert_eq!(parsed["secret_probe"]["present"], true);
        assert!(parsed["secret_probe"].get("content").is_none());
    }

    #[tokio::test]
    async fn tool_registry_routes_to_manager_provider() {
        let mut mock = crate::storage::MockStorageProvider::new();
        mock.expect_get_agent_profile()
            .with(eq(77_i64), eq("agent-x".to_string()))
            .returning(|user_id, agent_id| {
                Ok(Some(AgentProfileRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    agent_id,
                    profile: json!({"role":"support"}),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(ManagerControlPlaneProvider::new(
            Arc::new(mock),
            77,
        )));

        let response = registry
            .execute(
                TOOL_AGENT_PROFILE_GET,
                r#"{"agent_id":"agent-x"}"#,
                None,
                None,
            )
            .await
            .expect("registry execution should succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("response must be json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["profile"]["agent_id"], "agent-x");
    }

    #[tokio::test]
    async fn tool_registry_without_manager_provider_rejects_manager_tools() {
        let registry = ToolRegistry::new();
        let err = registry
            .execute(
                TOOL_TOPIC_BINDING_GET,
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect_err("manager tools must be unavailable without provider");

        assert!(err.to_string().contains("Unknown tool"));
    }
}
