use super::AgentExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
#[cfg(feature = "tool-agents-md")]
use crate::agent::providers::AgentsMdProvider;
#[cfg(feature = "tool-reminder")]
use crate::agent::providers::ReminderProvider;
#[cfg(feature = "integration-ssh-mcp")]
use crate::agent::providers::SshMcpProvider;
use crate::agent::providers::{
    DelegationProvider, ManagerControlPlaneProvider, SandboxProvider, TodoList, WikiMemoryProvider,
};
use crate::agent::registry::ToolRegistry;
#[cfg(feature = "tool-browser-use")]
use crate::agent::tool_runtime::BrowserUseToolModule;
#[cfg(feature = "tool-compression")]
use crate::agent::tool_runtime::CompressionToolModule;
#[cfg(feature = "tool-file-delivery")]
use crate::agent::tool_runtime::FileDeliveryToolModule;
#[cfg(feature = "integration-mcp-jira")]
use crate::agent::tool_runtime::JiraMcpToolModule;
#[cfg(feature = "tool-tts-kokoro")]
use crate::agent::tool_runtime::KokoroTtsToolModule;
#[cfg(feature = "integration-mcp-mattermost")]
use crate::agent::tool_runtime::MattermostMcpToolModule;
#[cfg(feature = "tool-media-audio")]
use crate::agent::tool_runtime::MediaAudioToolModule;
#[cfg(feature = "tool-media-image")]
use crate::agent::tool_runtime::MediaImageToolModule;
#[cfg(feature = "tool-media-video")]
use crate::agent::tool_runtime::MediaVideoToolModule;
#[cfg(feature = "tool-reminder")]
use crate::agent::tool_runtime::ReminderToolModule;
#[cfg(feature = "tool-sandbox-exec")]
use crate::agent::tool_runtime::SandboxExecToolModule;
#[cfg(feature = "tool-sandbox-fileops")]
use crate::agent::tool_runtime::SandboxFileOpsToolModule;
#[cfg(feature = "tool-sandbox-recreate")]
use crate::agent::tool_runtime::SandboxRecreateToolModule;
#[cfg(feature = "tool-searxng")]
use crate::agent::tool_runtime::SearxngToolModule;
#[cfg(feature = "tool-tts-silero")]
use crate::agent::tool_runtime::SileroTtsToolModule;
#[cfg(feature = "tool-stack-logs")]
use crate::agent::tool_runtime::StackLogsToolModule;
#[cfg(feature = "tool-tavily")]
use crate::agent::tool_runtime::TavilyToolModule;
#[cfg(feature = "tool-todos")]
use crate::agent::tool_runtime::TodosToolModule;
#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate",
    feature = "integration-mcp-jira",
    feature = "integration-mcp-mattermost",
    feature = "tool-agents-md",
    feature = "tool-browser-use",
    feature = "tool-compression",
    feature = "tool-file-delivery",
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video",
    feature = "tool-reminder",
    feature = "tool-searxng",
    feature = "tool-stack-logs",
    feature = "tool-tavily",
    feature = "tool-todos",
    feature = "tool-tts-kokoro",
    feature = "tool-tts-silero",
    feature = "tool-webfetch-md",
    feature = "tool-ytdlp",
))]
use crate::agent::tool_runtime::ToolModule;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::tool_runtime::WebFetchMdToolModule;
#[cfg(feature = "tool-ytdlp")]
use crate::agent::tool_runtime::YtdlpToolModule;
use crate::agent::tool_runtime::{
    v1_tool_runtime_enabled_for_model, OutputNormalizer, ToolExecutor, ToolInvocation,
    ToolModuleContext, ToolModuleContextParts, ToolName, ToolOutput,
    ToolRegistry as RuntimeToolRegistry, ToolRuntimeConfig, ToolRuntimeError,
};
#[cfg(feature = "tool-agents-md")]
use crate::agent::tool_runtime::{AgentsMdModuleContext, AgentsMdToolModule};
use crate::config::ModelInfo;
use crate::llm::ToolDefinition;
use crate::sandbox::SandboxScope;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};
use tracing::warn;

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
        let module_ctx = self.build_tool_module_context(todos_arc, progress_tx);

        // Core providers: module-owned tools, media file analysis, and delegation
        self.register_core_providers(&mut registry, &module_ctx);

        // Topic-scoped providers: agents_md, manager, ssh, reminders
        self.register_topic_providers(&mut registry, &module_ctx);
        self.register_wiki_memory_provider(&mut registry);

        // Feature-gated MCP, search, and browser automation providers
        self.register_mcp_providers(&mut registry, &module_ctx);
        self.register_search_providers(&mut registry, &module_ctx);
        self.register_browser_providers(&mut registry, &module_ctx);

        // Optional TTS providers.
        self.register_tts_providers(&mut registry, &module_ctx);

        registry
    }

    #[must_use]
    pub(super) fn build_tool_runtime_registry(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> RuntimeToolRegistry {
        let mut registry = RuntimeToolRegistry::new();

        let module_ctx = self.build_tool_module_context(Arc::clone(&todos_arc), progress_tx);
        self.register_tool_runtime_modules(&mut registry, &module_ctx);

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
        ctx: &ToolModuleContext,
    ) {
        #[cfg(not(any(
            feature = "tool-sandbox-exec",
            feature = "tool-sandbox-fileops",
            feature = "tool-sandbox-recreate",
            feature = "integration-mcp-jira",
            feature = "integration-mcp-mattermost",
            feature = "tool-agents-md",
            feature = "tool-browser-use",
            feature = "tool-compression",
            feature = "tool-file-delivery",
            feature = "tool-media-audio",
            feature = "tool-media-image",
            feature = "tool-media-video",
            feature = "tool-reminder",
            feature = "tool-searxng",
            feature = "tool-stack-logs",
            feature = "tool-tavily",
            feature = "tool-todos",
            feature = "tool-tts-kokoro",
            feature = "tool-tts-silero",
            feature = "tool-webfetch-md",
            feature = "tool-ytdlp"
        )))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-agents-md")]
        self.register_tool_runtime_module(registry, &AgentsMdToolModule, ctx);
        #[cfg(feature = "integration-mcp-jira")]
        self.register_tool_runtime_module(registry, &JiraMcpToolModule, ctx);
        #[cfg(feature = "integration-mcp-mattermost")]
        self.register_tool_runtime_module(registry, &MattermostMcpToolModule, ctx);
        #[cfg(feature = "tool-browser-use")]
        self.register_tool_runtime_module(registry, &BrowserUseToolModule, ctx);
        #[cfg(feature = "tool-compression")]
        self.register_tool_runtime_module(registry, &CompressionToolModule, ctx);
        #[cfg(feature = "tool-file-delivery")]
        self.register_tool_runtime_module(registry, &FileDeliveryToolModule, ctx);
        #[cfg(feature = "tool-media-audio")]
        self.register_tool_runtime_module(registry, &MediaAudioToolModule, ctx);
        #[cfg(feature = "tool-media-image")]
        self.register_tool_runtime_module(registry, &MediaImageToolModule, ctx);
        #[cfg(feature = "tool-media-video")]
        self.register_tool_runtime_module(registry, &MediaVideoToolModule, ctx);
        #[cfg(feature = "tool-reminder")]
        self.register_tool_runtime_module(registry, &ReminderToolModule, ctx);
        #[cfg(feature = "tool-searxng")]
        self.register_tool_runtime_module(registry, &SearxngToolModule, ctx);
        #[cfg(feature = "tool-stack-logs")]
        self.register_tool_runtime_module(registry, &StackLogsToolModule, ctx);
        #[cfg(feature = "tool-tavily")]
        self.register_tool_runtime_module(registry, &TavilyToolModule, ctx);
        #[cfg(feature = "tool-todos")]
        self.register_tool_runtime_module(registry, &TodosToolModule, ctx);
        #[cfg(feature = "tool-tts-kokoro")]
        self.register_tool_runtime_module(registry, &KokoroTtsToolModule, ctx);
        #[cfg(feature = "tool-tts-silero")]
        self.register_tool_runtime_module(registry, &SileroTtsToolModule, ctx);
        #[cfg(feature = "tool-webfetch-md")]
        self.register_tool_runtime_module(registry, &WebFetchMdToolModule, ctx);
        #[cfg(feature = "tool-ytdlp")]
        self.register_tool_runtime_module(registry, &YtdlpToolModule, ctx);
        #[cfg(feature = "tool-sandbox-exec")]
        self.register_tool_runtime_module(registry, &SandboxExecToolModule, ctx);
        #[cfg(feature = "tool-sandbox-fileops")]
        self.register_tool_runtime_module(registry, &SandboxFileOpsToolModule, ctx);
        #[cfg(feature = "tool-sandbox-recreate")]
        self.register_tool_runtime_module(registry, &SandboxRecreateToolModule, ctx);
    }

    #[cfg(any(
        feature = "tool-sandbox-exec",
        feature = "tool-sandbox-fileops",
        feature = "tool-sandbox-recreate",
        feature = "integration-mcp-jira",
        feature = "integration-mcp-mattermost",
        feature = "tool-agents-md",
        feature = "tool-browser-use",
        feature = "tool-compression",
        feature = "tool-file-delivery",
        feature = "tool-media-audio",
        feature = "tool-media-image",
        feature = "tool-media-video",
        feature = "tool-reminder",
        feature = "tool-searxng",
        feature = "tool-stack-logs",
        feature = "tool-tavily",
        feature = "tool-todos",
        feature = "tool-tts-kokoro",
        feature = "tool-tts-silero",
        feature = "tool-webfetch-md",
        feature = "tool-ytdlp"
    ))]
    fn register_tool_runtime_module<M>(
        &self,
        registry: &mut RuntimeToolRegistry,
        module: &M,
        ctx: &ToolModuleContext,
    ) where
        M: ToolModule,
    {
        let module_id = module.module_id();
        if !self.settings.is_module_enabled(module_id.as_str()) {
            tracing::debug!(%module_id, "Skipping disabled typed tool runtime module");
            return;
        }

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
        #[cfg(feature = "tool-agents-md")]
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

        #[cfg(feature = "tool-reminder")]
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

    fn build_tool_module_context(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolModuleContext {
        let sandbox_scope = self.session.sandbox_scope().clone();
        ToolModuleContext::new(ToolModuleContextParts {
            todos: todos_arc,
            sandbox_scope: sandbox_scope.clone(),
            sandbox_provider: self.build_sandbox_provider(sandbox_scope, progress_tx),
            llm_client: self.runner.llm_client(),
            settings: Arc::clone(&self.settings),
            browser_use_profile_scope: self.browser_use_profile_scope(),
            #[cfg(feature = "tool-agents-md")]
            agents_md_context: self.agents_md.as_ref().map(|context| {
                AgentsMdModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_id.clone(),
                )
            }),
            #[cfg(feature = "tool-reminder")]
            reminder_context: self.reminder_context.clone(),
            progress_tx: progress_tx.cloned(),
        })
    }

    fn build_sandbox_provider(
        &self,
        sandbox_scope: SandboxScope,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Arc<SandboxProvider> {
        let provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope)
        };
        Arc::new(provider)
    }

    fn register_core_providers(&self, registry: &mut ToolRegistry, module_ctx: &ToolModuleContext) {
        let sandbox_scope = module_ctx.sandbox_scope();
        self.register_legacy_tool_modules(registry, module_ctx);

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

    fn register_legacy_tool_modules(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(not(any(
            feature = "tool-sandbox-exec",
            feature = "tool-sandbox-fileops",
            feature = "tool-sandbox-recreate",
            feature = "tool-compression",
            feature = "tool-file-delivery",
            feature = "tool-media-audio",
            feature = "tool-media-image",
            feature = "tool-media-video",
            feature = "tool-stack-logs",
            feature = "tool-todos",
            feature = "tool-ytdlp"
        )))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-compression")]
        self.register_legacy_tool_module(registry, &CompressionToolModule, ctx);
        #[cfg(feature = "tool-file-delivery")]
        self.register_legacy_tool_module(registry, &FileDeliveryToolModule, ctx);
        #[cfg(feature = "tool-stack-logs")]
        self.register_legacy_tool_module(registry, &StackLogsToolModule, ctx);
        #[cfg(feature = "tool-todos")]
        self.register_legacy_tool_module(registry, &TodosToolModule, ctx);
        #[cfg(feature = "tool-ytdlp")]
        self.register_legacy_tool_module(registry, &YtdlpToolModule, ctx);
        #[cfg(feature = "tool-sandbox-exec")]
        self.register_legacy_tool_module(registry, &SandboxExecToolModule, ctx);
        #[cfg(feature = "tool-sandbox-fileops")]
        self.register_legacy_tool_module(registry, &SandboxFileOpsToolModule, ctx);
        #[cfg(feature = "tool-sandbox-recreate")]
        self.register_legacy_tool_module(registry, &SandboxRecreateToolModule, ctx);
        #[cfg(feature = "tool-media-audio")]
        self.register_legacy_tool_module(registry, &MediaAudioToolModule, ctx);
        #[cfg(feature = "tool-media-image")]
        self.register_legacy_tool_module(registry, &MediaImageToolModule, ctx);
        #[cfg(feature = "tool-media-video")]
        self.register_legacy_tool_module(registry, &MediaVideoToolModule, ctx);
    }

    #[cfg(any(
        feature = "tool-sandbox-exec",
        feature = "tool-sandbox-fileops",
        feature = "tool-sandbox-recreate",
        feature = "integration-mcp-jira",
        feature = "integration-mcp-mattermost",
        feature = "tool-agents-md",
        feature = "tool-browser-use",
        feature = "tool-compression",
        feature = "tool-file-delivery",
        feature = "tool-media-audio",
        feature = "tool-media-image",
        feature = "tool-media-video",
        feature = "tool-reminder",
        feature = "tool-searxng",
        feature = "tool-stack-logs",
        feature = "tool-tavily",
        feature = "tool-todos",
        feature = "tool-tts-kokoro",
        feature = "tool-tts-silero",
        feature = "tool-webfetch-md",
        feature = "tool-ytdlp"
    ))]
    fn register_legacy_tool_module<M>(
        &self,
        registry: &mut ToolRegistry,
        module: &M,
        ctx: &ToolModuleContext,
    ) where
        M: ToolModule,
    {
        let module_id = module.module_id();
        if !self.settings.is_module_enabled(module_id.as_str()) {
            tracing::debug!(%module_id, "Skipping disabled legacy tool module");
            return;
        }

        tracing::debug!(%module_id, "Registering legacy tool module");
        if let Some(provider) = module.legacy_provider(ctx) {
            registry.register(provider);
        }
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

    fn register_topic_providers(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(feature = "tool-agents-md")]
        self.register_legacy_tool_module(registry, &AgentsMdToolModule, ctx);

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

        #[cfg(feature = "tool-reminder")]
        self.register_legacy_tool_module(registry, &ReminderToolModule, ctx);

        #[cfg(not(any(feature = "tool-agents-md", feature = "tool-reminder")))]
        let _ = ctx;
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

    fn register_mcp_providers(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(not(any(
            feature = "integration-mcp-jira",
            feature = "integration-mcp-mattermost"
        )))]
        let _ = (registry, ctx);

        #[cfg(feature = "integration-mcp-jira")]
        self.register_legacy_tool_module(registry, &JiraMcpToolModule, ctx);

        #[cfg(feature = "integration-mcp-mattermost")]
        self.register_legacy_tool_module(registry, &MattermostMcpToolModule, ctx);
    }

    fn register_search_providers(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(not(any(
            feature = "tool-tavily",
            feature = "tool-searxng",
            feature = "tool-webfetch-md"
        )))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-tavily")]
        self.register_legacy_tool_module(registry, &TavilyToolModule, ctx);
        #[cfg(not(feature = "tool-tavily"))]
        if crate::config::is_tavily_enabled() {
            tracing::warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(feature = "tool-searxng")]
        self.register_legacy_tool_module(registry, &SearxngToolModule, ctx);
        #[cfg(not(feature = "tool-searxng"))]
        if crate::config::is_searxng_enabled() {
            tracing::warn!("SearXNG enabled but feature not compiled in");
        }

        #[cfg(feature = "tool-webfetch-md")]
        self.register_legacy_tool_module(registry, &WebFetchMdToolModule, ctx);
    }

    fn register_browser_providers(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(feature = "tool-browser-use")]
        self.register_legacy_tool_module(registry, &BrowserUseToolModule, ctx);
        #[cfg(not(feature = "tool-browser-use"))]
        if crate::config::is_browser_use_enabled() {
            tracing::warn!("Browser Use enabled but feature not compiled in");
        }
        #[cfg(not(feature = "tool-browser-use"))]
        let _ = (registry, ctx);
    }

    fn register_tts_providers(&self, registry: &mut ToolRegistry, ctx: &ToolModuleContext) {
        #[cfg(not(any(feature = "tool-tts-kokoro", feature = "tool-tts-silero")))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-tts-kokoro")]
        self.register_legacy_tool_module(registry, &KokoroTtsToolModule, ctx);
        #[cfg(feature = "tool-tts-silero")]
        self.register_legacy_tool_module(registry, &SileroTtsToolModule, ctx);
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
