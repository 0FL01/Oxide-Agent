//! Capability-oriented tool modules.

#[cfg(any(
    feature = "tool-agents-md",
    feature = "tool-reminder",
    feature = "tool-wiki-memory"
))]
use super::provider_runtime_executors;
use super::ToolExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::{SandboxProvider, TodoList};
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
#[cfg(feature = "tool-file-delivery")]
use crate::agent::providers::FileHosterProvider;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video",
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
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
#[cfg(feature = "integration-mcp-mattermost")]
use crate::agent::providers::{MattermostMcpConfig, MattermostMcpProvider};
#[cfg(feature = "tool-reminder")]
use crate::agent::providers::{ReminderContext, ReminderProvider};
#[cfg(feature = "tool-tts-silero")]
use crate::agent::providers::{SileroTtsConfig, SileroTtsProvider};
#[cfg(feature = "tool-agents-md")]
use crate::storage::StorageProvider;

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

/// Runtime context passed to tool capability modules.
pub struct ToolModuleContext {
    todos: Arc<Mutex<TodoList>>,
    sandbox_scope: SandboxScope,
    sandbox_provider: Arc<SandboxProvider>,
    llm_client: Arc<LlmClient>,
    settings: Arc<AgentSettings>,
    browser_use_profile_scope: Option<String>,
    #[cfg(feature = "tool-agents-md")]
    agents_md_context: Option<AgentsMdModuleContext>,
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
    /// Shared sandbox provider.
    pub sandbox_provider: Arc<SandboxProvider>,
    /// Shared LLM client.
    pub llm_client: Arc<LlmClient>,
    /// Shared agent settings.
    pub settings: Arc<AgentSettings>,
    /// Optional Browser Use profile scope.
    pub browser_use_profile_scope: Option<String>,
    /// Optional AGENTS.md context.
    #[cfg(feature = "tool-agents-md")]
    pub agents_md_context: Option<AgentsMdModuleContext>,
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
            sandbox_provider: parts.sandbox_provider,
            llm_client: parts.llm_client,
            settings: parts.settings,
            browser_use_profile_scope: parts.browser_use_profile_scope,
            #[cfg(feature = "tool-agents-md")]
            agents_md_context: parts.agents_md_context,
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

    /// Shared sandbox provider for modules that own sandbox tools.
    #[must_use]
    pub fn sandbox_provider(&self) -> Arc<SandboxProvider> {
        Arc::clone(&self.sandbox_provider)
    }

    /// Shared sandbox provider as a legacy provider trait object.
    #[must_use]
    pub fn sandbox_provider_dyn(&self) -> Arc<dyn ToolProvider> {
        Arc::<SandboxProvider>::clone(&self.sandbox_provider)
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

/// Capability module for external file delivery from sandbox files.
#[cfg(feature = "tool-file-delivery")]
pub struct FileDeliveryToolModule;

#[cfg(feature = "tool-file-delivery")]
impl ToolModule for FileDeliveryToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/file-delivery")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        Some(Box::new(FileHosterProvider::new(ctx.sandbox_scope())))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
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
            .map(|provider| provider_runtime_executors(Arc::new(provider), ctx.progress_tx()))
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
            .map(|provider| provider_runtime_executors(Arc::new(provider), ctx.progress_tx()))
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
            .map(|provider| provider_runtime_executors(Arc::new(provider), ctx.progress_tx()))
            .unwrap_or_default()
    }
}

#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
fn media_file_provider(ctx: &ToolModuleContext) -> Arc<dyn ToolProvider> {
    Arc::new(MediaFileProvider::new(
        ctx.llm_client(),
        ctx.sandbox_scope(),
    ))
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
        Some(Box::new(FilteredToolProvider::new(
            media_file_provider(ctx),
            &["transcribe_audio_file"],
        )))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
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
        Some(Box::new(FilteredToolProvider::new(
            media_file_provider(ctx),
            &["describe_image_file"],
        )))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
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
        Some(Box::new(FilteredToolProvider::new(
            media_file_provider(ctx),
            &["describe_video_file"],
        )))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for the Browser Use sidecar tools.
#[cfg(feature = "tool-browser-use")]
pub struct BrowserUseToolModule;

#[cfg(feature = "tool-browser-use")]
impl ToolModule for BrowserUseToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/browser-use")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
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
                provider = provider.with_sandbox_scope(ctx.sandbox_scope());
                Some(Box::new(provider))
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

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for Jira MCP tools.
#[cfg(feature = "integration-mcp-jira")]
pub struct JiraMcpToolModule;

#[cfg(feature = "integration-mcp-jira")]
impl ToolModule for JiraMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-jira")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
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
                Some(Box::new(provider))
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

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for Mattermost MCP tools.
#[cfg(feature = "integration-mcp-mattermost")]
pub struct MattermostMcpToolModule;

#[cfg(feature = "integration-mcp-mattermost")]
impl ToolModule for MattermostMcpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("integration/mcp-mattermost")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
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
                Some(Box::new(provider))
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

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
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
        Vec::new()
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
        Vec::new()
    }
}

/// Capability module for Tavily search/extract tools.
#[cfg(feature = "tool-tavily")]
pub struct TavilyToolModule;

#[cfg(feature = "tool-tavily")]
impl ToolModule for TavilyToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tavily")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        if !crate::config::is_tavily_enabled() {
            return None;
        }

        match std::env::var("TAVILY_API_KEY") {
            Ok(tavily_key) if !tavily_key.trim().is_empty() => {
                match TavilyProvider::new(&tavily_key) {
                    Ok(provider) => Some(Box::new(provider)),
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

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for SearXNG web search.
#[cfg(feature = "tool-searxng")]
pub struct SearxngToolModule;

#[cfg(feature = "tool-searxng")]
impl ToolModule for SearxngToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/searxng")
    }

    fn legacy_provider(&self, _ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        if !crate::config::is_searxng_enabled() {
            return None;
        }

        match crate::config::get_searxng_url() {
            Some(url) if !url.trim().is_empty() => match SearxngProvider::new(&url) {
                Ok(provider) => Some(Box::new(provider)),
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

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for Kokoro English text-to-speech tools.
#[cfg(feature = "tool-tts-kokoro")]
pub struct KokoroTtsToolModule;

#[cfg(feature = "tool-tts-kokoro")]
impl ToolModule for KokoroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-kokoro")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
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
            KokoroTtsProvider::from_config(config).with_sandbox_scope(ctx.sandbox_scope());
        if let Some(tx) = ctx.progress_tx() {
            provider = provider.with_progress_tx(tx);
        }

        let base_url = provider.base_url().to_string();
        tracing::debug!(url = %base_url, "Kokoro TTS provider registered");
        Some(Box::new(provider))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for Silero Russian text-to-speech tools.
#[cfg(feature = "tool-tts-silero")]
pub struct SileroTtsToolModule;

#[cfg(feature = "tool-tts-silero")]
impl ToolModule for SileroTtsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/tts-silero")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
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
            SileroTtsProvider::from_config(config).with_sandbox_scope(ctx.sandbox_scope());
        if let Some(tx) = ctx.progress_tx() {
            provider = provider.with_progress_tx(tx);
        }

        let base_url = provider.base_url().to_string();
        tracing::debug!(url = %base_url, "Silero TTS provider registered");
        Some(Box::new(provider))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
    }
}

/// Capability module for yt-dlp media tools.
#[cfg(feature = "tool-ytdlp")]
pub struct YtdlpToolModule;

#[cfg(feature = "tool-ytdlp")]
impl ToolModule for YtdlpToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/ytdlp")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        let provider = if let Some(tx) = ctx.progress_tx() {
            YtdlpProvider::new(ctx.sandbox_scope()).with_progress_tx(tx)
        } else {
            YtdlpProvider::new(ctx.sandbox_scope())
        };
        Some(Box::new(provider))
    }

    fn tool_runtime_executors(&self, _ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Vec::new()
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
        sandbox_legacy_provider(ctx, &["execute_command"])
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(ctx, &["execute_command"])
    }
}

/// Capability module for sandbox file operations and file delivery.
#[cfg(feature = "tool-sandbox-fileops")]
pub struct SandboxFileOpsToolModule;

#[cfg(feature = "tool-sandbox-fileops")]
impl ToolModule for SandboxFileOpsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-fileops")
    }

    fn legacy_provider(&self, ctx: &ToolModuleContext) -> Option<Box<dyn ToolProvider>> {
        sandbox_legacy_provider(
            ctx,
            &["write_file", "read_file", "send_file_to_user", "list_files"],
        )
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            ctx,
            &["write_file", "read_file", "send_file_to_user", "list_files"],
        )
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
        sandbox_legacy_provider(ctx, &["recreate_sandbox"])
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(ctx, &["recreate_sandbox"])
    }
}

#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
))]
fn sandbox_legacy_provider(
    ctx: &ToolModuleContext,
    owned_tool_names: &'static [&'static str],
) -> Option<Box<dyn ToolProvider>> {
    Some(Box::new(FilteredToolProvider::new(
        ctx.sandbox_provider_dyn(),
        owned_tool_names,
    )))
}

#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
))]
fn sandbox_tool_runtime_executors(
    ctx: &ToolModuleContext,
    owned_tool_names: &[&str],
) -> Vec<Arc<dyn ToolExecutor>> {
    ctx.sandbox_provider()
        .tool_runtime_executors()
        .into_iter()
        .filter(|executor| {
            let name = executor.name();
            owned_tool_names.iter().any(|owned| *owned == name.as_str())
        })
        .collect()
}
