//! Capability-oriented tool modules.

use super::ToolExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
#[cfg(feature = "tool-sandbox-exec")]
use crate::agent::providers::SandboxExecProvider;
#[cfg(feature = "tool-sandbox-fileops")]
use crate::agent::providers::SandboxFileOpsProvider;
#[cfg(feature = "tool-sandbox-recreate")]
use crate::agent::providers::SandboxLifecycleProvider;
use crate::agent::providers::{SandboxRuntime, TodoList};
#[cfg(feature = "tool-wiki-memory")]
use crate::agent::session::AgentMemoryScope;
#[cfg(feature = "tool-wiki-memory")]
use crate::agent::wiki_memory::WikiStore;
use crate::capabilities::ModuleId;
use crate::config::AgentSettings;
use crate::llm::LlmClient;
use crate::sandbox::SandboxScope;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};

#[cfg(feature = "tool-agents-md")]
use crate::agent::providers::AgentsMdProvider;
#[cfg(feature = "tool-browser-use")]
use crate::agent::providers::BrowserUseProvider;
#[cfg(feature = "tool-compression")]
use crate::agent::providers::CompressionProvider;
#[cfg(feature = "tool-delegation")]
use crate::agent::providers::DelegationProvider;
#[cfg(feature = "tool-file-delivery")]
use crate::agent::providers::FileHosterProvider;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
use crate::agent::providers::FilteredToolProvider;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
use crate::agent::providers::MediaFileProvider;
#[cfg(feature = "tool-searxng")]
use crate::agent::providers::SearxngProvider;
#[cfg(feature = "tool-stack-logs")]
use crate::agent::providers::StackLogsProvider;
#[cfg(feature = "tool-tavily")]
use crate::agent::providers::TavilyProvider;
#[cfg(feature = "tool-todos")]
use crate::agent::providers::TodosProvider;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::providers::WebFetchMdProvider;
#[cfg(feature = "tool-wiki-memory")]
use crate::agent::providers::WikiMemoryProvider;
#[cfg(feature = "tool-ytdlp")]
use crate::agent::providers::YtdlpProvider;
#[cfg(feature = "integration-mcp-jira")]
use crate::agent::providers::{JiraMcpConfig, JiraMcpProvider};
#[cfg(feature = "tool-tts-kokoro")]
use crate::agent::providers::{KokoroTtsProvider, TtsConfig};
#[cfg(feature = "manager-control-plane")]
use crate::agent::providers::{ManagerControlPlaneProvider, ManagerTopicLifecycle};
#[cfg(feature = "integration-mcp-mattermost")]
use crate::agent::providers::{MattermostMcpConfig, MattermostMcpProvider};
#[cfg(feature = "tool-reminder")]
use crate::agent::providers::{ReminderContext, ReminderProvider};
#[cfg(feature = "tool-tts-silero")]
use crate::agent::providers::{SileroTtsConfig, SileroTtsProvider};
#[cfg(feature = "integration-ssh-mcp")]
use crate::agent::providers::{SshApprovalRegistry, SshMcpProvider};
#[cfg(any(
    feature = "tool-agents-md",
    feature = "manager-control-plane",
    feature = "integration-ssh-mcp"
))]
use crate::storage::StorageProvider;
#[cfg(feature = "integration-ssh-mcp")]
use crate::storage::TopicInfraConfigRecord;

/// Topic-scoped context required by the AGENTS.md tools.
#[cfg(feature = "tool-agents-md")]
#[derive(Clone)]
pub struct AgentsMdModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
}

#[cfg(feature = "tool-agents-md")]
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
#[cfg(feature = "manager-control-plane")]
#[derive(Clone)]
pub struct ManagerControlPlaneModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

#[cfg(feature = "manager-control-plane")]
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
#[cfg(feature = "integration-ssh-mcp")]
#[derive(Clone)]
pub struct SshMcpModuleContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: TopicInfraConfigRecord,
    approvals: SshApprovalRegistry,
}

#[cfg(feature = "integration-ssh-mcp")]
impl SshMcpModuleContext {
    /// Create a context for topic-scoped SSH MCP tools.
    #[must_use]
    pub fn new(
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: TopicInfraConfigRecord,
        approvals: SshApprovalRegistry,
    ) -> Self {
        Self {
            storage,
            user_id,
            topic_id,
            config,
            approvals,
        }
    }
}

/// Runtime context passed to tool capability modules.
pub struct ToolModuleContext {
    todos: Arc<Mutex<TodoList>>,
    sandbox_scope: SandboxScope,
    sandbox_runtime: Arc<SandboxRuntime>,
    llm_client: Arc<LlmClient>,
    settings: Arc<AgentSettings>,
    browser_use_profile_scope: Option<String>,
    #[cfg(feature = "tool-agents-md")]
    agents_md_context: Option<AgentsMdModuleContext>,
    #[cfg(feature = "manager-control-plane")]
    manager_control_plane_context: Option<ManagerControlPlaneModuleContext>,
    #[cfg(feature = "integration-ssh-mcp")]
    ssh_mcp_context: Option<SshMcpModuleContext>,
    #[cfg(feature = "tool-reminder")]
    reminder_context: Option<ReminderContext>,
    #[cfg(feature = "tool-wiki-memory")]
    wiki_memory_store: Option<WikiStore>,
    #[cfg(feature = "tool-wiki-memory")]
    memory_scope: AgentMemoryScope,
    progress_tx: Option<Sender<AgentEvent>>,
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
    /// Optional Browser Use profile scope.
    pub browser_use_profile_scope: Option<String>,
    /// Optional AGENTS.md context.
    #[cfg(feature = "tool-agents-md")]
    pub agents_md_context: Option<AgentsMdModuleContext>,
    /// Optional manager control-plane context.
    #[cfg(feature = "manager-control-plane")]
    pub manager_control_plane_context: Option<ManagerControlPlaneModuleContext>,
    /// Optional topic infrastructure context for SSH MCP tools.
    #[cfg(feature = "integration-ssh-mcp")]
    pub ssh_mcp_context: Option<SshMcpModuleContext>,
    /// Optional reminder context.
    #[cfg(feature = "tool-reminder")]
    pub reminder_context: Option<ReminderContext>,
    /// Optional durable wiki memory store.
    #[cfg(feature = "tool-wiki-memory")]
    pub wiki_memory_store: Option<WikiStore>,
    /// Stable memory scope for wiki memory tools.
    #[cfg(feature = "tool-wiki-memory")]
    pub memory_scope: AgentMemoryScope,
    /// Optional progress sender.
    pub progress_tx: Option<Sender<AgentEvent>>,
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
            browser_use_profile_scope: parts.browser_use_profile_scope,
            #[cfg(feature = "tool-agents-md")]
            agents_md_context: parts.agents_md_context,
            #[cfg(feature = "manager-control-plane")]
            manager_control_plane_context: parts.manager_control_plane_context,
            #[cfg(feature = "integration-ssh-mcp")]
            ssh_mcp_context: parts.ssh_mcp_context,
            #[cfg(feature = "tool-reminder")]
            reminder_context: parts.reminder_context,
            #[cfg(feature = "tool-wiki-memory")]
            wiki_memory_store: parts.wiki_memory_store,
            #[cfg(feature = "tool-wiki-memory")]
            memory_scope: parts.memory_scope,
            progress_tx: parts.progress_tx,
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

    /// Optional Browser Use profile scope derived from topic/reminder context.
    #[must_use]
    pub fn browser_use_profile_scope(&self) -> Option<String> {
        self.browser_use_profile_scope.clone()
    }

    /// Optional context for topic-scoped AGENTS.md tools.
    #[cfg(feature = "tool-agents-md")]
    #[must_use]
    pub fn agents_md_context(&self) -> Option<AgentsMdModuleContext> {
        self.agents_md_context.clone()
    }

    /// Optional context for manager control-plane tools.
    #[cfg(feature = "manager-control-plane")]
    #[must_use]
    pub fn manager_control_plane_context(&self) -> Option<ManagerControlPlaneModuleContext> {
        self.manager_control_plane_context.clone()
    }

    /// Optional context for topic-scoped SSH MCP tools.
    #[cfg(feature = "integration-ssh-mcp")]
    #[must_use]
    pub fn ssh_mcp_context(&self) -> Option<SshMcpModuleContext> {
        self.ssh_mcp_context.clone()
    }

    /// Optional context for reminder tools.
    #[cfg(feature = "tool-reminder")]
    #[must_use]
    pub fn reminder_context(&self) -> Option<ReminderContext> {
        self.reminder_context.clone()
    }

    /// Optional durable wiki memory store.
    #[cfg(feature = "tool-wiki-memory")]
    #[must_use]
    pub fn wiki_memory_store(&self) -> Option<WikiStore> {
        self.wiki_memory_store.clone()
    }

    /// Stable memory scope used by wiki memory tools.
    #[cfg(feature = "tool-wiki-memory")]
    #[must_use]
    pub fn memory_scope(&self) -> AgentMemoryScope {
        self.memory_scope.clone()
    }

    /// Optional progress sender for modules that emit progress events.
    #[must_use]
    pub fn progress_tx(&self) -> Option<Sender<AgentEvent>> {
        self.progress_tx.clone()
    }
}

/// Tool capability module.
pub trait ToolModule {
    /// Stable module ID corresponding to the compiled capability manifest.
    fn module_id(&self) -> ModuleId;

    /// Builds the legacy provider owned by this module.
    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>>;

    /// Builds typed tool executors owned by this module.
    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>>;
}

/// Capability module for the runner-handled `compress` tool.
#[cfg(feature = "tool-compression")]
pub struct CompressionToolModule;

#[cfg(feature = "tool-compression")]
impl ToolModule for CompressionToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/compression")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(CompressionProvider::new()))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for chat and external file delivery from sandbox files.
#[cfg(feature = "tool-file-delivery")]
pub struct FileDeliveryToolModule;

#[cfg(feature = "tool-file-delivery")]
impl ToolModule for FileDeliveryToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/file-delivery")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(FileHosterProvider::from_runtime(
            ctx.sandbox_runtime(),
        )))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(FileHosterProvider::from_runtime(ctx.sandbox_runtime()))
            .tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for topic-scoped AGENTS.md self-editing tools.
#[cfg(feature = "tool-agents-md")]
pub struct AgentsMdToolModule;

#[cfg(feature = "tool-agents-md")]
impl AgentsMdToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<AgentsMdProvider> {
        ctx.agents_md_context().map(|agents_md| {
            AgentsMdProvider::new(agents_md.storage, agents_md.user_id, agents_md.topic_id)
        })
    }
}

#[cfg(feature = "tool-agents-md")]
impl ToolModule for AgentsMdToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/agents-md")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for sub-agent delegation tools.
#[cfg(feature = "tool-delegation")]
pub struct DelegationToolModule;

#[cfg(feature = "tool-delegation")]
impl DelegationToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> DelegationProvider {
        let mut provider =
            DelegationProvider::new(ctx.llm_client(), ctx.sandbox_scope(), ctx.settings());

        #[cfg(feature = "tool-agents-md")]
        if let Some(agents_md) = ctx.agents_md_context() {
            provider = provider.with_topic_agents_md_context(
                agents_md.storage,
                agents_md.user_id,
                agents_md.topic_id,
            );
        }

        if let Some(profile_scope) = ctx.browser_use_profile_scope() {
            provider = provider.with_browser_use_profile_scope(profile_scope);
        }

        provider
    }
}

#[cfg(feature = "tool-delegation")]
impl ToolModule for DelegationToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/delegation")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(self.provider(ctx)))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for manager control-plane tools.
#[cfg(feature = "manager-control-plane")]
pub struct ManagerControlPlaneToolModule;

#[cfg(feature = "manager-control-plane")]
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

#[cfg(feature = "manager-control-plane")]
impl ToolModule for ManagerControlPlaneToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("manager/control-plane")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for topic-scoped SSH MCP tools.
#[cfg(feature = "integration-ssh-mcp")]
pub struct SshMcpToolModule;

#[cfg(feature = "integration-ssh-mcp")]
impl SshMcpToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<SshMcpProvider> {
        let ssh = ctx.ssh_mcp_context()?;
        Some(SshMcpProvider::new(
            ssh.storage,
            ssh.user_id,
            ssh.topic_id,
            ssh.config,
            ssh.approvals,
        ))
    }
}

#[cfg(feature = "integration-ssh-mcp")]
impl ToolModule for SshMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/ssh-mcp")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for reminder scheduling tools.
#[cfg(feature = "tool-reminder")]
pub struct ReminderToolModule;

#[cfg(feature = "tool-reminder")]
impl ReminderToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<ReminderProvider> {
        ctx.reminder_context().map(ReminderProvider::new)
    }
}

#[cfg(feature = "tool-reminder")]
impl ToolModule for ReminderToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/reminder")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for scoped durable wiki memory tools.
#[cfg(feature = "tool-wiki-memory")]
pub struct WikiMemoryToolModule;

#[cfg(feature = "tool-wiki-memory")]
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

#[cfg(feature = "tool-wiki-memory")]
impl ToolModule for WikiMemoryToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/wiki-memory")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
fn media_file_provider(ctx: &ToolModuleContext) -> MediaFileProvider {
    MediaFileProvider::from_runtime(ctx.llm_client(), ctx.sandbox_runtime())
}

/// Capability module for audio file transcription.
#[cfg(feature = "tool-media-audio")]
pub struct MediaAudioToolModule;

#[cfg(feature = "tool-media-audio")]
impl ToolModule for MediaAudioToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-audio")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        let provider: Arc<dyn ToolProvider> = Arc::new(media_file_provider(ctx));
        Some(Box::new(FilteredToolProvider::new(
            provider,
            &["transcribe_audio_file"],
        )))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["transcribe_audio_file"])
    }
}

/// Capability module for image file description.
#[cfg(feature = "tool-media-image")]
pub struct MediaImageToolModule;

#[cfg(feature = "tool-media-image")]
impl ToolModule for MediaImageToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-image")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        let provider: Arc<dyn ToolProvider> = Arc::new(media_file_provider(ctx));
        Some(Box::new(FilteredToolProvider::new(
            provider,
            &["describe_image_file"],
        )))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["describe_image_file"])
    }
}

/// Capability module for video file description.
#[cfg(feature = "tool-media-video")]
pub struct MediaVideoToolModule;

#[cfg(feature = "tool-media-video")]
impl ToolModule for MediaVideoToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/media-video")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        let provider: Arc<dyn ToolProvider> = Arc::new(media_file_provider(ctx));
        Some(Box::new(FilteredToolProvider::new(
            provider,
            &["describe_video_file"],
        )))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(media_file_provider(ctx)).tool_runtime_executors_for(&["describe_video_file"])
    }
}

/// Capability module for the Browser Use sidecar tools.
#[cfg(feature = "tool-browser-use")]
pub struct BrowserUseToolModule;

#[cfg(feature = "tool-browser-use")]
impl BrowserUseToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<BrowserUseProvider> {
        // NOTE: Browser Use is disabled until a quality vision-capable agent model
        // is available at a reasonable price-per-token. To re-enable, set
        // `BROWSER_USE_URL` (and optionally `BROWSER_USE_MODEL_ID` /
        // `BROWSER_USE_MODEL_PROVIDER`). See `docs/browser-use.md`.
        if !crate::config::is_browser_use_enabled() {
            return None;
        }

        match crate::config::get_browser_use_url() {
            Some(url) if !url.trim().is_empty() => {
                let mut provider = BrowserUseProvider::new(&url, ctx.settings());
                if let Some(profile_scope) = ctx.browser_use_profile_scope() {
                    provider = provider.with_profile_scope(profile_scope);
                }
                provider = provider.with_sandbox_runtime(ctx.sandbox_runtime());
                Some(provider)
            }
            Some(_) => {
                tracing::warn!(
                    "Browser Use enabled but BROWSER_USE_URL is empty; provider not registered"
                );
                None
            }
            None => {
                tracing::warn!(
                    "Browser Use enabled but BROWSER_USE_URL is not set; provider not registered"
                );
                None
            }
        }
    }
}

#[cfg(feature = "tool-browser-use")]
impl ToolModule for BrowserUseToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/browser-use")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Jira MCP tools.
#[cfg(feature = "integration-mcp-jira")]
pub struct JiraMcpToolModule;

#[cfg(feature = "integration-mcp-jira")]
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

#[cfg(feature = "integration-mcp-jira")]
impl ToolModule for JiraMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-jira")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider()
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Mattermost MCP tools.
#[cfg(feature = "integration-mcp-mattermost")]
pub struct MattermostMcpToolModule;

#[cfg(feature = "integration-mcp-mattermost")]
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

#[cfg(feature = "integration-mcp-mattermost")]
impl ToolModule for MattermostMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-mattermost")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider()
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for compose-stack log tools.
#[cfg(feature = "tool-stack-logs")]
pub struct StackLogsToolModule;

#[cfg(feature = "tool-stack-logs")]
impl ToolModule for StackLogsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/stack-logs")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(StackLogsProvider::new()))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(StackLogsProvider::new()).tool_runtime_executors()
    }
}

/// Capability module for one-shot URL-to-Markdown fetches.
#[cfg(feature = "tool-webfetch-md")]
pub struct WebFetchMdToolModule;

#[cfg(feature = "tool-webfetch-md")]
impl ToolModule for WebFetchMdToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/webfetch-md")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(WebFetchMdProvider::new()))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(WebFetchMdProvider::new()).tool_runtime_executors()
    }
}

/// Capability module for Tavily search/extract tools.
#[cfg(feature = "tool-tavily")]
pub struct TavilyToolModule;

#[cfg(feature = "tool-tavily")]
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

#[cfg(feature = "tool-tavily")]
impl ToolModule for TavilyToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tavily")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider()
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for SearXNG web search.
#[cfg(feature = "tool-searxng")]
pub struct SearxngToolModule;

#[cfg(feature = "tool-searxng")]
impl SearxngToolModule {
    fn provider(&self) -> Option<SearxngProvider> {
        if !crate::config::is_searxng_enabled() {
            return None;
        }

        match crate::config::get_searxng_url() {
            Some(url) if !url.trim().is_empty() => match SearxngProvider::new(&url) {
                Ok(provider) => Some(provider),
                Err(error) => {
                    tracing::warn!(error = %error, "SearXNG provider initialization failed");
                    None
                }
            },
            Some(_) => {
                tracing::warn!("SearXNG enabled but SEARXNG_URL is empty; provider not registered");
                None
            }
            None => {
                tracing::warn!(
                    "SearXNG enabled but SEARXNG_URL is not set; provider not registered"
                );
                None
            }
        }
    }
}

#[cfg(feature = "tool-searxng")]
impl ToolModule for SearxngToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/searxng")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider()
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider()
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Kokoro English text-to-speech tools.
#[cfg(feature = "tool-tts-kokoro")]
pub struct KokoroTtsToolModule;

#[cfg(feature = "tool-tts-kokoro")]
impl KokoroTtsToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<KokoroTtsProvider> {
        let config = TtsConfig::from_env();

        if let Ok(url) = std::env::var("KOKORO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "TTS provider disabled: KOKORO_TTS_URL is explicitly set to empty string"
                );
                return None;
            }
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

#[cfg(feature = "tool-tts-kokoro")]
impl ToolModule for KokoroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-kokoro")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for Silero Russian text-to-speech tools.
#[cfg(feature = "tool-tts-silero")]
pub struct SileroTtsToolModule;

#[cfg(feature = "tool-tts-silero")]
impl SileroTtsToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> Option<SileroTtsProvider> {
        let config = SileroTtsConfig::from_env();

        if let Ok(url) = std::env::var("SILERO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "Silero TTS provider disabled: SILERO_TTS_URL is explicitly set to empty string"
                );
                return None;
            }
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

#[cfg(feature = "tool-tts-silero")]
impl ToolModule for SileroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-silero")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        self.provider(ctx)
            .map(|provider| Box::new(provider) as Box<dyn ToolProvider>)
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        self.provider(ctx)
            .map(|provider| Arc::new(provider).tool_runtime_executors())
            .unwrap_or_default()
    }
}

/// Capability module for yt-dlp media tools.
#[cfg(feature = "tool-ytdlp")]
pub struct YtdlpToolModule;

#[cfg(feature = "tool-ytdlp")]
impl YtdlpToolModule {
    fn provider(&self, ctx: &ToolModuleContext) -> YtdlpProvider {
        if let Some(tx) = ctx.progress_tx() {
            YtdlpProvider::from_runtime(ctx.sandbox_runtime()).with_progress_tx(tx)
        } else {
            YtdlpProvider::from_runtime(ctx.sandbox_runtime())
        }
    }
}

#[cfg(feature = "tool-ytdlp")]
impl ToolModule for YtdlpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/ytdlp")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(self.provider(ctx)))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(self.provider(ctx)).tool_runtime_executors()
    }
}

/// Capability module for the `write_todos` typed runtime tool.
#[cfg(feature = "tool-todos")]
pub struct TodosToolModule;

#[cfg(feature = "tool-todos")]
impl ToolModule for TodosToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/todos")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(TodosProvider::new(ctx.todos())))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(TodosProvider::new(ctx.todos())).tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for sandbox command execution.
#[cfg(feature = "tool-sandbox-exec")]
pub struct SandboxExecToolModule;

#[cfg(feature = "tool-sandbox-exec")]
impl ToolModule for SandboxExecToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-exec")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(SandboxExecProvider::new(ctx.sandbox_runtime())))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxExecProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}

/// Capability module for sandbox file operations.
#[cfg(feature = "tool-sandbox-fileops")]
pub struct SandboxFileOpsToolModule;

#[cfg(feature = "tool-sandbox-fileops")]
impl ToolModule for SandboxFileOpsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-fileops")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(SandboxFileOpsProvider::new(ctx.sandbox_runtime())))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxFileOpsProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}

/// Capability module for sandbox recreation.
#[cfg(feature = "tool-sandbox-recreate")]
pub struct SandboxRecreateToolModule;

#[cfg(feature = "tool-sandbox-recreate")]
impl ToolModule for SandboxRecreateToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-recreate")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(SandboxLifecycleProvider::new(
            ctx.sandbox_runtime(),
        )))
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(SandboxLifecycleProvider::new(ctx.sandbox_runtime())).tool_runtime_executors()
    }
}
