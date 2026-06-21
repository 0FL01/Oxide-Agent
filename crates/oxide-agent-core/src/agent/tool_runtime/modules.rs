//! Capability-oriented tool modules.

use super::ToolExecutor;
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
use crate::agent::session::AgentMemoryScope;
use crate::agent::wiki_memory::WikiStore;
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
#[cfg(oxide_module_tool_brave_search)]
use crate::agent::providers::BraveSearchProvider;
#[cfg(oxide_module_tool_compression)]
use crate::agent::providers::CompressionProvider;
#[cfg(oxide_module_tool_crw)]
use crate::agent::providers::CrwProvider;
#[cfg(oxide_module_tool_delegation)]
use crate::agent::providers::DelegationProvider;
#[cfg(oxide_module_tool_file_delivery)]
use crate::agent::providers::FileHosterProvider;
#[cfg(oxide_module_manager_control_plane)]
use crate::agent::providers::ManagerControlPlaneProvider;
use crate::agent::providers::ManagerTopicLifecycle;
#[cfg(any(
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video
))]
use crate::agent::providers::MediaFileProvider;
use crate::agent::providers::ReminderContext;
#[cfg(oxide_module_tool_reminder)]
use crate::agent::providers::ReminderProvider;
#[cfg(oxide_module_integration_ssh_mcp)]
use crate::agent::providers::SshMcpProvider;
#[cfg(oxide_module_tool_stack_logs)]
use crate::agent::providers::StackLogsProvider;
#[cfg(oxide_module_tool_tavily)]
use crate::agent::providers::TavilyProvider;
#[cfg(oxide_module_tool_todos)]
use crate::agent::providers::TodosProvider;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::WebFetchMdProvider;
#[cfg(oxide_module_tool_wiki_memory)]
use crate::agent::providers::WikiMemoryProvider;
#[cfg(oxide_module_tool_ytdlp)]
use crate::agent::providers::YtdlpProvider;
#[cfg(oxide_module_integration_ssh_mcp)]
use crate::agent::providers::ssh_mcp::cleanup_stale_private_key_tempfiles;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::webfetch_md::WebMarkdownArgs;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::providers::webfetch_md::{
    MarkdownDeliveryResult, MarkdownReadMode, OutputWindow, document_metadata,
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
    wiki_memory_store: Option<WikiStore>,
    memory_scope: AgentMemoryScope,
    progress_tx: Option<Sender<AgentEvent>>,
    inherited_model: Option<crate::config::ModelInfo>,
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
    /// Optional durable wiki memory store.
    pub wiki_memory_store: Option<WikiStore>,
    /// Stable memory scope for wiki memory tools.
    pub memory_scope: AgentMemoryScope,
    /// Optional progress sender.
    pub progress_tx: Option<Sender<AgentEvent>>,
    /// Parent session's effective model, inherited by sub-agents when no
    /// explicit sub-agent model is configured. `None` when no per-session
    /// override is active (e.g. Telegram, or web sessions using the bootstrap
    /// default).
    pub inherited_model: Option<crate::config::ModelInfo>,
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
            wiki_memory_store: parts.wiki_memory_store,
            memory_scope: parts.memory_scope,
            progress_tx: parts.progress_tx,
            inherited_model: parts.inherited_model,
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

    /// Optional durable wiki memory store.
    #[cfg_attr(not(oxide_module_tool_wiki_memory), allow(dead_code))]
    #[must_use]
    pub fn wiki_memory_store(&self) -> Option<WikiStore> {
        self.wiki_memory_store.clone()
    }

    /// Stable memory scope used by wiki memory tools.
    #[cfg_attr(not(oxide_module_tool_wiki_memory), allow(dead_code))]
    #[must_use]
    pub fn memory_scope(&self) -> AgentMemoryScope {
        self.memory_scope.clone()
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
}

/// Tool capability module.
pub trait ToolModule {
    /// Stable module ID corresponding to the compiled capability manifest.
    fn module_id(&self) -> ModuleId;

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

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for scoped durable wiki memory tools.
#[cfg(oxide_module_tool_wiki_memory)]
pub struct WikiMemoryToolModule;

#[cfg(oxide_module_tool_wiki_memory)]
impl WikiMemoryToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<WikiMemoryProvider> {
        let store = ctx.wiki_memory_store()?;
        let scope = ctx.memory_scope();
        Some(WikiMemoryProvider::new(
            store,
            scope.user_id,
            scope.context_key,
        ))
    }
}

#[cfg(oxide_module_tool_wiki_memory)]
impl ToolModule for WikiMemoryToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/wiki-memory")
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

#[cfg(any(
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video
))]
fn media_file_provider(ctx: &ToolModuleContext) -> MediaFileProvider {
    match ctx.browser_live_context() {
        Some(live_ctx) => MediaFileProvider::from_runtime_with_storage(
            ctx.llm_client(),
            ctx.sandbox_runtime(),
            live_ctx.storage(),
            live_ctx.user_id(),
        ),
        None => MediaFileProvider::from_runtime(ctx.llm_client(), ctx.sandbox_runtime()),
    }
}

/// Capability module for audio file transcription.
#[cfg(oxide_module_tool_media_audio)]
pub struct MediaAudioToolModule;

#[cfg(oxide_module_tool_media_audio)]
impl ToolModule for MediaAudioToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-audio")
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["transcribe_audio_file"])
    }
}

/// Capability module for image file description.
#[cfg(oxide_module_tool_media_image)]
pub struct MediaImageToolModule;

#[cfg(oxide_module_tool_media_image)]
impl ToolModule for MediaImageToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-image")
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["describe_image_file"])
    }
}

/// Capability module for video file description.
#[cfg(oxide_module_tool_media_video)]
pub struct MediaVideoToolModule;

#[cfg(oxide_module_tool_media_video)]
impl ToolModule for MediaVideoToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-video")
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
#[derive(Debug, Deserialize, Clone, Default)]
struct WebCrawlerArgs {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    read: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    offset_chars: Option<usize>,
    #[serde(default)]
    wait_for: Option<String>,
    #[serde(default)]
    fresh: bool,
}

#[cfg(oxide_module_tool_webfetch_md)]
struct WebCrawlerToolExecutor {
    webfetch: WebFetchMdProvider,
    #[cfg(oxide_module_tool_crw)]
    crw: Option<Arc<CrwProvider>>,
    name: ToolName,
    spec: ToolDefinition,
}

#[cfg(oxide_module_tool_webfetch_md)]
impl WebCrawlerToolExecutor {
    fn new() -> Self {
        #[cfg(oxide_module_tool_crw)]
        let crw = crate::config::is_crw_enabled()
            .then(CrwProvider::new)
            .and_then(|res| res.ok())
            .map(Arc::new);

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
        if web_crawler_read_mode(&args)? == MarkdownReadMode::Next {
            return self
                .execute_cached_next(invocation, &normalizer, &args)
                .await;
        }

        let url = args
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                ToolRuntimeError::InvalidArguments(
                    "web_crawler requires url unless read is \"next\"".to_string(),
                )
            })?
            .to_string();
        let webfetch_args = WebMarkdownArgs {
            url: Some(url.clone()),
            read: None,
            timeout_secs: Some(web_crawler_webfetch_timeout_secs(&args)),
            max_chars: None,
            offset_chars: None,
        };

        match self
            .webfetch
            .fetch_markdown_document(webfetch_args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(document) => {
                let window = self
                    .webfetch
                    .store_markdown_window(
                        invocation.session_id.as_i64(),
                        url.clone(),
                        document,
                        web_crawler_output_window(&args, args.offset_chars.unwrap_or(0)),
                    )
                    .await;
                let final_url = document_metadata(&window.document, "URL");
                let stdout = web_crawler_window_output(
                    "webfetch_md",
                    None,
                    &url,
                    final_url.as_deref(),
                    &window,
                );
                let mut output = normalizer.success(invocation, &stdout, "");
                output.structured_payload = Some(web_crawler_document_success_payload(
                    "webfetch_md",
                    None,
                    &url,
                    final_url.as_deref(),
                    &window,
                    None,
                    None,
                ));
                Ok(output)
            }
            Err(webfetch_error) => {
                let fallback_reason = web_crawler_fallback_reason(&webfetch_args, &webfetch_error);
                let Some(fallback_reason) = fallback_reason else {
                    let message =
                        WebFetchMdProvider::failure_message(Some(&webfetch_args), &webfetch_error);
                    let mut output = normalizer.failure(invocation, message);
                    output.structured_payload = Some(web_crawler_webfetch_failure_payload(
                        Some(&webfetch_args),
                        &webfetch_error,
                    ));
                    return Ok(output);
                };

                self.execute_rendered_fallback(
                    invocation,
                    &normalizer,
                    args,
                    webfetch_args,
                    &webfetch_error,
                    fallback_reason,
                )
                .await
            }
        }
    }

    async fn execute_cached_next(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: &WebCrawlerArgs,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let Some(window) = self
            .webfetch
            .next_markdown_window(
                invocation.session_id.as_i64(),
                args.url.as_deref(),
                web_crawler_output_window(args, 0),
            )
            .await
        else {
            let mut output = normalizer.failure(
                invocation,
                "web_crawler has no cached page to continue in this session; call web_crawler with url first",
            );
            output.structured_payload = Some(json!({
                "provider": TOOL_WEB_CRAWLER,
                "backend": "webfetch_md",
                "error_kind": "no_cached_document",
                "retryable": false,
                "success": false
            }));
            return Ok(output);
        };

        let final_url = document_metadata(&window.document, "URL");
        let stdout = web_crawler_window_output(
            "webfetch_md",
            None,
            &window.requested_url,
            final_url.as_deref(),
            &window,
        );
        let mut output = normalizer.success(invocation, &stdout, "");
        output.structured_payload = Some(web_crawler_document_success_payload(
            "webfetch_md",
            None,
            &window.requested_url,
            final_url.as_deref(),
            &window,
            None,
            None,
        ));
        Ok(output)
    }

    async fn execute_rendered_fallback(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: WebCrawlerArgs,
        webfetch_args: WebMarkdownArgs,
        webfetch_error: &anyhow::Error,
        fallback_reason: &'static str,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let _ = (&args.wait_for, args.fresh);

        #[cfg(oxide_module_tool_crw)]
        if let Some(crw) = &self.crw {
            return self
                .execute_crw_scrape_fallback(
                    invocation,
                    normalizer,
                    &args,
                    &webfetch_args,
                    webfetch_error,
                    fallback_reason,
                    crw,
                )
                .await;
        }

        let mut output = normalizer.failure(
            invocation,
            web_crawler_fallback_unavailable_message(web_crawler_webfetch_url(&webfetch_args)),
        );
        output.structured_payload = Some(web_crawler_no_fallback_payload(
            &webfetch_args,
            webfetch_error,
            fallback_reason,
        ));
        Ok(output)
    }

    /// CRW scrape fallback for anti-bot/JS-blocked URLs.
    ///
    /// Called when webfetch fails with an anti-bot or access-block error
    /// and a CRW provider is configured.
    #[cfg(oxide_module_tool_crw)]
    async fn execute_crw_scrape_fallback(
        &self,
        invocation: &ToolInvocation,
        normalizer: &OutputNormalizer,
        args: &WebCrawlerArgs,
        webfetch_args: &WebMarkdownArgs,
        webfetch_error: &anyhow::Error,
        fallback_reason: &'static str,
        crw: &Arc<CrwProvider>,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        use crate::agent::providers::crw::CrwScrapeArgs;

        let scrape_args = CrwScrapeArgs {
            url: web_crawler_webfetch_url(webfetch_args).to_string(),
        };

        match crw.client().scrape(&scrape_args).await {
            Ok(response) => {
                let markdown = &response.data.markdown;
                let final_url = response.data.metadata.url.as_deref();
                let status_code = response.data.metadata.status_code.map(u64::from);

                let stdout = web_crawler_output(
                    "crw_scrape",
                    Some(fallback_reason),
                    web_crawler_webfetch_url(webfetch_args),
                    final_url,
                    markdown,
                );
                let mut output = normalizer.success(invocation, &stdout, "");
                output.structured_payload = Some(web_crawler_success_payload(
                    "crw_scrape",
                    Some(fallback_reason),
                    web_crawler_webfetch_url(webfetch_args),
                    final_url,
                    markdown,
                    status_code,
                    None,
                    None,
                ));
                Ok(output)
            }
            Err(crw_error) => {
                let crw_error_kind = crw_error.kind().to_string();
                let crw_error_message = crw_error.agent_message();
                let message = format!(
                    "web_crawler lightweight fetch failed for {} ({}); \
                     CRW scrape fallback also failed: {}. \
                     This path is closed for this task; use another source.",
                    web_crawler_webfetch_url(webfetch_args),
                    fallback_reason,
                    crw_error_message
                );
                let mut output = normalizer.failure(invocation, message);
                output.structured_payload = Some(web_crawler_crw_failure_payload(
                    webfetch_args,
                    webfetch_error,
                    fallback_reason,
                    &crw_error_kind,
                    &crw_error_message,
                ));
                Ok(output)
            }
        }
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
            "Fetch one known http/https URL as Markdown. Uses lightweight webfetch first, ",
            "then falls back to a browser-rendered service only for JS/CAPTCHA/anti-bot blocks when configured. ",
            "If both paths fail, use another source instead of retrying the same host."
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
                    "description": "auto fetches the URL and starts reading; next continues the last cached page in this session without requiring offset_chars"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Optional request timeout in seconds; lightweight fast path defaults to 10 seconds"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Optional maximum Markdown output characters"
                },
                "offset_chars": {
                    "type": "integer",
                    "description": "Optional character offset for lightweight webfetch pagination"
                },
                "wait_for": {
                    "type": "string",
                    "description": "Optional CSS selector for rendered fallback"
                },
                "fresh": {
                    "type": "boolean",
                    "description": "If true, bypass cache on rendered fallback; default false"
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
fn web_crawler_webfetch_url(args: &WebMarkdownArgs) -> &str {
    args.url.as_deref().unwrap_or("")
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_read_mode(args: &WebCrawlerArgs) -> Result<MarkdownReadMode, ToolRuntimeError> {
    match args
        .read
        .as_deref()
        .map(str::trim)
        .filter(|read| !read.is_empty())
    {
        None | Some("auto") => Ok(MarkdownReadMode::Auto),
        Some("next") => Ok(MarkdownReadMode::Next),
        Some(other) => Err(ToolRuntimeError::InvalidArguments(format!(
            "invalid web_crawler read mode '{other}'; expected 'auto' or 'next'"
        ))),
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_output_window(args: &WebCrawlerArgs, offset_chars: usize) -> OutputWindow {
    OutputWindow {
        max_chars: args
            .max_chars
            .unwrap_or(WEB_CRAWLER_DEFAULT_INLINE_CHARS)
            .clamp(WEB_CRAWLER_MIN_INLINE_CHARS, WEB_CRAWLER_MAX_INLINE_CHARS),
        offset_chars,
    }
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

#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
fn web_crawler_output(
    backend: &str,
    fallback_reason: Option<&str>,
    url: &str,
    final_url: Option<&str>,
    markdown: &str,
) -> String {
    let mut output = String::from("## Web Crawler\n\n");
    output.push_str("Backend: ");
    output.push_str(backend);
    output.push('\n');
    if let Some(reason) = fallback_reason {
        output.push_str("Fallback-Reason: ");
        output.push_str(reason);
        output.push('\n');
    }
    output.push_str("URL: ");
    output.push_str(url);
    output.push('\n');
    if let Some(final_url) = final_url {
        output.push_str("Final-URL: ");
        output.push_str(final_url);
        output.push('\n');
    }
    output.push_str("Chars: ");
    output.push_str(&markdown.chars().count().to_string());
    output.push_str("\n\n---\n\n");
    output.push_str(markdown);
    output
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_window_output(
    backend: &str,
    fallback_reason: Option<&str>,
    url: &str,
    final_url: Option<&str>,
    window: &MarkdownDeliveryResult,
) -> String {
    let mut output = String::from("## Web Crawler\n\n");
    output.push_str("Backend: ");
    output.push_str(backend);
    output.push('\n');
    if let Some(reason) = fallback_reason {
        output.push_str("Fallback-Reason: ");
        output.push_str(reason);
        output.push('\n');
    }
    output.push_str("URL: ");
    output.push_str(url);
    output.push('\n');
    if let Some(final_url) = final_url {
        output.push_str("Final-URL: ");
        output.push_str(final_url);
        output.push('\n');
    }
    output.push_str("Range-Chars: ");
    output.push_str(&window.output_window.offset_chars.to_string());
    output.push_str("..");
    output.push_str(
        &(window.output_window.offset_chars + window.windowed.returned_chars).to_string(),
    );
    output.push('\n');
    output.push_str("Markdown-Chars: ");
    output.push_str(&window.windowed.markdown_chars.to_string());
    output.push('\n');
    output.push_str("Truncated: ");
    output.push_str(if window.windowed.was_truncated {
        "yes"
    } else {
        "no"
    });
    output.push_str("\n\n---\n\n");
    output.push_str(&window.windowed.text);
    output
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_document_success_payload(
    backend: &str,
    fallback_reason: Option<&str>,
    url: &str,
    final_url: Option<&str>,
    window: &MarkdownDeliveryResult,
    status_code: Option<u64>,
    raw_payload: Option<&Value>,
) -> Value {
    let start_chars = window.output_window.offset_chars;
    let end_chars = start_chars + window.windowed.returned_chars;
    let has_more = window.windowed.was_truncated;
    let continue_with = has_more.then(|| {
        json!({
            "tool": TOOL_WEB_CRAWLER,
            "args": { "read": "next" }
        })
    });

    json!({
        "provider": TOOL_WEB_CRAWLER,
        "backend": backend,
        "fallback_reason": fallback_reason,
        "url": url,
        "final_url": final_url,
        "status_code": status_code,
        "markdown": window.windowed.text,
        "chars": window.windowed.markdown_chars,
        "markdown_chars": window.windowed.markdown_chars,
        "returned_chars": window.windowed.returned_chars,
        "remaining_chars": window.windowed.remaining_chars,
        "next_offset_chars": window.windowed.next_offset_chars,
        "truncated": has_more,
        "complete": start_chars == 0 && !has_more,
        "range": {
            "start_chars": start_chars,
            "end_chars": end_chars,
            "total_chars": window.windowed.markdown_chars,
            "has_more": has_more
        },
        "continue_with": continue_with,
        "raw_payload": raw_payload,
        "success": true
    })
}

#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
fn web_crawler_success_payload(
    backend: &str,
    fallback_reason: Option<&str>,
    url: &str,
    final_url: Option<&str>,
    markdown: &str,
    status_code: Option<u64>,
    truncated: Option<bool>,
    raw_payload: Option<&Value>,
) -> Value {
    json!({
        "provider": TOOL_WEB_CRAWLER,
        "backend": backend,
        "fallback_reason": fallback_reason,
        "url": url,
        "final_url": final_url,
        "status_code": status_code,
        "markdown": markdown,
        "chars": markdown.chars().count(),
        "truncated": truncated.unwrap_or(false),
        "raw_payload": raw_payload,
        "success": true
    })
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
        object.insert(
            "webfetch_error_kind".to_string(),
            json!(WebFetchMdProvider::error_kind(error)),
        );
    }
    payload
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_no_fallback_payload(
    args: &WebMarkdownArgs,
    error: &anyhow::Error,
    fallback_reason: &'static str,
) -> Value {
    let mut payload = web_crawler_webfetch_failure_payload(Some(args), error);
    if let Some(object) = payload.as_object_mut() {
        object.insert("backend".to_string(), json!("webfetch_md"));
        object.insert("fallback_backend".to_string(), json!("rendered_fallback"));
        object.insert("fallback_attempted".to_string(), json!(false));
        object.insert("fallback_reason".to_string(), json!(fallback_reason));
        object.insert(
            "fallback_error_kind".to_string(),
            json!("fallback_unavailable"),
        );
        object.insert("provider_unavailable".to_string(), json!(true));
        object.insert("retryable".to_string(), json!(false));
        object.insert(
            "message".to_string(),
            json!(web_crawler_fallback_unavailable_message(
                web_crawler_webfetch_url(args)
            )),
        );
    }
    payload
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_fallback_unavailable_message(url: &str) -> String {
    format!(
        "web_crawler lightweight fetch needs rendered fallback for {url}, but no rendered fallback provider is configured. This path is closed for this task; use another source."
    )
}

#[cfg(all(oxide_module_tool_webfetch_md, oxide_module_tool_crw))]
fn web_crawler_crw_failure_payload(
    webfetch_args: &WebMarkdownArgs,
    webfetch_error: &anyhow::Error,
    fallback_reason: &'static str,
    crw_error_kind: &str,
    crw_error_message: &str,
) -> Value {
    let web_payload = WebFetchMdProvider::failure_payload(Some(webfetch_args), webfetch_error);
    json!({
        "provider": TOOL_WEB_CRAWLER,
        "backend": "crw_scrape",
        "fallback_backend": "crw_scrape",
        "fallback_attempted": true,
        "fallback_reason": fallback_reason,
        "url": web_crawler_webfetch_url(webfetch_args),
        "host": web_payload.get("host").cloned().unwrap_or(Value::Null),
        "error_kind": crw_error_kind,
        "webfetch_error_kind": WebFetchMdProvider::error_kind(webfetch_error),
        "crw_error_kind": crw_error_kind,
        "retryable": false,
        "provider_unavailable": true,
        "message": crw_error_message,
        "webfetch_payload": web_payload
    })
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_fallback_reason(
    args: &WebMarkdownArgs,
    error: &anyhow::Error,
) -> Option<&'static str> {
    match WebFetchMdProvider::error_kind(error) {
        "anti_bot" => Some("webfetch anti_bot"),
        "http_status" => web_crawler_http_status_fallback_reason(args, error),
        _ => None,
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_http_status_fallback_reason(
    args: &WebMarkdownArgs,
    error: &anyhow::Error,
) -> Option<&'static str> {
    let payload = WebFetchMdProvider::failure_payload(Some(args), error);
    let status = payload.get("status_code").and_then(Value::as_u64);
    match status {
        Some(400..=403 | 429)
            if web_crawler_is_reddit_thread_url(web_crawler_webfetch_url(args)) =>
        {
            Some("webfetch reddit_rss_http_status")
        }
        Some(400..=403 | 429) => Some("webfetch http_status"),
        Some(503) => Some("webfetch http_status"),
        Some(500..=504) if web_crawler_is_reddit_thread_url(web_crawler_webfetch_url(args)) => {
            Some("webfetch reddit_rss_http_status")
        }
        _ => None,
    }
}

#[cfg(oxide_module_tool_webfetch_md)]
fn web_crawler_is_reddit_thread_url(raw_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw_url) else {
        return false;
    };
    let Some(host) = url
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
    else {
        return false;
    };
    if !matches!(
        host.as_str(),
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "new.reddit.com" | "sh.reddit.com"
    ) {
        return false;
    }

    let Some(mut segments) = url.path_segments() else {
        return false;
    };
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some("r"), Some(_), Some("comments"), Some(_))
    )
}

#[cfg(all(test, oxide_module_tool_webfetch_md))]
mod web_crawler_tests {
    use super::*;
    use crate::agent::providers::webfetch_md::FetchedMarkdownDocument;

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
            web_crawler_read_mode(&args).expect("valid read mode"),
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

        let payload = web_crawler_document_success_payload(
            "webfetch_md",
            None,
            "https://example.test/page",
            Some("https://example.test/page"),
            &window,
            None,
            None,
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
    fn web_crawler_falls_back_for_reddit_rss_retryable_http_status() {
        let args = WebMarkdownArgs {
            url: Some("https://www.reddit.com/r/LocalLLaMA/comments/1tcv14c/mtp_speed_with_3090_qwen_27b_q4/".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error = anyhow::anyhow!(
            "reddit rss fast-path failed: reddit rss returned non-success status: 429 Too Many Requests"
        );

        assert_eq!(
            web_crawler_fallback_reason(&args, &error),
            Some("webfetch reddit_rss_http_status")
        );
    }

    #[test]
    fn web_crawler_falls_back_for_generic_rate_limit_http_status() {
        let args = WebMarkdownArgs {
            url: Some("https://example.test/page".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error =
            anyhow::anyhow!("web_markdown fetch failed: non-success status: 429 Too Many Requests");

        assert_eq!(
            web_crawler_fallback_reason(&args, &error),
            Some("webfetch http_status")
        );
    }

    #[test]
    fn web_crawler_falls_back_for_generic_payment_required_http_status() {
        let args = WebMarkdownArgs {
            url: Some("https://www.investopedia.com/article-123".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error =
            anyhow::anyhow!("web_markdown fetch failed: non-success status: 402 Payment Required");

        assert_eq!(
            web_crawler_fallback_reason(&args, &error),
            Some("webfetch http_status")
        );
    }

    #[test]
    fn web_crawler_falls_back_for_generic_forbidden_http_status() {
        let args = WebMarkdownArgs {
            url: Some("https://example.test/page".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error = anyhow::anyhow!("web_markdown fetch failed: non-success status: 403 Forbidden");

        assert_eq!(
            web_crawler_fallback_reason(&args, &error),
            Some("webfetch http_status")
        );
    }

    #[test]
    fn web_crawler_falls_back_for_generic_service_unavailable() {
        let args = WebMarkdownArgs {
            url: Some("https://example.test/page".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error = anyhow::anyhow!(
            "web_markdown fetch failed: non-success status: 503 Service Unavailable"
        );

        assert_eq!(
            web_crawler_fallback_reason(&args, &error),
            Some("webfetch http_status")
        );
    }

    #[test]
    fn web_crawler_does_not_fallback_for_generic_not_found() {
        let args = WebMarkdownArgs {
            url: Some("https://example.test/missing".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error = anyhow::anyhow!("web_markdown fetch failed: non-success status: 404 Not Found");

        assert_eq!(web_crawler_fallback_reason(&args, &error), None);
    }

    #[test]
    fn web_crawler_does_not_fallback_for_reddit_not_found() {
        let args = WebMarkdownArgs {
            url: Some("https://www.reddit.com/r/LocalLLaMA/comments/missing/thread/".to_string()),
            ..WebMarkdownArgs::default()
        };
        let error = anyhow::anyhow!(
            "reddit rss fast-path failed: reddit rss returned non-success status: 404 Not Found"
        );

        assert_eq!(web_crawler_fallback_reason(&args, &error), None);
    }
}

/// Capability module for Tavily search/extract tools.
#[cfg(oxide_module_tool_tavily)]
pub struct TavilyToolModule;

#[cfg(oxide_module_tool_tavily)]
impl TavilyToolModule {
    fn provider(&self) -> Option<TavilyProvider> {
        if !crate::config::is_tavily_enabled() {
            return None;
        }

        match std::env::var("TAVILY_API_KEY") {
            Ok(tavily_key) if !tavily_key.trim().is_empty() => {
                match TavilyProvider::new(&tavily_key) {
                    Ok(provider) => Some(provider),
                    Err(error) => {
                        tracing::warn!(error = %error, "Tavily provider initialization failed");
                        None
                    }
                }
            }
            Ok(_) => {
                tracing::warn!(
                    "Tavily enabled but TAVILY_API_KEY is empty; provider not registered"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    "Tavily enabled but TAVILY_API_KEY is not set; provider not registered"
                );
                None
            }
        }
    }
}

#[cfg(oxide_module_tool_tavily)]
impl ToolModule for TavilyToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tavily")
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        let executors = self
            .provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default();

        // When CRW is enabled, CRW owns `web_search`; keep Tavily's `web_extract` only.
        #[cfg(oxide_module_tool_crw)]
        if crate::config::is_crw_enabled() {
            return executors
                .into_iter()
                .filter(|executor| executor.name().as_str() != "web_search")
                .collect();
        }

        executors
    }
}

/// Capability module for Brave Search API web search.
#[cfg(oxide_module_tool_brave_search)]
pub struct BraveSearchToolModule;

#[cfg(oxide_module_tool_brave_search)]
impl BraveSearchToolModule {
    fn provider(&self) -> Option<BraveSearchProvider> {
        if !crate::config::is_brave_search_enabled() {
            return None;
        }

        match BraveSearchProvider::new_from_config() {
            Ok(provider) => Some(provider),
            Err(error) => {
                tracing::warn!(error = %error, "Brave Search provider initialization failed");
                None
            }
        }
    }
}

#[cfg(oxide_module_tool_brave_search)]
impl ToolModule for BraveSearchToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/brave-search")
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for CRW-backed web search.
#[cfg(oxide_module_tool_crw)]
pub struct CrwSearchToolModule;

#[cfg(oxide_module_tool_crw)]
impl CrwSearchToolModule {
    fn provider(&self) -> Option<Arc<CrwProvider>> {
        if !crate::config::is_crw_enabled() {
            return None;
        }

        match CrwProvider::new() {
            Ok(provider) => Some(Arc::new(provider)),
            Err(error) => {
                tracing::warn!(error = %error, "CRW provider initialization failed");
                None
            }
        }
    }
}

#[cfg(oxide_module_tool_crw)]
impl ToolModule for CrwSearchToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/crw")
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

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxLifecycleProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}
