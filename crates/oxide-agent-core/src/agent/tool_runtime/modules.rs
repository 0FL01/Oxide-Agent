//! Capability-oriented tool modules.

use super::ToolExecutor;
#[cfg(oxide_module_tool_webfetch_md)]
use super::arguments::deserialize_optional_unsigned;
use super::surface::{CapabilityGroup, ToolSurfaceHandle, ToolVisibility};
#[cfg(oxide_module_tool_webfetch_md)]
use super::{
    OutputNormalizer, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig, ToolRuntimeError,
};
use crate::agent::progress::AgentEvent;
#[cfg(oxide_module_tool_sandbox_exec)]
use crate::agent::providers::SandboxExecProvider;
#[cfg(oxide_module_tool_sandbox_fileops)]
use crate::agent::providers::SandboxFileOpsProvider;
#[cfg(oxide_module_tool_sandbox_recreate)]
use crate::agent::providers::SandboxLifecycleProvider;
use crate::agent::providers::{SandboxRuntime, TodoList};
use crate::capabilities::ModuleId;
use crate::config::AgentSettings;
use crate::llm::LlmClient;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::llm::ToolDefinition;
#[cfg(oxide_module_tool_browser_live)]
use crate::sandbox::SandboxFileOps;
use crate::sandbox::SandboxScope;
use async_trait::async_trait;
#[cfg(oxide_module_tool_webfetch_md)]
use serde::Deserialize;
#[cfg(oxide_module_tool_webfetch_md)]
use serde_json::{Value, json};
use std::sync::Arc;
#[cfg(oxide_module_integration_ssh_mcp)]
use std::sync::OnceLock;
use tokio::sync::{Mutex, mpsc::Sender};

#[cfg(oxide_module_tool_agents_md)]
use crate::agent::providers::AgentsMdProvider;
#[cfg(oxide_module_tool_compression)]
use crate::agent::providers::CompressionProvider;
#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
use crate::agent::providers::CrwScrapeClient;
#[cfg(oxide_module_tool_delegation)]
use crate::agent::providers::DelegationProvider;
#[cfg(oxide_module_tool_file_delivery)]
use crate::agent::providers::FileHosterProvider;
#[cfg(oxide_module_manager_control_plane)]
use crate::agent::providers::ManagerControlPlaneProvider;
use crate::agent::providers::ManagerTopicLifecycle;
#[cfg(any(
    oxide_module_tool_audio_stt,
    oxide_module_tool_vision_image,
    oxide_module_tool_vision_video
))]
use crate::agent::providers::MediaFileProvider;
use crate::agent::providers::ReminderContext;
#[cfg(oxide_module_tool_reminder)]
use crate::agent::providers::ReminderProvider;
#[cfg(oxide_module_integration_ssh_mcp)]
use crate::agent::providers::SshMcpProvider;
#[cfg(oxide_module_tool_stack_logs)]
use crate::agent::providers::StackLogsProvider;
#[cfg(oxide_module_tool_todos)]
use crate::agent::providers::TodosProvider;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::WebFetchMdProvider;
#[cfg(oxide_module_tool_web_search)]
use crate::agent::providers::WebSearchProvider;
#[cfg(oxide_module_tool_ytdlp)]
use crate::agent::providers::YtdlpProvider;
#[cfg(oxide_module_integration_ssh_mcp)]
use crate::agent::providers::ssh_mcp::cleanup_stale_private_key_tempfiles;
#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
use crate::agent::providers::webfetch_md::FetchedMarkdownDocument;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::webfetch_md::WebMarkdownArgs;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::webfetch_md::{
    DeliveryPayloadExtra, DeliveryStdoutExtra, MarkdownReadMode, delivery_success_payload,
    no_cached_document_message, no_cached_document_payload, parse_read_mode,
    render_delivery_stdout, require_url, resolve_output_window,
};
#[cfg(oxide_module_tool_browser_live)]
use crate::agent::providers::{BrowserArtifactSettings, BrowserLiveProvider};
#[cfg(oxide_module_integration_mcp_jira)]
use crate::agent::providers::{JiraMcpConfig, JiraMcpProvider};
#[cfg(oxide_module_tool_tts_kokoro)]
use crate::agent::providers::{KokoroTtsProvider, TtsConfig};
#[cfg(oxide_module_integration_mcp_mattermost)]
use crate::agent::providers::{MattermostMcpConfig, MattermostMcpProvider};
#[cfg(oxide_module_tool_tts_silero)]
use crate::agent::providers::{SileroTtsConfig, SileroTtsProvider};
use crate::storage::StorageProvider;
use crate::storage::TopicInfraConfigRecord;

/// Topic-scoped context required by the AGENTS.md tools.
#[derive(Clone)]
#[cfg_attr(not(oxide_module_tool_agents_md), allow(dead_code))]
pub struct AgentsMdModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
}

#[cfg_attr(not(oxide_module_tool_agents_md), allow(dead_code))]
impl AgentsMdModuleContext {
    /// Create a context for topic-scoped AGENTS.md tools.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, user_id: i64, topic_id: String) -> Self {
        Self {
            storage,
            user_id,
            topic_id,
        }
    }
}

/// User-scoped context required by manager control-plane tools.
#[derive(Clone)]
#[cfg_attr(not(oxide_module_manager_control_plane), allow(dead_code))]
pub struct ManagerControlPlaneModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

#[cfg_attr(not(oxide_module_manager_control_plane), allow(dead_code))]
impl ManagerControlPlaneModuleContext {
    /// Create a context for manager control-plane tools.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
    ) -> Self {
        Self {
            storage,
            user_id,
            topic_lifecycle,
        }
    }
}

/// Topic-scoped infrastructure context required by SSH MCP tools.
#[derive(Clone)]
#[cfg_attr(not(oxide_module_integration_ssh_mcp), allow(dead_code))]
pub struct SshMcpModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: TopicInfraConfigRecord,
}

#[cfg_attr(not(oxide_module_integration_ssh_mcp), allow(dead_code))]
impl SshMcpModuleContext {
    /// Create a context for topic-scoped SSH MCP tools.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: TopicInfraConfigRecord,
    ) -> Self {
        Self {
            storage,
            user_id,
            topic_id,
            config,
        }
    }
}

/// RAII cleanup contract for browser session lifecycle.
///
/// Implemented by `BrowserLiveProvider` when the browser-live module is
/// compiled. Held by sub-agent execution to ensure all browser sessions
/// are closed when the sub-agent ends (success, timeout, cancel, or error),
/// preventing Chromium process leaks at the sidecar.
#[cfg_attr(not(oxide_module_tool_browser_live), allow(dead_code))]
#[async_trait]
pub trait BrowserSessionCleanup: Send + Sync {
    /// Close all browser sessions tracked by this provider.
    async fn close_all_sessions(&self);
}

/// Context required by browser-live tools: durable storage for screenshot
/// artifacts and transport-agnostic session scope for deletion.
#[derive(Clone)]
#[cfg_attr(not(oxide_module_tool_browser_live), allow(dead_code))]
pub struct BrowserLiveModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
}

#[cfg_attr(not(oxide_module_tool_browser_live), allow(dead_code))]
impl BrowserLiveModuleContext {
    /// Create a context for browser-live screenshot storage.
    #[must_use]
    pub fn new(storage: Arc<dyn StorageProvider>, user_id: i64, context_key: String) -> Self {
        Self {
            storage,
            user_id,
            context_key,
        }
    }

    /// Durable storage handle for saving/loading browser artifacts.
    #[must_use]
    pub fn storage(&self) -> Arc<dyn StorageProvider> {
        Arc::clone(&self.storage)
    }

    /// Owning user ID.
    #[must_use]
    pub const fn user_id(&self) -> i64 {
        self.user_id
    }

    /// Transport-agnostic session identifier (from `AgentMemoryScope`).
    #[must_use]
    pub fn context_key(&self) -> &str {
        &self.context_key
    }
}

/// Runtime context passed to tool capability modules.
pub struct ToolModuleContext {
    todos: Arc<Mutex<TodoList>>,
    sandbox_scope: SandboxScope,
    sandbox_runtime: Arc<SandboxRuntime>,
    llm_client: Arc<LlmClient>,
    settings: Arc<AgentSettings>,
    agents_md_context: Option<AgentsMdModuleContext>,
    manager_control_plane_context: Option<ManagerControlPlaneModuleContext>,
    ssh_mcp_context: Option<SshMcpModuleContext>,
    browser_live_context: Option<BrowserLiveModuleContext>,
    reminder_context: Option<ReminderContext>,
    progress_tx: Option<Sender<AgentEvent>>,
    inherited_model: Option<crate::config::ModelInfo>,
    /// Shared tool surface handle for the lazy tool protocol.
    ///
    /// Created at run start, shared between the runner (reads visible specs)
    /// and the `retrieve_tools` executor (activates groups).  The group→tools
    /// mapping is populated during module registration.
    tool_surface_handle: Arc<ToolSurfaceHandle>,
}

/// Constructor arguments for [`ToolModuleContext`].
pub struct ToolModuleContextParts {
    /// Shared todo list state.
    pub todos: Arc<Mutex<TodoList>>,
    /// Current sandbox scope.
    pub sandbox_scope: SandboxScope,
    /// Shared sandbox runtime.
    pub sandbox_runtime: Arc<SandboxRuntime>,
    /// Shared LLM client.
    pub llm_client: Arc<LlmClient>,
    /// Shared agent settings.
    pub settings: Arc<AgentSettings>,
    /// Optional AGENTS.md context.
    pub agents_md_context: Option<AgentsMdModuleContext>,
    /// Optional manager control-plane context.
    pub manager_control_plane_context: Option<ManagerControlPlaneModuleContext>,
    /// Optional topic infrastructure context for SSH MCP tools.
    pub ssh_mcp_context: Option<SshMcpModuleContext>,
    /// Optional browser-live context for screenshot storage.
    pub browser_live_context: Option<BrowserLiveModuleContext>,
    /// Optional reminder context.
    pub reminder_context: Option<ReminderContext>,
    /// Optional progress sender.
    pub progress_tx: Option<Sender<AgentEvent>>,
    /// Parent session's effective model, inherited by sub-agents when no
    /// explicit sub-agent model is configured. `None` when no per-session
    /// override is active (e.g. Telegram, or web sessions using the bootstrap
    /// default).
    pub inherited_model: Option<crate::config::ModelInfo>,
    /// Shared tool surface handle for the lazy tool protocol.
    pub tool_surface_handle: Arc<ToolSurfaceHandle>,
}

impl ToolModuleContext {
    /// Creates a tool module context.
    #[must_use]
    pub fn new(parts: ToolModuleContextParts) -> Self {
        Self {
            todos: parts.todos,
            sandbox_scope: parts.sandbox_scope,
            sandbox_runtime: parts.sandbox_runtime,
            llm_client: parts.llm_client,
            settings: parts.settings,
            agents_md_context: parts.agents_md_context,
            manager_control_plane_context: parts.manager_control_plane_context,
            ssh_mcp_context: parts.ssh_mcp_context,
            browser_live_context: parts.browser_live_context,
            reminder_context: parts.reminder_context,
            progress_tx: parts.progress_tx,
            inherited_model: parts.inherited_model,
            tool_surface_handle: parts.tool_surface_handle,
        }
    }

    /// Shared todo list state for modules that own todo tools.
    #[must_use]
    pub fn todos(&self) -> Arc<Mutex<TodoList>> {
        Arc::clone(&self.todos)
    }

    /// Shared sandbox runtime for modules that own sandbox tools.
    #[must_use]
    pub fn sandbox_runtime(&self) -> Arc<SandboxRuntime> {
        Arc::clone(&self.sandbox_runtime)
    }

    /// Sandbox scope for modules that need their own sandbox-backed provider.
    #[must_use]
    pub fn sandbox_scope(&self) -> SandboxScope {
        self.sandbox_scope.clone()
    }

    /// Shared LLM client for modules that call model-side media APIs.
    #[must_use]
    pub fn llm_client(&self) -> Arc<LlmClient> {
        Arc::clone(&self.llm_client)
    }

    /// Shared agent settings for modules that need runtime policy/config access.
    #[must_use]
    pub fn settings(&self) -> Arc<AgentSettings> {
        Arc::clone(&self.settings)
    }

    /// Optional context for topic-scoped AGENTS.md tools.
    #[cfg_attr(not(oxide_module_tool_agents_md), allow(dead_code))]
    #[must_use]
    pub fn agents_md_context(&self) -> Option<AgentsMdModuleContext> {
        self.agents_md_context.clone()
    }

    /// Optional context for manager control-plane tools.
    #[cfg_attr(not(oxide_module_manager_control_plane), allow(dead_code))]
    #[must_use]
    pub fn manager_control_plane_context(&self) -> Option<ManagerControlPlaneModuleContext> {
        self.manager_control_plane_context.clone()
    }

    /// Optional context for topic-scoped SSH MCP tools.
    #[cfg_attr(not(oxide_module_integration_ssh_mcp), allow(dead_code))]
    #[must_use]
    pub fn ssh_mcp_context(&self) -> Option<SshMcpModuleContext> {
        self.ssh_mcp_context.clone()
    }

    /// Optional context for browser-live screenshot storage.
    #[cfg_attr(not(oxide_module_tool_browser_live), allow(dead_code))]
    #[must_use]
    pub fn browser_live_context(&self) -> Option<BrowserLiveModuleContext> {
        self.browser_live_context.clone()
    }

    /// Optional context for reminder tools.
    #[cfg_attr(not(oxide_module_tool_reminder), allow(dead_code))]
    #[must_use]
    pub fn reminder_context(&self) -> Option<ReminderContext> {
        self.reminder_context.clone()
    }

    /// Optional progress sender for modules that emit progress events.
    #[must_use]
    pub fn progress_tx(&self) -> Option<Sender<AgentEvent>> {
        self.progress_tx.clone()
    }

    /// Parent session's effective model for sub-agent inheritance.
    ///
    /// Returns the per-execution model override (e.g. from a web UI model
    /// selection) that sub-agents should inherit when no explicit sub-agent
    /// model is configured. `None` when no override is active.
    #[cfg_attr(not(oxide_module_tool_delegation), allow(dead_code))]
    #[must_use]
    pub fn inherited_model(&self) -> Option<crate::config::ModelInfo> {
        self.inherited_model.clone()
    }

    /// Shared tool surface handle for the lazy tool protocol.
    ///
    /// Used by the `retrieve_tools` executor to activate capability groups
    /// and by the runner to compute the model-visible tool surface.
    #[must_use]
    pub fn tool_surface_handle(&self) -> Arc<ToolSurfaceHandle> {
        Arc::clone(&self.tool_surface_handle)
    }
}

/// Tool capability module.
pub trait ToolModule {
    /// Stable module ID corresponding to the compiled capability manifest.
    fn module_id(&self) -> ModuleId;

    /// Capability group for deferred tool activation.
    ///
    /// Returns `None` for always-visible bootstrap tools (e.g. `retrieve_tools`,
    /// `write_todos`, `compress`), `Some(group)` for deferred tools that are
    /// activated via `retrieve_tools`.
    fn capability_group(&self) -> Option<CapabilityGroup>;

    /// Whether this module's tools are always visible or deferred.
    fn visibility(&self) -> ToolVisibility;

    /// Builds typed tool executors owned by this module.
    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>>;
}

/// Capability module for Browser Live autonomous browser tools.
#[cfg(oxide_module_tool_browser_live)]
pub struct BrowserLiveToolModule;

#[cfg(oxide_module_tool_browser_live)]
impl BrowserLiveToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<BrowserLiveProvider> {
        let settings = ctx.settings();
        let browser = settings.get_browser_agent_settings();
        if !browser.enabled {
            return None;
        }
        let base_url = browser.sidecar_base_url.as_deref()?;
        let token = browser.sidecar_token.as_deref()?;
        let live_ctx = ctx.browser_live_context()?;
        let fileops: Arc<dyn SandboxFileOps> = ctx.sandbox_runtime();
        BrowserLiveProvider::from_sidecar_config(
            base_url,
            token,
            BrowserArtifactSettings::default(),
            ctx.progress_tx(),
            live_ctx.storage(),
            live_ctx.user_id(),
            live_ctx.context_key().to_string(),
            Some(fileops),
        )
        .ok()
    }

    /// Build a shared browser-live provider wrapped in `Arc`.
    ///
    /// Unlike `tool_runtime_executors`, this exposes the `Arc<BrowserLiveProvider>`
    /// so callers (e.g. sub-agent delegation) can hold it for RAII cleanup via
    /// [`BrowserSessionCleanup::close_all_sessions`].
    #[must_use]
    pub fn shared_provider(&self, ctx: &ToolModuleContext) -> Option<Arc<BrowserLiveProvider>> {
        self.provider(ctx).map(Arc::new)
    }
}

#[cfg(oxide_module_tool_browser_live)]
impl ToolModule for BrowserLiveToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/browser-live")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Browser)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for the runner-handled `compress` tool.
#[cfg(oxide_module_tool_compression)]
pub struct CompressionToolModule;

#[cfg(oxide_module_tool_compression)]
impl ToolModule for CompressionToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/compression")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        None
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::AlwaysVisible
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(CompressionProvider::new()).tool_runtime_executors()
    }
}

/// Capability module for chat and external file delivery from sandbox files.
#[cfg(oxide_module_tool_file_delivery)]
pub struct FileDeliveryToolModule;

#[cfg(oxide_module_tool_file_delivery)]
impl ToolModule for FileDeliveryToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/file-delivery")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Files)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(FileHosterProvider::from_runtime(ctx.sandbox_runtime()))
            .tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for topic-scoped AGENTS.md self-editing tools.
#[cfg(oxide_module_tool_agents_md)]
pub struct AgentsMdToolModule;

#[cfg(oxide_module_tool_agents_md)]
impl AgentsMdToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<AgentsMdProvider> {
        ctx.agents_md_context().map(|agents_md| {
            AgentsMdProvider::new(agents_md.storage, agents_md.user_id, agents_md.topic_id)
        })
    }
}

#[cfg(oxide_module_tool_agents_md)]
impl ToolModule for AgentsMdToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/agents-md")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::AgentsMd)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for sub-agent delegation tools.
#[cfg(oxide_module_tool_delegation)]
pub struct DelegationToolModule;

#[cfg(oxide_module_tool_delegation)]
impl DelegationToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> DelegationProvider {
        let provider =
            DelegationProvider::new(ctx.llm_client(), ctx.sandbox_scope(), ctx.settings());

        #[cfg(oxide_module_tool_agents_md)]
        let provider = if let Some(agents_md) = ctx.agents_md_context() {
            provider.with_topic_agents_md_context(
                agents_md.storage,
                agents_md.user_id,
                agents_md.topic_id,
            )
        } else {
            provider
        };

        #[cfg(oxide_module_tool_browser_live)]
        let provider = provider.with_browser_live_context(ctx.browser_live_context());

        provider.with_inherited_model(ctx.inherited_model())
    }
}

#[cfg(oxide_module_tool_delegation)]
impl ToolModule for DelegationToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/delegation")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Delegation)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(self.provider(ctx)).tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for manager control-plane tools.
#[cfg(oxide_module_manager_control_plane)]
pub struct ManagerControlPlaneToolModule;

#[cfg(oxide_module_manager_control_plane)]
impl ManagerControlPlaneToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<ManagerControlPlaneProvider> {
        let manager = ctx.manager_control_plane_context()?;
        let mut provider = ManagerControlPlaneProvider::new(manager.storage, manager.user_id);
        if let Some(topic_lifecycle) = manager.topic_lifecycle {
            provider = provider.with_topic_lifecycle(topic_lifecycle);
        }
        Some(provider)
    }
}

#[cfg(oxide_module_manager_control_plane)]
impl ToolModule for ManagerControlPlaneToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("manager/control-plane")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Manager)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for topic-scoped SSH MCP tools.
#[cfg(oxide_module_integration_ssh_mcp)]
pub struct SshMcpToolModule;

#[cfg(oxide_module_integration_ssh_mcp)]
static SSH_PRIVATE_KEY_CLEANUP_RESULT: OnceLock<Result<usize, String>> = OnceLock::new();

#[cfg(oxide_module_integration_ssh_mcp)]
impl SshMcpToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<SshMcpProvider> {
        let ssh = ctx.ssh_mcp_context()?;
        self.cleanup_stale_private_key_tempfiles_once();
        Some(SshMcpProvider::new(
            ssh.storage,
            ssh.user_id,
            ssh.topic_id,
            ssh.config,
        ))
    }

    fn cleanup_stale_private_key_tempfiles_once(&self) {
        let result = SSH_PRIVATE_KEY_CLEANUP_RESULT.get_or_init(|| {
            cleanup_stale_private_key_tempfiles().map_err(|error| error.to_string())
        });
        match result {
            Ok(removed) if *removed > 0 => {
                tracing::info!(removed, "Removed stale SSH private key temp files");
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "Failed to clean up stale SSH private key temp files");
            }
        }
    }
}

#[cfg(oxide_module_integration_ssh_mcp)]
impl ToolModule for SshMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/ssh-mcp")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Ssh)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors(ctx.progress_tx()))
            .unwrap_or_default()
    }
}

/// Capability module for reminder scheduling tools.
#[cfg(oxide_module_tool_reminder)]
pub struct ReminderToolModule;

#[cfg(oxide_module_tool_reminder)]
impl ReminderToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<ReminderProvider> {
        ctx.reminder_context().map(ReminderProvider::new)
    }
}

#[cfg(oxide_module_tool_reminder)]
impl ToolModule for ReminderToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/reminder")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Reminders)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

#[cfg(any(
    oxide_module_tool_audio_stt,
    oxide_module_tool_vision_image,
    oxide_module_tool_vision_video
))]
fn media_file_provider(ctx: &ToolModuleContext) -> MediaFileProvider {
    let audio_transcriber =
        crate::audio_stt::build_audio_transcriber(&ctx.settings().audio_stt_config());
    match ctx.browser_live_context() {
        Some(live_ctx) => MediaFileProvider::from_runtime_with_storage(
            ctx.llm_client(),
            audio_transcriber,
            ctx.sandbox_runtime(),
            live_ctx.storage(),
            live_ctx.user_id(),
        ),
        None => MediaFileProvider::from_runtime(
            ctx.llm_client(),
            audio_transcriber,
            ctx.sandbox_runtime(),
        ),
    }
}

/// Capability module for audio file transcription.
#[cfg(oxide_module_tool_audio_stt)]
pub struct AudioSttToolModule;

#[cfg(oxide_module_tool_audio_stt)]
impl ToolModule for AudioSttToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/audio-stt")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Media)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["transcribe_audio_file"])
    }
}

/// Capability module for image file description.
#[cfg(oxide_module_tool_vision_image)]
pub struct VisionImageToolModule;

#[cfg(oxide_module_tool_vision_image)]
impl ToolModule for VisionImageToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/vision-image")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Media)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["describe_image_file"])
    }
}

/// Capability module for video file description.
#[cfg(oxide_module_tool_vision_video)]
pub struct VisionVideoToolModule;

#[cfg(oxide_module_tool_vision_video)]
impl ToolModule for VisionVideoToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/vision-video")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Media)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["describe_video_file"])
    }
}

/// Capability module for Jira MCP tools.
#[cfg(oxide_module_integration_mcp_jira)]
pub struct JiraMcpToolModule;

#[cfg(oxide_module_integration_mcp_jira)]
impl JiraMcpToolModule {
    fn provider(&self) -> Option<JiraMcpProvider> {
        match JiraMcpConfig::from_env() {
            Some(config) => {
                let binary_path = config.binary_path.clone();
                tracing::debug!(
                    binary_path = %binary_path,
                    jira_url_present = !config.jira_url.is_empty(),
                    jira_email_present = !config.jira_email.is_empty(),
                    jira_token_present = !config.jira_token.is_empty(),
                    "Registering Jira MCP provider"
                );
                let provider = JiraMcpProvider::new(config);
                tracing::debug!(binary_path = %binary_path, "Jira MCP provider registered");
                Some(provider)
            }
            None => {
                tracing::warn!(
                    "jira feature is enabled but JIRA_URL, JIRA_EMAIL, or JIRA_API_TOKEN is not set; \
                     Jira MCP provider will not be available. Set these env vars to enable it."
                );
                None
            }
        }
    }
}

#[cfg(oxide_module_integration_mcp_jira)]
impl ToolModule for JiraMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-jira")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Jira)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Mattermost MCP tools.
#[cfg(oxide_module_integration_mcp_mattermost)]
pub struct MattermostMcpToolModule;

#[cfg(oxide_module_integration_mcp_mattermost)]
impl MattermostMcpToolModule {
    fn provider(&self) -> Option<MattermostMcpProvider> {
        match MattermostMcpConfig::from_env() {
            Some(config) => {
                let binary_path = config.binary_path.clone();
                tracing::debug!(
                    binary_path = %binary_path,
                    mattermost_url_present = !config.mattermost_url.is_empty(),
                    mattermost_token_present = !config.mattermost_token.is_empty(),
                    timeout_secs = config.timeout_secs,
                    max_retries = config.max_retries,
                    verify_ssl = config.verify_ssl,
                    "Registering Mattermost MCP provider"
                );
                let provider = MattermostMcpProvider::new(config);
                tracing::debug!(binary_path = %binary_path, "Mattermost MCP provider registered");
                Some(provider)
            }
            None => {
                tracing::warn!(
                    "mattermost feature is enabled but MATTERMOST_URL or MATTERMOST_TOKEN is not set; \
                     Mattermost MCP provider will not be available. Set these env vars to enable it."
                );
                None
            }
        }
    }
}

#[cfg(oxide_module_integration_mcp_mattermost)]
impl ToolModule for MattermostMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-mattermost")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Mattermost)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for compose-stack log tools.
#[cfg(oxide_module_tool_stack_logs)]
pub struct StackLogsToolModule;

#[cfg(oxide_module_tool_stack_logs)]
impl ToolModule for StackLogsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/stack-logs")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::StackLogs)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(StackLogsProvider::new()).tool_runtime_executors()
    }
}

/// Capability module for one-shot URL-to-Markdown fetches.
#[cfg(oxide_module_tool_webfetch_md)]
pub struct WebFetchMdToolModule;

#[cfg(oxide_module_tool_webfetch_md)]
impl ToolModule for WebFetchMdToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/webfetch-md")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Web)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        if crate::config::is_web_crawler_merge_enabled() {
            return Vec::new();
        }
        Arc::new(WebFetchMdProvider::new()).tool_runtime_executors()
    }
}

/// Capability module for merged URL-to-Markdown fetches.
#[cfg(oxide_module_tool_webfetch_md)]
pub struct WebCrawlerToolModule;

#[cfg(oxide_module_tool_webfetch_md)]
impl ToolModule for WebCrawlerToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/web-crawler")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Web)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        if !crate::config::is_web_crawler_merge_enabled() {
            return Vec::new();
        }
        vec![Arc::new(WebCrawlerToolExecutor::new())]
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
const TOOL_WEB_CRAWLER: &str = "web_crawler";
#[cfg(oxide_module_tool_webfetch_md)]
const WEB_CRAWLER_DEFAULT_WEBFETCH_TIMEOUT_SECS: u64 = 10;
#[cfg(oxide_module_tool_webfetch_md)]
const WEB_CRAWLER_DEFAULT_INLINE_CHARS: usize = 60_000;
#[cfg(oxide_module_tool_webfetch_md)]
const WEB_CRAWLER_MIN_INLINE_CHARS: usize = 1_000;
#[cfg(oxide_module_tool_webfetch_md)]
const WEB_CRAWLER_MAX_INLINE_CHARS: usize = 100_000;
#[cfg(oxide_module_tool_webfetch_md)]
const WEB_CRAWLER_DEFAULT_RENDER_WAIT_MS: u64 = 3000;

#[cfg(oxide_module_tool_webfetch_md)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Http,
    Lightpanda,
    Playwright,
}

#[cfg(oxide_module_tool_webfetch_md)]
impl RenderMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Lightpanda => "lightpanda",
            Self::Playwright => "playwright",
        }
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn parse_render_mode(value: Option<&str>) -> anyhow::Result<RenderMode> {
    match value.unwrap_or("http").trim() {
        "http" | "" => Ok(RenderMode::Http),
        "lightpanda" => Ok(RenderMode::Lightpanda),
        "playwright" => Ok(RenderMode::Playwright),
        other => anyhow::bail!(
            "invalid web_crawler render mode: {other} (expected http, lightpanda, or playwright)"
        ),
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
#[derive(Debug, Deserialize, Clone, Default)]
struct WebCrawlerArgs {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    read: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_unsigned")]
    timeout_secs: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_unsigned")]
    max_chars: Option<usize>,
    /// Render mode: `http` (default), `lightpanda`, or `playwright`.
    #[serde(default)]
    render: Option<String>,
    /// Milliseconds to wait after JS rendering for late content (rendered modes only).
    #[serde(default, deserialize_with = "deserialize_optional_unsigned")]
    render_wait_ms: Option<u64>,
}

#[cfg(oxide_module_tool_webfetch_md)]
struct WebCrawlerToolExecutor {
    webfetch: WebFetchMdProvider,
    #[cfg(oxide_module_tool_crw)]
    crw: Option<Arc<CrwScrapeClient>>,
    name: ToolName,
    spec: ToolDefinition,
}

#[cfg(oxide_module_tool_webfetch_md)]
impl WebCrawlerToolExecutor {
    fn new() -> Self {
        #[cfg(oxide_module_tool_crw)]
        let crw = CrwScrapeClient::new_from_env().ok().flatten().map(Arc::new);

        Self {
            webfetch: WebFetchMdProvider::new(),
            #[cfg(oxide_module_tool_crw)]
            crw,
            name: ToolName::from(TOOL_WEB_CRAWLER),
            spec: web_crawler_tool_definition(),
        }
    }

    async fn execute_crawler(
        &self,
        invocation: &ToolInvocation,
        args: WebCrawlerArgs,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        if parse_read_mode(args.read.as_deref(), TOOL_WEB_CRAWLER)? == MarkdownReadMode::Next {
            return self
                .execute_cached_next(invocation, &normalizer, &args)
                .await;
        }

        let url = require_url(args.url.as_deref(), TOOL_WEB_CRAWLER)?.to_string();
        let render_mode =
            parse_render_mode(args.render.as_deref()).map_err(web_crawler_runtime_error)?;

        match render_mode {
            RenderMode::Http => self.execute_http(invocation, &normalizer, &args, url).await,
            RenderMode::Lightpanda | RenderMode::Playwright => {
                self.execute_rendered(invocation, &normalizer, &args, url, render_mode)
                    .await
            }
        }
    }

    async fn execute_http(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: &WebCrawlerArgs,
        url: String,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let webfetch_args = WebMarkdownArgs {
            url: Some(url.clone()),
            read: None,
            timeout_secs: Some(web_crawler_webfetch_timeout_secs(args)),
            max_chars: None,
        };

        match self
            .webfetch
            .fetch_markdown_document(webfetch_args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(document) => {
                let output_window = resolve_output_window(
                    args.max_chars,
                    WEB_CRAWLER_DEFAULT_INLINE_CHARS,
                    WEB_CRAWLER_MIN_INLINE_CHARS,
                    WEB_CRAWLER_MAX_INLINE_CHARS,
                );
                let delivery = self
                    .webfetch
                    .store_markdown_window(
                        invocation.session_id.as_i64(),
                        url.clone(),
                        document,
                        output_window,
                    )
                    .await;
                let stdout = render_delivery_stdout(
                    TOOL_WEB_CRAWLER,
                    &delivery,
                    Some(&DeliveryStdoutExtra {
                        backend: Some("webfetch_md"),
                        render: Some("http"),
                    }),
                );
                let mut output = normalizer.success(invocation, &stdout, "");
                output.structured_payload = Some(delivery_success_payload(
                    TOOL_WEB_CRAWLER,
                    &delivery,
                    Some(&DeliveryPayloadExtra {
                        backend: Some("webfetch_md"),
                        render: Some("http"),
                        rendered_with: None,
                        status_code: None,
                        raw_payload: None,
                    }),
                ));
                Ok(output)
            }
            Err(webfetch_error) => {
                let message =
                    WebFetchMdProvider::failure_message(Some(&webfetch_args), &webfetch_error);
                let mut output = normalizer.failure(invocation, message);
                output.structured_payload = Some(web_crawler_webfetch_failure_payload(
                    Some(&webfetch_args),
                    &webfetch_error,
                ));
                Ok(output)
            }
        }
    }

    #[cfg(oxide_module_tool_crw)]
    async fn execute_rendered(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: &WebCrawlerArgs,
        url: String,
        render_mode: RenderMode,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        use crate::agent::providers::crw::CrwScrapeArgs;

        let crw = match &self.crw {
            Some(crw) => crw,
            None => {
                let mut output = normalizer.failure(
                    invocation,
                    web_crawler_render_unavailable_message(&url, render_mode.as_str()),
                );
                output.structured_payload = Some(web_crawler_render_unavailable_payload(
                    &url,
                    render_mode.as_str(),
                ));
                return Ok(output);
            }
        };

        let wait_for_ms = args
            .render_wait_ms
            .unwrap_or(WEB_CRAWLER_DEFAULT_RENDER_WAIT_MS);
        let scrape_args = CrwScrapeArgs {
            url: url.clone(),
            renderer: render_mode.as_str().to_string(),
            wait_for_ms,
        };

        match crw.scrape(&scrape_args).await {
            Ok(response) => {
                let final_url = response.data.metadata.url.as_deref().unwrap_or(&url);
                let status_code = response.data.metadata.status_code.map(u64::from);
                let rendered_with = response
                    .data
                    .metadata
                    .rendered_with
                    .as_deref()
                    .unwrap_or(render_mode.as_str());

                let document = FetchedMarkdownDocument {
                    metadata: vec![("URL".to_string(), final_url.to_string())],
                    fetched_bytes: None,
                    markdown: response.data.markdown.trim().to_string(),
                };
                let output_window = resolve_output_window(
                    args.max_chars,
                    WEB_CRAWLER_DEFAULT_INLINE_CHARS,
                    WEB_CRAWLER_MIN_INLINE_CHARS,
                    WEB_CRAWLER_MAX_INLINE_CHARS,
                );
                let delivery = self
                    .webfetch
                    .store_markdown_window(
                        invocation.session_id.as_i64(),
                        url,
                        document,
                        output_window,
                    )
                    .await;
                let stdout = render_delivery_stdout(
                    TOOL_WEB_CRAWLER,
                    &delivery,
                    Some(&DeliveryStdoutExtra {
                        backend: Some("crw_scrape"),
                        render: Some(render_mode.as_str()),
                    }),
                );
                let mut output = normalizer.success(invocation, &stdout, "");
                output.structured_payload = Some(delivery_success_payload(
                    TOOL_WEB_CRAWLER,
                    &delivery,
                    Some(&DeliveryPayloadExtra {
                        backend: Some("crw_scrape"),
                        render: Some(render_mode.as_str()),
                        rendered_with: Some(rendered_with),
                        status_code,
                        raw_payload: None,
                    }),
                ));
                Ok(output)
            }
            Err(crw_error) => {
                let crw_error_kind = crw_error.kind().to_string();
                let crw_error_message = crw_error.scrape_agent_message();
                let message = format!(
                    "web_crawler render:{} failed for {}: {}",
                    render_mode.as_str(),
                    url,
                    crw_error_message,
                );
                let mut output = normalizer.failure(invocation, message);
                output.structured_payload = Some(web_crawler_crw_failure_payload(
                    &url,
                    render_mode.as_str(),
                    &crw_error_kind,
                    &crw_error_message,
                ));
                Ok(output)
            }
        }
    }

    #[cfg(not(oxide_module_tool_crw))]
    async fn execute_rendered(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        _args: &WebCrawlerArgs,
        url: String,
        render_mode: RenderMode,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let mut output = normalizer.failure(
            invocation,
            web_crawler_render_unavailable_message(&url, render_mode.as_str()),
        );
        output.structured_payload = Some(web_crawler_render_unavailable_payload(
            &url,
            render_mode.as_str(),
        ));
        Ok(output)
    }

    async fn execute_cached_next(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: &WebCrawlerArgs,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let output_window = resolve_output_window(
            args.max_chars,
            WEB_CRAWLER_DEFAULT_INLINE_CHARS,
            WEB_CRAWLER_MIN_INLINE_CHARS,
            WEB_CRAWLER_MAX_INLINE_CHARS,
        );
        let Some(delivery) = self
            .webfetch
            .next_markdown_window(
                invocation.session_id.as_i64(),
                args.url.as_deref(),
                output_window,
            )
            .await
        else {
            let mut output =
                normalizer.failure(invocation, no_cached_document_message(TOOL_WEB_CRAWLER));
            output.structured_payload = Some(no_cached_document_payload(
                TOOL_WEB_CRAWLER,
                Some("webfetch_md"),
            ));
            return Ok(output);
        };

        let stdout = render_delivery_stdout(
            TOOL_WEB_CRAWLER,
            &delivery,
            Some(&DeliveryStdoutExtra {
                backend: Some("webfetch_md"),
                render: Some("http"),
            }),
        );
        let mut output = normalizer.success(invocation, &stdout, "");
        output.structured_payload = Some(delivery_success_payload(
            TOOL_WEB_CRAWLER,
            &delivery,
            Some(&DeliveryPayloadExtra {
                backend: Some("webfetch_md"),
                render: Some("http"),
                rendered_with: None,
                status_code: None,
                raw_payload: None,
            }),
        ));
        Ok(output)
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
#[async_trait]
impl ToolExecutor for WebCrawlerToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        if self.name.as_str() != TOOL_WEB_CRAWLER {
            return Err(ToolRuntimeError::Failure(format!(
                "unknown web_crawler tool: {}",
                self.name.as_str()
            )));
        }

        let args =
            parse_web_crawler_args(&invocation.raw_arguments).map_err(web_crawler_runtime_error)?;
        self.execute_crawler(&invocation, args).await
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_WEB_CRAWLER.to_string(),
        description: concat!(
            "Fetch one known http/https URL as Markdown. ",
            "Use render:\"http\" for static pages (default), ",
            "render:\"lightpanda\" for lightweight JS rendering, ",
            "or render:\"playwright\" for full browser rendering of SPAs and dynamic content. ",
            "If a render mode returns only a shell or loading placeholder, ",
            "retry with a heavier mode instead of the same one."
        )
        .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Fully-qualified public http/https URL to fetch. Required unless read is \"next\"."
                },
                "read": {
                    "type": "string",
                    "enum": ["auto", "next"],
                    "description": "auto fetches the URL and starts reading; next continues the last cached page in this session"
                },
                "render": {
                    "type": "string",
                    "enum": ["http", "lightpanda", "playwright"],
                    "description": "Render mode: http (default, no JS), lightpanda (lightweight JS), playwright (full browser). Use http for static pages; lightpanda or playwright for SPAs and JS-rendered content."
                },
                "render_wait_ms": {
                    "type": "integer",
                    "description": "Milliseconds to wait after JS rendering for late content (rendered modes only; default 3000)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Optional request timeout in seconds for http mode; defaults to 10"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Optional maximum Markdown output characters"
                }
            },
            "additionalProperties": false
        }),
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn parse_web_crawler_args(arguments: &str) -> anyhow::Result<WebCrawlerArgs> {
    serde_json::from_str(arguments)
        .map_err(|error| anyhow::anyhow!("invalid web_crawler arguments: {error}"))
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_webfetch_timeout_secs(args: &WebCrawlerArgs) -> u64 {
    args.timeout_secs
        .unwrap_or(WEB_CRAWLER_DEFAULT_WEBFETCH_TIMEOUT_SECS)
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid web_crawler arguments") {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_webfetch_failure_payload(
    args: Option<&WebMarkdownArgs>,
    error: &anyhow::Error,
) -> Value {
    let mut payload = WebFetchMdProvider::failure_payload(args, error);
    if let Some(object) = payload.as_object_mut() {
        object.insert("provider".to_string(), json!(TOOL_WEB_CRAWLER));
        object.insert("backend".to_string(), json!("webfetch_md"));
        object.insert("render".to_string(), json!("http"));
        object.insert(
            "webfetch_error_kind".to_string(),
            json!(WebFetchMdProvider::error_kind(error)),
        );
    }
    payload
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_render_unavailable_message(url: &str, render: &str) -> String {
    format!(
        "web_crawler render:{render} requested for {url}, but no CRW provider is configured. \
         Use render:\"http\" or configure CRW."
    )
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_render_unavailable_payload(url: &str, render: &str) -> Value {
    json!({
        "provider": TOOL_WEB_CRAWLER,
        "backend": null,
        "render": render,
        "kind": "fetch",
        "url": url,
        "error_kind": "render_provider_unavailable",
        "retryable": false,
        "provider_unavailable": true,
        "success": false,
        "message": web_crawler_render_unavailable_message(url, render)
    })
}

#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
fn web_crawler_crw_failure_payload(
    url: &str,
    render: &str,
    crw_error_kind: &str,
    crw_error_message: &str,
) -> Value {
    json!({
        "provider": TOOL_WEB_CRAWLER,
        "backend": "crw_scrape",
        "render": render,
        "kind": "fetch",
        "url": url,
        "error_kind": crw_error_kind,
        "retryable": false,
        "provider_unavailable": true,
        "success": false,
        "message": crw_error_message
    })
}

#[cfg(all(test, oxide_module_tool_webfetch_md))]
mod web_crawler_tests {
    use super::*;
    use crate::agent::providers::webfetch_md::{FetchedMarkdownDocument, OutputWindow};

    #[test]
    fn web_crawler_accepts_stringified_numeric_arguments() {
        let args = parse_web_crawler_args(
            r#"{"timeout_secs":"10","max_chars":"8000","render_wait_ms":"3000"}"#,
        )
        .expect("stringified unsigned integers should parse");

        assert_eq!(args.timeout_secs, Some(10));
        assert_eq!(args.max_chars, Some(8_000));
        assert_eq!(args.render_wait_ms, Some(3_000));
    }

    #[test]
    fn web_crawler_rejects_invalid_stringified_numeric_arguments() {
        assert!(parse_web_crawler_args(r#"{"max_chars":"many"}"#).is_err());
        assert!(parse_web_crawler_args(r#"{"timeout_secs":"-1"}"#).is_err());
        assert!(parse_web_crawler_args(r#"{"render_wait_ms":"1.5"}"#).is_err());
    }

    #[test]
    fn web_crawler_webfetch_timeout_defaults_to_ten_seconds() {
        let args = WebCrawlerArgs {
            url: Some("https://example.test".to_string()),
            ..WebCrawlerArgs::default()
        };

        assert_eq!(web_crawler_webfetch_timeout_secs(&args), 10);
    }

    #[test]
    fn web_crawler_webfetch_timeout_preserves_explicit_value() {
        let args = WebCrawlerArgs {
            url: Some("https://example.test".to_string()),
            timeout_secs: Some(3),
            ..WebCrawlerArgs::default()
        };

        assert_eq!(web_crawler_webfetch_timeout_secs(&args), 3);
    }

    #[test]
    fn web_crawler_read_next_does_not_require_url() {
        let args = WebCrawlerArgs {
            read: Some("next".to_string()),
            ..WebCrawlerArgs::default()
        };

        assert_eq!(
            parse_read_mode(args.read.as_deref(), TOOL_WEB_CRAWLER).expect("valid read mode"),
            MarkdownReadMode::Next
        );
    }

    #[tokio::test]
    async fn web_crawler_window_payload_reports_honest_continuation() {
        let executor = WebCrawlerToolExecutor::new();
        let document = FetchedMarkdownDocument {
            metadata: vec![("URL".to_string(), "https://example.test/page".to_string())],
            fetched_bytes: Some(42),
            markdown: "abcdef".to_string(),
        };
        let window = executor
            .webfetch
            .store_markdown_window(
                7,
                "https://example.test/page".to_string(),
                document,
                OutputWindow {
                    max_chars: 3,
                    offset_chars: 0,
                },
            )
            .await;

        let payload = delivery_success_payload(
            TOOL_WEB_CRAWLER,
            &window,
            Some(&DeliveryPayloadExtra {
                backend: Some("webfetch_md"),
                render: Some("http"),
                rendered_with: None,
                status_code: None,
                raw_payload: None,
            }),
        );

        assert_eq!(payload["truncated"], true);
        assert_eq!(payload["complete"], false);
        assert_eq!(payload["markdown_chars"], 6);
        assert_eq!(payload["returned_chars"], 3);
        assert_eq!(payload["remaining_chars"], 3);
        assert_eq!(payload["next_offset_chars"], 3);
        assert_eq!(payload["continue_with"]["args"]["read"], "next");
    }

    #[tokio::test]
    async fn web_crawler_cached_next_advances_without_llm_offset() {
        let executor = WebCrawlerToolExecutor::new();
        let document = FetchedMarkdownDocument {
            metadata: vec![("URL".to_string(), "https://example.test/page".to_string())],
            fetched_bytes: Some(42),
            markdown: "abcdef".to_string(),
        };
        let first = executor
            .webfetch
            .store_markdown_window(
                7,
                "https://example.test/page".to_string(),
                document,
                OutputWindow {
                    max_chars: 3,
                    offset_chars: 0,
                },
            )
            .await;
        assert_eq!(first.windowed.next_offset_chars, Some(3));

        let next = executor
            .webfetch
            .next_markdown_window(
                7,
                None,
                OutputWindow {
                    max_chars: 3,
                    offset_chars: 0,
                },
            )
            .await
            .expect("cached document");

        assert_eq!(next.output_window.offset_chars, 3);
        assert_eq!(next.windowed.text, "def");
        assert_eq!(next.windowed.next_offset_chars, None);
    }

    #[test]
    fn parse_render_mode_defaults_to_http() {
        assert_eq!(
            parse_render_mode(None).expect("missing render defaults to http"),
            RenderMode::Http
        );
        assert_eq!(
            parse_render_mode(Some("")).expect("empty render defaults to http"),
            RenderMode::Http
        );
        assert_eq!(
            parse_render_mode(Some("http")).expect("http render parses"),
            RenderMode::Http
        );
    }

    #[test]
    fn parse_render_mode_accepts_lightpanda_and_playwright() {
        assert_eq!(
            parse_render_mode(Some("lightpanda")).expect("lightpanda render parses"),
            RenderMode::Lightpanda
        );
        assert_eq!(
            parse_render_mode(Some("playwright")).expect("playwright render parses"),
            RenderMode::Playwright
        );
    }

    #[test]
    fn parse_render_mode_rejects_unknown() {
        assert!(parse_render_mode(Some("chrome")).is_err());
        assert!(parse_render_mode(Some("auto")).is_err());
        assert!(parse_render_mode(Some("rendered")).is_err());
    }

    #[test]
    fn parse_render_mode_trims_whitespace() {
        assert_eq!(
            parse_render_mode(Some("  playwright  ")).expect("trimmed playwright render parses"),
            RenderMode::Playwright
        );
    }

    #[test]
    fn render_mode_as_str_round_trips() {
        assert_eq!(RenderMode::Http.as_str(), "http");
        assert_eq!(RenderMode::Lightpanda.as_str(), "lightpanda");
        assert_eq!(RenderMode::Playwright.as_str(), "playwright");
    }

    #[test]
    fn web_crawler_render_unavailable_payload_has_correct_shape() {
        let payload =
            web_crawler_render_unavailable_payload("https://example.test/page", "playwright");
        assert_eq!(payload["provider"], "web_crawler");
        assert_eq!(payload["render"], "playwright");
        assert_eq!(payload["error_kind"], "render_provider_unavailable");
        assert_eq!(payload["provider_unavailable"], true);
        assert_eq!(payload["success"], false);
        assert_eq!(payload["retryable"], false);
    }
}

/// Capability module for unified indexed web search.
#[cfg(oxide_module_tool_web_search)]
pub struct WebSearchToolModule;

#[cfg(oxide_module_tool_web_search)]
impl WebSearchToolModule {
    fn provider(&self) -> Option<WebSearchProvider> {
        match WebSearchProvider::new_from_env() {
            Ok(provider) => provider,
            Err(error) => {
                tracing::warn!(error = %error, "web_search provider initialization failed");
                None
            }
        }
    }
}

#[cfg(oxide_module_tool_web_search)]
impl ToolModule for WebSearchToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/web-search")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Web)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Kokoro English text-to-speech tools.
#[cfg(oxide_module_tool_tts_kokoro)]
pub struct KokoroTtsToolModule;

#[cfg(oxide_module_tool_tts_kokoro)]
impl KokoroTtsToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<KokoroTtsProvider> {
        let config = TtsConfig::from_env();

        if let Ok(url) = std::env::var("KOKORO_TTS_URL")
            && url.trim().is_empty()
        {
            tracing::debug!(
                "TTS provider disabled: KOKORO_TTS_URL is explicitly set to empty string"
            );
            return None;
        }

        tracing::debug!(url = %config.base_url, "Registering TTS provider");
        let mut provider =
            KokoroTtsProvider::from_config(config).with_sandbox_runtime(ctx.sandbox_runtime());
        if let Some(tx) = ctx.progress_tx() {
            provider = provider.with_progress_tx(tx);
        }

        let base_url = provider.base_url().to_string();
        tracing::debug!(url = %base_url, "Kokoro TTS provider registered");
        Some(provider)
    }
}

#[cfg(oxide_module_tool_tts_kokoro)]
impl ToolModule for KokoroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-kokoro")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Tts)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Silero Russian text-to-speech tools.
#[cfg(oxide_module_tool_tts_silero)]
pub struct SileroTtsToolModule;

#[cfg(oxide_module_tool_tts_silero)]
impl SileroTtsToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<SileroTtsProvider> {
        let config = SileroTtsConfig::from_env();

        if let Ok(url) = std::env::var("SILERO_TTS_URL")
            && url.trim().is_empty()
        {
            tracing::debug!(
                "Silero TTS provider disabled: SILERO_TTS_URL is explicitly set to empty string"
            );
            return None;
        }

        tracing::debug!(url = %config.base_url, "Registering Silero TTS provider");
        let mut provider =
            SileroTtsProvider::from_config(config).with_sandbox_runtime(ctx.sandbox_runtime());
        if let Some(tx) = ctx.progress_tx() {
            provider = provider.with_progress_tx(tx);
        }

        let base_url = provider.base_url().to_string();
        tracing::debug!(url = %base_url, "Silero TTS provider registered");
        Some(provider)
    }
}

#[cfg(oxide_module_tool_tts_silero)]
impl ToolModule for SileroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-silero")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Tts)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for yt-dlp media tools.
#[cfg(oxide_module_tool_ytdlp)]
pub struct YtdlpToolModule;

#[cfg(oxide_module_tool_ytdlp)]
impl YtdlpToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> YtdlpProvider {
        if let Some(tx) = ctx.progress_tx() {
            YtdlpProvider::from_runtime(ctx.sandbox_runtime()).with_progress_tx(tx)
        } else {
            YtdlpProvider::from_runtime(ctx.sandbox_runtime())
        }
    }
}

#[cfg(oxide_module_tool_ytdlp)]
impl ToolModule for YtdlpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/ytdlp")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Ytdlp)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(self.provider(ctx)).tool_runtime_executors()
    }
}

/// Capability module for the `write_todos` typed runtime tool.
#[cfg(oxide_module_tool_todos)]
pub struct TodosToolModule;

#[cfg(oxide_module_tool_todos)]
impl ToolModule for TodosToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/todos")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        None
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::AlwaysVisible
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(TodosProvider::new(ctx.todos())).tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for sandbox command execution.
#[cfg(oxide_module_tool_sandbox_exec)]
pub struct SandboxExecToolModule;

#[cfg(oxide_module_tool_sandbox_exec)]
impl ToolModule for SandboxExecToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-exec")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Shell)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxExecProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}

/// Capability module for sandbox file operations.
#[cfg(oxide_module_tool_sandbox_fileops)]
pub struct SandboxFileOpsToolModule;

#[cfg(oxide_module_tool_sandbox_fileops)]
impl ToolModule for SandboxFileOpsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-fileops")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Files)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxFileOpsProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}

/// Capability module for sandbox recreation.
#[cfg(oxide_module_tool_sandbox_recreate)]
pub struct SandboxRecreateToolModule;

#[cfg(oxide_module_tool_sandbox_recreate)]
impl ToolModule for SandboxRecreateToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-recreate")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        Some(CapabilityGroup::Shell)
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::Deferred
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxLifecycleProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}

#[cfg(test)]
mod capability_mapping_tests {
    use super::*;

    /// Verify every compiled tool module has a consistent
    /// (`capability_group`, `visibility`) pair:
    /// - AlwaysVisible ⇒ `None` group
    /// - Deferred ⇒ `Some(group)`
    #[test]
    #[allow(clippy::vec_init_then_push)]
    #[allow(unused_mut)]
    fn compiled_modules_have_consistent_group_and_visibility() {
        let mut checks: Vec<(&str, Option<CapabilityGroup>, ToolVisibility)> = Vec::new();

        #[cfg(oxide_module_tool_browser_live)]
        checks.push((
            "browser-live",
            BrowserLiveToolModule.capability_group(),
            BrowserLiveToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_compression)]
        checks.push((
            "compression",
            CompressionToolModule.capability_group(),
            CompressionToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_file_delivery)]
        checks.push((
            "file-delivery",
            FileDeliveryToolModule.capability_group(),
            FileDeliveryToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_agents_md)]
        checks.push((
            "agents-md",
            AgentsMdToolModule.capability_group(),
            AgentsMdToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_delegation)]
        checks.push((
            "delegation",
            DelegationToolModule.capability_group(),
            DelegationToolModule.visibility(),
        ));

        #[cfg(oxide_module_manager_control_plane)]
        checks.push((
            "manager",
            ManagerControlPlaneToolModule.capability_group(),
            ManagerControlPlaneToolModule.visibility(),
        ));

        #[cfg(oxide_module_integration_ssh_mcp)]
        checks.push((
            "ssh-mcp",
            SshMcpToolModule.capability_group(),
            SshMcpToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_reminder)]
        checks.push((
            "reminder",
            ReminderToolModule.capability_group(),
            ReminderToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_audio_stt)]
        checks.push((
            "audio-stt",
            AudioSttToolModule.capability_group(),
            AudioSttToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_vision_image)]
        checks.push((
            "vision-image",
            VisionImageToolModule.capability_group(),
            VisionImageToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_vision_video)]
        checks.push((
            "vision-video",
            VisionVideoToolModule.capability_group(),
            VisionVideoToolModule.visibility(),
        ));

        #[cfg(oxide_module_integration_mcp_jira)]
        checks.push((
            "mcp-jira",
            JiraMcpToolModule.capability_group(),
            JiraMcpToolModule.visibility(),
        ));

        #[cfg(oxide_module_integration_mcp_mattermost)]
        checks.push((
            "mcp-mattermost",
            MattermostMcpToolModule.capability_group(),
            MattermostMcpToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_stack_logs)]
        checks.push((
            "stack-logs",
            StackLogsToolModule.capability_group(),
            StackLogsToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_webfetch_md)]
        checks.push((
            "webfetch-md",
            WebFetchMdToolModule.capability_group(),
            WebFetchMdToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_webfetch_md)]
        checks.push((
            "web-crawler",
            WebCrawlerToolModule.capability_group(),
            WebCrawlerToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_web_search)]
        checks.push((
            "web-search",
            WebSearchToolModule.capability_group(),
            WebSearchToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_tts_kokoro)]
        checks.push((
            "tts-kokoro",
            KokoroTtsToolModule.capability_group(),
            KokoroTtsToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_tts_silero)]
        checks.push((
            "tts-silero",
            SileroTtsToolModule.capability_group(),
            SileroTtsToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_ytdlp)]
        checks.push((
            "ytdlp",
            YtdlpToolModule.capability_group(),
            YtdlpToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_todos)]
        checks.push((
            "todos",
            TodosToolModule.capability_group(),
            TodosToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_sandbox_exec)]
        checks.push((
            "sandbox-exec",
            SandboxExecToolModule.capability_group(),
            SandboxExecToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_sandbox_fileops)]
        checks.push((
            "sandbox-fileops",
            SandboxFileOpsToolModule.capability_group(),
            SandboxFileOpsToolModule.visibility(),
        ));

        #[cfg(oxide_module_tool_sandbox_recreate)]
        checks.push((
            "sandbox-recreate",
            SandboxRecreateToolModule.capability_group(),
            SandboxRecreateToolModule.visibility(),
        ));

        assert!(
            !checks.is_empty(),
            "no tool modules compiled in this profile"
        );

        for (name, group, visibility) in &checks {
            match visibility {
                ToolVisibility::AlwaysVisible => assert!(
                    group.is_none(),
                    "AlwaysVisible module {name} must have None capability_group"
                ),
                ToolVisibility::Deferred => assert!(
                    group.is_some(),
                    "Deferred module {name} must have Some(capability_group)"
                ),
            }
        }
    }

    /// Verify specific group assignments for representative modules.
    #[test]
    fn representative_group_assignments() {
        #[cfg(oxide_module_tool_todos)]
        {
            assert_eq!(TodosToolModule.capability_group(), None);
            assert_eq!(TodosToolModule.visibility(), ToolVisibility::AlwaysVisible);
        }

        #[cfg(oxide_module_tool_compression)]
        {
            assert_eq!(CompressionToolModule.capability_group(), None);
            assert_eq!(
                CompressionToolModule.visibility(),
                ToolVisibility::AlwaysVisible
            );
        }

        #[cfg(oxide_module_tool_sandbox_fileops)]
        {
            assert_eq!(
                SandboxFileOpsToolModule.capability_group(),
                Some(CapabilityGroup::Files)
            );
            assert_eq!(
                SandboxFileOpsToolModule.visibility(),
                ToolVisibility::Deferred
            );
        }

        #[cfg(oxide_module_tool_sandbox_exec)]
        {
            assert_eq!(
                SandboxExecToolModule.capability_group(),
                Some(CapabilityGroup::Shell)
            );
        }

        #[cfg(oxide_module_tool_web_search)]
        {
            assert_eq!(
                WebSearchToolModule.capability_group(),
                Some(CapabilityGroup::Web)
            );
        }
    }
}
