use super::AgentExecutor;
use crate::agent::persistent_memory::LlmMemoryEmbeddingGenerator;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{
    AgentsMdProvider, CompressionProvider, DelegationProvider, FileHosterProvider,
    KokoroTtsProvider, ManagerControlPlaneProvider, MediaFileProvider, MemoryProvider,
    ReminderProvider, SandboxProvider, StackLogsProvider, TodoList, TodosProvider, YtdlpProvider,
};
use crate::agent::registry::ToolRegistry;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

#[cfg(feature = "browser_use")]
use crate::agent::providers::BrowserUseProvider;
#[cfg(feature = "crawl4ai")]
use crate::agent::providers::Crawl4aiProvider;
#[cfg(feature = "searxng")]
use crate::agent::providers::SearxngProvider;
#[cfg(feature = "tavily")]
use crate::agent::providers::TavilyProvider;

impl AgentExecutor {
    /// Build the currently exposed tool definitions for this executor state.
    #[must_use]
    pub fn current_tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let registry = self.build_tool_registry(todos_arc, None);
        self.execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools())
    }

    pub(super) fn build_tool_registry(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();

        // Core providers: todos, sandbox, filehoster, media file analysis, ytdlp, delegation
        self.register_core_providers(&mut registry, todos_arc, progress_tx);

        // Topic-scoped providers: agents_md, manager, ssh, reminders
        self.register_topic_providers(&mut registry);

        // Feature-gated MCP, search, and browser automation providers
        self.register_mcp_providers(&mut registry);
        self.register_search_providers(&mut registry);
        self.register_browser_providers(&mut registry);

        // Optional TTS providers.
        self.register_kokoro_tts_provider(&mut registry, progress_tx);
        self.register_silero_tts_provider(&mut registry, progress_tx);

        registry
    }

    fn register_core_providers(
        &self,
        registry: &mut ToolRegistry,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));

        let sandbox_scope = self.session.sandbox_scope().clone();
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(sandbox_provider));
        registry.register(Box::new(CompressionProvider::new()));
        registry.register(Box::new(StackLogsProvider::new()));
        registry.register(Box::new(FileHosterProvider::new(sandbox_scope.clone())));
        registry.register(Box::new(MediaFileProvider::new(
            self.runner.llm_client(),
            sandbox_scope.clone(),
        )));

        let ytdlp_provider = if let Some(tx) = progress_tx {
            YtdlpProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(ytdlp_provider));

        if let Some(store) = &self.memory_store {
            let mut provider = MemoryProvider::new_with_store(
                Arc::clone(store),
                self.memory_artifact_storage.clone(),
                self.session.memory_scope().clone(),
            );
            if self.runner.llm_client().is_embedding_available() {
                provider = provider.with_query_embedding_generator(Arc::new(
                    LlmMemoryEmbeddingGenerator::new(self.runner.llm_client()),
                ));
            }
            registry.register(Box::new(provider));
        }

        let mut delegation_provider = DelegationProvider::new(
            self.runner.llm_client(),
            sandbox_scope,
            Arc::clone(&self.settings),
        );
        if let Some(agents_md) = &self.agents_md {
            delegation_provider = delegation_provider.with_topic_agents_md_context(
                Arc::clone(&agents_md.storage),
                agents_md.user_id,
                agents_md.topic_id.clone(),
            );
        }
        if let Some(profile_scope) = self.browser_use_profile_scope() {
            delegation_provider = delegation_provider.with_browser_use_profile_scope(profile_scope);
        }
        registry.register(Box::new(delegation_provider));
    }

    #[cfg(feature = "browser_use")]
    pub(super) fn browser_use_profile_scope(&self) -> Option<String> {
        self.reminder_context
            .as_ref()
            .map(|context| context.context_key.clone())
            .or_else(|| {
                self.agents_md
                    .as_ref()
                    .map(|context| context.topic_id.clone())
            })
            .or_else(|| {
                self.topic_infra
                    .as_ref()
                    .map(|context| context.topic_id.clone())
            })
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    #[cfg(not(feature = "browser_use"))]
    pub(super) fn browser_use_profile_scope(&self) -> Option<String> {
        self.reminder_context
            .as_ref()
            .map(|context| context.context_key.clone())
            .or_else(|| {
                self.agents_md
                    .as_ref()
                    .map(|context| context.topic_id.clone())
            })
            .or_else(|| {
                self.topic_infra
                    .as_ref()
                    .map(|context| context.topic_id.clone())
            })
            .map(|scope| scope.trim().to_string())
            .filter(|scope| !scope.is_empty())
    }

    fn register_topic_providers(&self, registry: &mut ToolRegistry) {
        if let Some(agents_md) = &self.agents_md {
            registry.register(Box::new(AgentsMdProvider::new(
                Arc::clone(&agents_md.storage),
                agents_md.user_id,
                agents_md.topic_id.clone(),
            )));
        }

        if let Some(control_plane) = &self.manager_control_plane {
            let mut manager_provider = ManagerControlPlaneProvider::new(
                Arc::clone(&control_plane.storage),
                control_plane.user_id,
            );
            if let Some(topic_lifecycle) = &control_plane.topic_lifecycle {
                manager_provider =
                    manager_provider.with_topic_lifecycle(Arc::clone(topic_lifecycle));
            }
            registry.register(Box::new(manager_provider));
        }

        if let Some(topic_infra) = &self.topic_infra {
            registry.register(Box::new(crate::agent::providers::SshMcpProvider::new(
                Arc::clone(&topic_infra.storage),
                topic_infra.user_id,
                topic_infra.topic_id.clone(),
                topic_infra.config.clone(),
                topic_infra.approvals.clone(),
            )));
        }

        if let Some(reminder_context) = &self.reminder_context {
            registry.register(Box::new(ReminderProvider::new(reminder_context.clone())));
        }
    }

    #[cfg(feature = "jira")]
    fn register_jira_mcp_provider(registry: &mut ToolRegistry) {
        if let Some(config) = crate::agent::providers::JiraMcpConfig::from_env() {
            let binary_path = config.binary_path.clone();
            tracing::debug!(
                binary_path = %binary_path,
                jira_url_present = !config.jira_url.is_empty(),
                jira_email_present = !config.jira_email.is_empty(),
                jira_token_present = !config.jira_token.is_empty(),
                "Registering Jira MCP provider"
            );
            registry.register(Box::new(crate::agent::providers::JiraMcpProvider::new(
                config,
            )));
            tracing::debug!(binary_path = %binary_path, "Jira MCP provider registered");
        } else {
            tracing::warn!(
                "jira feature is enabled but JIRA_URL, JIRA_EMAIL, or JIRA_API_TOKEN is not set; \
                 Jira MCP provider will not be available. Set these env vars to enable it."
            );
        }
    }

    #[cfg(feature = "mattermost")]
    fn register_mattermost_mcp_provider(registry: &mut ToolRegistry) {
        if let Some(config) = crate::agent::providers::MattermostMcpConfig::from_env() {
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
            registry.register(Box::new(
                crate::agent::providers::MattermostMcpProvider::new(config),
            ));
            tracing::debug!(binary_path = %binary_path, "Mattermost MCP provider registered");
        } else {
            tracing::warn!(
                "mattermost feature is enabled but MATTERMOST_URL or MATTERMOST_TOKEN is not set; \
                 Mattermost MCP provider will not be available. Set these env vars to enable it."
            );
        }
    }

    fn register_mcp_providers(&self, _registry: &mut ToolRegistry) {
        #[cfg(feature = "jira")]
        Self::register_jira_mcp_provider(_registry);

        #[cfg(feature = "mattermost")]
        Self::register_mattermost_mcp_provider(_registry);
    }

    fn register_search_providers(&self, registry: &mut ToolRegistry) {
        #[cfg(feature = "tavily")]
        if crate::config::is_tavily_enabled() {
            if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
                if !tavily_key.trim().is_empty() {
                    if let Ok(provider) = TavilyProvider::new(&tavily_key) {
                        registry.register(Box::new(provider));
                    }
                } else {
                    warn!("Tavily enabled but TAVILY_API_KEY is empty; provider not registered");
                }
            } else {
                warn!("Tavily enabled but TAVILY_API_KEY is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "tavily"))]
        if crate::config::is_tavily_enabled() {
            tracing::warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(feature = "searxng")]
        if crate::config::is_searxng_enabled() {
            if let Some(url) = crate::config::get_searxng_url() {
                if !url.trim().is_empty() {
                    match SearxngProvider::new(&url) {
                        Ok(provider) => registry.register(Box::new(provider)),
                        Err(error) => {
                            warn!(error = %error, "SearXNG provider initialization failed")
                        }
                    }
                } else {
                    warn!("SearXNG enabled but SEARXNG_URL is empty; provider not registered");
                }
            } else {
                warn!("SearXNG enabled but SEARXNG_URL is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "searxng"))]
        if crate::config::is_searxng_enabled() {
            tracing::warn!("SearXNG enabled but feature not compiled in");
        }

        #[cfg(feature = "crawl4ai")]
        if crate::config::is_crawl4ai_enabled() {
            if let Some(url) = crate::config::get_crawl4ai_url() {
                if !url.trim().is_empty() {
                    registry.register(Box::new(Crawl4aiProvider::new(&url)));
                } else {
                    warn!("Crawl4AI enabled but CRAWL4AI_URL is empty; provider not registered");
                }
            } else {
                warn!("Crawl4AI enabled but CRAWL4AI_URL is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "crawl4ai"))]
        if crate::config::is_crawl4ai_enabled() {
            tracing::warn!("Crawl4AI enabled but feature not compiled in");
        }
    }

    fn register_browser_providers(&self, _registry: &mut ToolRegistry) {
        // NOTE: Browser Use is disabled until a quality vision-capable agent model
        // is available at a reasonable price-per-token. To re-enable, set
        // `BROWSER_USE_URL` (and optionally `BROWSER_USE_MODEL_ID` /
        // `BROWSER_USE_MODEL_PROVIDER`). See `docs/browser-use.md`.
        #[cfg(feature = "browser_use")]
        if crate::config::is_browser_use_enabled() {
            if let Some(url) = crate::config::get_browser_use_url() {
                if !url.trim().is_empty() {
                    let mut provider = BrowserUseProvider::new(&url, Arc::clone(&self.settings));
                    if let Some(profile_scope) = self.browser_use_profile_scope() {
                        provider = provider.with_profile_scope(profile_scope);
                    }
                    provider = provider.with_sandbox_scope(self.session.sandbox_scope().clone());
                    _registry.register(Box::new(provider));
                } else {
                    warn!(
                        "Browser Use enabled but BROWSER_USE_URL is empty; provider not registered"
                    );
                }
            } else {
                warn!(
                    "Browser Use enabled but BROWSER_USE_URL is not set; provider not registered"
                );
            }
        }
        #[cfg(not(feature = "browser_use"))]
        if crate::config::is_browser_use_enabled() {
            tracing::warn!("Browser Use enabled but feature not compiled in");
        }
    }

    fn register_kokoro_tts_provider(
        &self,
        registry: &mut ToolRegistry,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let config = crate::agent::providers::tts::TtsConfig::from_env();

        if let Ok(url) = std::env::var("KOKORO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "TTS provider disabled: KOKORO_TTS_URL is explicitly set to empty string"
                );
                return;
            }
        }

        tracing::debug!(url = %config.base_url, "Registering TTS provider");
        let sandbox_scope = self.session.sandbox_scope().clone();

        let provider = if let Some(tx) = progress_tx {
            KokoroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
                .with_progress_tx(tx.clone())
        } else {
            KokoroTtsProvider::from_config(config).with_sandbox_scope(sandbox_scope)
        };

        let base_url = provider.base_url().to_string();
        registry.register(Box::new(provider));
        tracing::debug!(url = %base_url, "Kokoro TTS provider registered");
    }

    fn register_silero_tts_provider(
        &self,
        registry: &mut ToolRegistry,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let config = crate::agent::providers::silero_tts::SileroTtsConfig::from_env();

        if let Ok(url) = std::env::var("SILERO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "Silero TTS provider disabled: SILERO_TTS_URL is explicitly set to empty string"
                );
                return;
            }
        }

        tracing::debug!(url = %config.base_url, "Registering Silero TTS provider");
        let sandbox_scope = self.session.sandbox_scope().clone();

        let provider = if let Some(tx) = progress_tx {
            crate::agent::providers::silero_tts::SileroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
                .with_progress_tx(tx.clone())
        } else {
            crate::agent::providers::silero_tts::SileroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
        };

        let base_url = provider.base_url().to_string();
        registry.register(Box::new(provider));
        tracing::debug!(url = %base_url, "Silero TTS provider registered");
    }
}
