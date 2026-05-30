use super::AgentExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{SandboxRuntime, TodoList};
#[cfg(test)]
use crate::agent::tool_runtime::v1_tool_runtime_enabled_for_model;
#[cfg(feature = "tool-browser-use")]
use crate::agent::tool_runtime::BrowserUseToolModule;
#[cfg(feature = "tool-compression")]
use crate::agent::tool_runtime::CompressionToolModule;
#[cfg(feature = "tool-delegation")]
use crate::agent::tool_runtime::DelegationToolModule;
#[cfg(feature = "tool-duckduckgo")]
use crate::agent::tool_runtime::DuckDuckGoToolModule;
#[cfg(feature = "tool-file-delivery")]
use crate::agent::tool_runtime::FileDeliveryToolModule;
#[cfg(feature = "integration-mcp-jira")]
use crate::agent::tool_runtime::JiraMcpToolModule;
#[cfg(feature = "tool-tts-kokoro")]
use crate::agent::tool_runtime::KokoroTtsToolModule;
#[cfg(feature = "manager-control-plane")]
use crate::agent::tool_runtime::ManagerControlPlaneModuleContext;
#[cfg(feature = "manager-control-plane")]
use crate::agent::tool_runtime::ManagerControlPlaneToolModule;
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
    feature = "manager-control-plane",
    feature = "integration-ssh-mcp",
    feature = "integration-mcp-jira",
    feature = "integration-mcp-mattermost",
    feature = "tool-agents-md",
    feature = "tool-browser-use",
    feature = "tool-compression",
    feature = "tool-delegation",
    feature = "tool-file-delivery",
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video",
    feature = "tool-reminder",
    feature = "tool-duckduckgo",
    feature = "tool-searxng",
    feature = "tool-stack-logs",
    feature = "tool-tavily",
    feature = "tool-todos",
    feature = "tool-tts-kokoro",
    feature = "tool-tts-silero",
    feature = "tool-webfetch-md",
    feature = "tool-wiki-memory",
    feature = "tool-ytdlp",
))]
use crate::agent::tool_runtime::ToolModule;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::tool_runtime::WebFetchMdToolModule;
#[cfg(feature = "tool-wiki-memory")]
use crate::agent::tool_runtime::WikiMemoryToolModule;
#[cfg(feature = "tool-ytdlp")]
use crate::agent::tool_runtime::YtdlpToolModule;
#[cfg(feature = "tool-agents-md")]
use crate::agent::tool_runtime::{AgentsMdModuleContext, AgentsMdToolModule};
#[cfg(feature = "integration-ssh-mcp")]
use crate::agent::tool_runtime::{SshMcpModuleContext, SshMcpToolModule};
use crate::agent::tool_runtime::{
    ToolExecutor, ToolModuleContext, ToolModuleContextParts, ToolRegistry as RuntimeToolRegistry,
};
#[cfg(test)]
use crate::config::ModelInfo;
use crate::sandbox::SandboxScope;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

impl AgentExecutor {
    /// Build the currently exposed tool definitions for this executor state.
    #[must_use]
    pub fn current_tool_definitions(&self) -> Vec<crate::llm::ToolDefinition> {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        self.build_tool_runtime_registry(todos_arc, None).specs()
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
            feature = "manager-control-plane",
            feature = "integration-ssh-mcp",
            feature = "integration-mcp-jira",
            feature = "integration-mcp-mattermost",
            feature = "tool-agents-md",
            feature = "tool-browser-use",
            feature = "tool-compression",
            feature = "tool-delegation",
            feature = "tool-file-delivery",
            feature = "tool-media-audio",
            feature = "tool-media-image",
            feature = "tool-media-video",
            feature = "tool-reminder",
            feature = "tool-duckduckgo",
            feature = "tool-searxng",
            feature = "tool-stack-logs",
            feature = "tool-tavily",
            feature = "tool-todos",
            feature = "tool-tts-kokoro",
            feature = "tool-tts-silero",
            feature = "tool-webfetch-md",
            feature = "tool-wiki-memory",
            feature = "tool-ytdlp"
        )))]
        let _ = (registry, ctx);

        #[cfg(feature = "tool-agents-md")]
        self.register_tool_runtime_module(registry, &AgentsMdToolModule, ctx);
        #[cfg(feature = "integration-mcp-jira")]
        self.register_tool_runtime_module(registry, &JiraMcpToolModule, ctx);
        #[cfg(feature = "manager-control-plane")]
        self.register_tool_runtime_module(registry, &ManagerControlPlaneToolModule, ctx);
        #[cfg(feature = "integration-mcp-mattermost")]
        self.register_tool_runtime_module(registry, &MattermostMcpToolModule, ctx);
        #[cfg(feature = "tool-browser-use")]
        self.register_tool_runtime_module(registry, &BrowserUseToolModule, ctx);
        #[cfg(feature = "tool-compression")]
        self.register_tool_runtime_module(registry, &CompressionToolModule, ctx);
        #[cfg(feature = "tool-delegation")]
        self.register_tool_runtime_module(registry, &DelegationToolModule, ctx);
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
        #[cfg(feature = "tool-duckduckgo")]
        self.register_tool_runtime_module(registry, &DuckDuckGoToolModule, ctx);
        #[cfg(feature = "tool-searxng")]
        self.register_tool_runtime_module(registry, &SearxngToolModule, ctx);
        #[cfg(feature = "integration-ssh-mcp")]
        self.register_tool_runtime_module(registry, &SshMcpToolModule, ctx);
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
        #[cfg(feature = "tool-wiki-memory")]
        self.register_tool_runtime_module(registry, &WikiMemoryToolModule, ctx);
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
        feature = "manager-control-plane",
        feature = "integration-ssh-mcp",
        feature = "integration-mcp-jira",
        feature = "integration-mcp-mattermost",
        feature = "tool-agents-md",
        feature = "tool-browser-use",
        feature = "tool-compression",
        feature = "tool-delegation",
        feature = "tool-file-delivery",
        feature = "tool-media-audio",
        feature = "tool-media-image",
        feature = "tool-media-video",
        feature = "tool-reminder",
        feature = "tool-duckduckgo",
        feature = "tool-stack-logs",
        feature = "tool-tavily",
        feature = "tool-todos",
        feature = "tool-tts-kokoro",
        feature = "tool-tts-silero",
        feature = "tool-webfetch-md",
        feature = "tool-wiki-memory",
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

    #[cfg_attr(
        not(any(
            feature = "tool-sandbox-exec",
            feature = "tool-sandbox-fileops",
            feature = "tool-sandbox-recreate",
            feature = "manager-control-plane",
            feature = "integration-ssh-mcp",
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
            feature = "tool-duckduckgo",
            feature = "tool-searxng",
            feature = "tool-stack-logs",
            feature = "tool-tavily",
            feature = "tool-todos",
            feature = "tool-tts-kokoro",
            feature = "tool-tts-silero",
            feature = "tool-webfetch-md",
            feature = "tool-wiki-memory",
            feature = "tool-ytdlp"
        )),
        allow(dead_code)
    )]
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

    #[must_use]
    #[cfg(test)]
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
            sandbox_runtime: self.build_sandbox_runtime(sandbox_scope, progress_tx),
            llm_client: self.runner.llm_client(),
            settings: Arc::clone(&self.settings),
            browser_use_profile_scope: self.browser_use_profile_scope(),
            browser_use_semaphore: None,
            #[cfg(feature = "tool-agents-md")]
            agents_md_context: self.agents_md.as_ref().map(|context| {
                AgentsMdModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_id.clone(),
                )
            }),
            #[cfg(feature = "manager-control-plane")]
            manager_control_plane_context: self.manager_control_plane.as_ref().map(|context| {
                ManagerControlPlaneModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_lifecycle.clone(),
                )
            }),
            #[cfg(feature = "integration-ssh-mcp")]
            ssh_mcp_context: self.topic_infra.as_ref().map(|context| {
                SshMcpModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_id.clone(),
                    context.config.clone(),
                    context.approvals.clone(),
                )
            }),
            #[cfg(feature = "tool-reminder")]
            reminder_context: self.reminder_context.clone(),
            #[cfg(feature = "tool-wiki-memory")]
            wiki_memory_store: self.wiki_memory_store.clone(),
            #[cfg(feature = "tool-wiki-memory")]
            memory_scope: self.session.memory_scope().clone(),
            progress_tx: progress_tx.cloned(),
        })
    }

    fn build_sandbox_runtime(
        &self,
        sandbox_scope: SandboxScope,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Arc<SandboxRuntime> {
        let runtime = if let Some(tx) = progress_tx {
            SandboxRuntime::new(sandbox_scope).with_progress_tx(tx.clone())
        } else {
            SandboxRuntime::new(sandbox_scope)
        };
        Arc::new(runtime)
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
}
