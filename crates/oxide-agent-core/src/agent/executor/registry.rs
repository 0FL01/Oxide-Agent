use super::AgentExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::{
    AgentsMdProvider, CompressionProvider, DelegationProvider, FileHosterProvider,
    ManagerControlPlaneProvider, ReminderProvider, SandboxProvider, TodoList, TodosProvider,
    WikiMemoryProvider, YtdlpProvider,
};
use crate::agent::registry::ToolRegistry;
use crate::agent::tool_runtime::{
    v1_tool_runtime_enabled_for_model, OutputNormalizer, ToolExecutor, ToolInvocation, ToolName,
    ToolOutput, ToolRegistry as RuntimeToolRegistry, ToolRuntimeConfig, ToolRuntimeError,
    ToolRuntimeModuleContext,
};
use crate::config::ModelInfo;
use crate::llm::ToolDefinition;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};
use tracing::warn;

#[cfg(feature = "tool-browser-use")]
use crate::agent::providers::BrowserUseProvider;
#[cfg(feature = "tool-tts-kokoro")]
use crate::agent::providers::KokoroTtsProvider;
#[cfg(any(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
use crate::agent::providers::MediaFileProvider;
#[cfg(feature = "tool-searxng")]
use crate::agent::providers::SearxngProvider;
#[cfg(feature = "integration-ssh-mcp")]
use crate::agent::providers::SshMcpProvider;
#[cfg(feature = "tool-stack-logs")]
use crate::agent::providers::StackLogsProvider;
#[cfg(feature = "tool-tavily")]
use crate::agent::providers::TavilyProvider;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::providers::WebFetchMdProvider;
#[cfg(feature = "tool-todos")]
use crate::agent::tool_runtime::TodosToolRuntimeModule;
#[cfg(feature = "tool-todos")]
use crate::agent::tool_runtime::ToolRuntimeModule;

impl AgentExecutor {
    /// Build the currently exposed tool definitions for this executor state.
    #[must_use]
    pub fn current_tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        let model_routes = self.settings.get_configured_agent_model_routes();
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_agent_model());
        if Self::v1_tool_runtime_enabled_for_model(&model) {
            let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
            return self.build_tool_runtime_registry(todos_arc, None).specs();
        }

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
        self.register_wiki_memory_provider(&mut registry);

        // Feature-gated MCP, search, and browser automation providers
        self.register_mcp_providers(&mut registry);
        self.register_search_providers(&mut registry);
        self.register_browser_providers(&mut registry);

        // Optional TTS providers.
        self.register_kokoro_tts_provider(&mut registry, progress_tx);
        self.register_silero_tts_provider(&mut registry, progress_tx);

        registry
    }

    #[must_use]
    pub(super) fn build_tool_runtime_registry(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> RuntimeToolRegistry {
        let mut registry = RuntimeToolRegistry::new();

        let module_ctx =
            ToolRuntimeModuleContext::new(Arc::clone(&todos_arc), progress_tx.cloned());
        self.register_tool_runtime_modules(&mut registry, &module_ctx);

        let sandbox_scope = self.session.sandbox_scope().clone();
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope)
        };
        self.register_tool_runtime_executors(
            &mut registry,
            Arc::new(sandbox_provider).tool_runtime_executors(),
        );

        self.register_topic_runtime_providers(&mut registry, progress_tx);
        self.register_wiki_memory_runtime_provider(&mut registry, progress_tx);

        #[cfg(feature = "integration-ssh-mcp")]
        if let Some(topic_infra) = &self.topic_infra {
            let ssh_provider = Arc::new(SshMcpProvider::new(
                Arc::clone(&topic_infra.storage),
                topic_infra.user_id,
                topic_infra.topic_id.clone(),
                topic_infra.config.clone(),
                topic_infra.approvals.clone(),
            ));
            self.register_tool_runtime_executors(
                &mut registry,
                ssh_provider.tool_runtime_executors(),
            );
        }

        registry
    }

    fn register_tool_runtime_modules(
        &self,
        registry: &mut RuntimeToolRegistry,
        ctx: &ToolRuntimeModuleContext,
    ) {
        #[cfg(not(feature = "tool-todos"))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-todos")]
        self.register_tool_runtime_module(registry, &TodosToolRuntimeModule, ctx);
    }

    #[cfg(feature = "tool-todos")]
    fn register_tool_runtime_module<M>(
        &self,
        registry: &mut RuntimeToolRegistry,
        module: &M,
        ctx: &ToolRuntimeModuleContext,
    ) where
        M: ToolRuntimeModule,
    {
        let module_id = module.module_id();
        tracing::debug!(%module_id, "Registering typed tool runtime module");
        self.register_tool_runtime_executors(registry, module.tool_runtime_executors(ctx));
    }

    fn register_tool_runtime_executors(
        &self,
        registry: &mut RuntimeToolRegistry,
        executors: Vec<Arc<dyn ToolExecutor>>,
    ) {
        for executor in executors {
            let tool_name = executor.name();
            if !self
                .execution_profile
                .tool_policy()
                .allows(tool_name.as_str())
            {
                continue;
            }
            if let Err(error) = registry.register(executor) {
                warn!(
                    tool_name = %tool_name,
                    error = %error,
                    "Skipping duplicate typed tool runtime executor"
                );
            }
        }
    }

    fn register_tool_runtime_provider(
        &self,
        registry: &mut RuntimeToolRegistry,
        provider: Arc<dyn ToolProvider>,
        progress_tx: Option<&Sender<AgentEvent>>,
    ) {
        self.register_tool_runtime_executors(
            registry,
            ProviderRuntimeExecutor::from_provider(provider, progress_tx.cloned()),
        );
    }

    fn register_topic_runtime_providers(
        &self,
        registry: &mut RuntimeToolRegistry,
        progress_tx: Option<&Sender<AgentEvent>>,
    ) {
        if let Some(agents_md) = &self.agents_md {
            self.register_tool_runtime_provider(
                registry,
                Arc::new(AgentsMdProvider::new(
                    Arc::clone(&agents_md.storage),
                    agents_md.user_id,
                    agents_md.topic_id.clone(),
                )),
                progress_tx,
            );
        }

        if let Some(manager_provider) = self.manager_control_plane_provider() {
            self.register_tool_runtime_provider(registry, Arc::new(manager_provider), progress_tx);
        }

        if let Some(reminder_context) = &self.reminder_context {
            self.register_tool_runtime_provider(
                registry,
                Arc::new(ReminderProvider::new(reminder_context.clone())),
                progress_tx,
            );
        }
    }

    fn register_wiki_memory_runtime_provider(
        &self,
        registry: &mut RuntimeToolRegistry,
        progress_tx: Option<&Sender<AgentEvent>>,
    ) {
        let Some(store) = self.wiki_memory_store.clone() else {
            return;
        };
        let scope = self.session.memory_scope();
        self.register_tool_runtime_provider(
            registry,
            Arc::new(WikiMemoryProvider::new(
                store,
                scope.user_id,
                scope.context_key.clone(),
            )),
            progress_tx,
        );
    }

    #[must_use]
    pub(super) fn v1_tool_runtime_enabled_for_model(model: &ModelInfo) -> bool {
        v1_tool_runtime_enabled_for_model(model)
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
        #[cfg(feature = "tool-stack-logs")]
        registry.register(Box::new(StackLogsProvider::new()));
        registry.register(Box::new(FileHosterProvider::new(sandbox_scope.clone())));
        #[cfg(any(
            feature = "tool-media-audio",
            feature = "tool-media-image",
            feature = "tool-media-video"
        ))]
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

    #[cfg(feature = "tool-browser-use")]
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

    #[cfg(not(feature = "tool-browser-use"))]
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

        if let Some(manager_provider) = self.manager_control_plane_provider() {
            registry.register(Box::new(manager_provider));
        }

        #[cfg(feature = "integration-ssh-mcp")]
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

    fn manager_control_plane_provider(&self) -> Option<ManagerControlPlaneProvider> {
        let control_plane = self.manager_control_plane.as_ref()?;
        let mut manager_provider = ManagerControlPlaneProvider::new(
            Arc::clone(&control_plane.storage),
            control_plane.user_id,
        );
        if let Some(topic_lifecycle) = &control_plane.topic_lifecycle {
            manager_provider = manager_provider.with_topic_lifecycle(Arc::clone(topic_lifecycle));
        }
        Some(manager_provider)
    }

    fn register_wiki_memory_provider(&self, registry: &mut ToolRegistry) {
        let Some(store) = self.wiki_memory_store.clone() else {
            return;
        };
        let scope = self.session.memory_scope();
        registry.register(Box::new(WikiMemoryProvider::new(
            store,
            scope.user_id,
            scope.context_key.clone(),
        )));
    }

    #[cfg(feature = "integration-mcp-jira")]
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

    #[cfg(feature = "integration-mcp-mattermost")]
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
        #[cfg(feature = "integration-mcp-jira")]
        Self::register_jira_mcp_provider(_registry);

        #[cfg(feature = "integration-mcp-mattermost")]
        Self::register_mattermost_mcp_provider(_registry);
    }

    fn register_search_providers(&self, registry: &mut ToolRegistry) {
        #[cfg(not(any(
            feature = "tool-tavily",
            feature = "tool-searxng",
            feature = "tool-webfetch-md"
        )))]
        let _ = registry;

        #[cfg(feature = "tool-tavily")]
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
        #[cfg(not(feature = "tool-tavily"))]
        if crate::config::is_tavily_enabled() {
            tracing::warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(feature = "tool-searxng")]
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
        #[cfg(not(feature = "tool-searxng"))]
        if crate::config::is_searxng_enabled() {
            tracing::warn!("SearXNG enabled but feature not compiled in");
        }

        #[cfg(feature = "tool-webfetch-md")]
        registry.register(Box::new(WebFetchMdProvider::new()));
    }

    fn register_browser_providers(&self, _registry: &mut ToolRegistry) {
        // NOTE: Browser Use is disabled until a quality vision-capable agent model
        // is available at a reasonable price-per-token. To re-enable, set
        // `BROWSER_USE_URL` (and optionally `BROWSER_USE_MODEL_ID` /
        // `BROWSER_USE_MODEL_PROVIDER`). See `docs/browser-use.md`.
        #[cfg(feature = "tool-browser-use")]
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
        #[cfg(not(feature = "tool-browser-use"))]
        if crate::config::is_browser_use_enabled() {
            tracing::warn!("Browser Use enabled but feature not compiled in");
        }
    }

    #[cfg(feature = "tool-tts-kokoro")]
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

    #[cfg(not(feature = "tool-tts-kokoro"))]
    fn register_kokoro_tts_provider(
        &self,
        _registry: &mut ToolRegistry,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
    }

    #[cfg(feature = "tool-tts-silero")]
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

    #[cfg(not(feature = "tool-tts-silero"))]
    fn register_silero_tts_provider(
        &self,
        _registry: &mut ToolRegistry,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
    }
}

struct ProviderRuntimeExecutor {
    provider: Arc<dyn ToolProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<Sender<AgentEvent>>,
    execution_lock: Arc<Mutex<()>>,
}

impl ProviderRuntimeExecutor {
    fn from_provider(
        provider: Arc<dyn ToolProvider>,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        let execution_lock = Arc::new(Mutex::new(()));
        provider
            .tools()
            .into_iter()
            .map(|spec| {
                Arc::new(Self {
                    provider: Arc::clone(&provider),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    progress_tx: progress_tx.clone(),
                    execution_lock: Arc::clone(&execution_lock),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }
}

#[async_trait]
impl ToolExecutor for ProviderRuntimeExecutor {
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
        let _guard = self.execution_lock.lock().await;
        let output = self
            .provider
            .execute(
                self.name.as_str(),
                &invocation.raw_arguments,
                self.progress_tx.as_ref(),
                Some(&invocation.cancellation_token),
            )
            .await
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))?;
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        Ok(normalizer.success(&invocation, &output, ""))
    }
}
