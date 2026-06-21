use super::AgentExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{SandboxRuntime, TodoList};
use crate::agent::tool_runtime::AgentsMdModuleContext;
#[cfg(oxide_module_tool_agents_md)]
use crate::agent::tool_runtime::AgentsMdToolModule;
#[cfg(oxide_module_tool_brave_search)]
use crate::agent::tool_runtime::BraveSearchToolModule;
use crate::agent::tool_runtime::BrowserLiveModuleContext;
#[cfg(oxide_module_tool_browser_live)]
use crate::agent::tool_runtime::BrowserLiveToolModule;
#[cfg(oxide_module_tool_compression)]
use crate::agent::tool_runtime::CompressionToolModule;
#[cfg(oxide_module_tool_crw)]
use crate::agent::tool_runtime::CrwSearchToolModule;
#[cfg(oxide_module_tool_delegation)]
use crate::agent::tool_runtime::DelegationToolModule;
#[cfg(oxide_module_tool_file_delivery)]
use crate::agent::tool_runtime::FileDeliveryToolModule;
#[cfg(oxide_module_integration_mcp_jira)]
use crate::agent::tool_runtime::JiraMcpToolModule;
#[cfg(oxide_module_tool_tts_kokoro)]
use crate::agent::tool_runtime::KokoroTtsToolModule;
use crate::agent::tool_runtime::ManagerControlPlaneModuleContext;
#[cfg(oxide_module_manager_control_plane)]
use crate::agent::tool_runtime::ManagerControlPlaneToolModule;
#[cfg(oxide_module_integration_mcp_mattermost)]
use crate::agent::tool_runtime::MattermostMcpToolModule;
#[cfg(oxide_module_tool_media_audio)]
use crate::agent::tool_runtime::MediaAudioToolModule;
#[cfg(oxide_module_tool_media_image)]
use crate::agent::tool_runtime::MediaImageToolModule;
#[cfg(oxide_module_tool_media_video)]
use crate::agent::tool_runtime::MediaVideoToolModule;
#[cfg(oxide_module_tool_reminder)]
use crate::agent::tool_runtime::ReminderToolModule;
#[cfg(oxide_module_tool_sandbox_exec)]
use crate::agent::tool_runtime::SandboxExecToolModule;
#[cfg(oxide_module_tool_sandbox_fileops)]
use crate::agent::tool_runtime::SandboxFileOpsToolModule;
#[cfg(oxide_module_tool_sandbox_recreate)]
use crate::agent::tool_runtime::SandboxRecreateToolModule;
#[cfg(oxide_module_tool_tts_silero)]
use crate::agent::tool_runtime::SileroTtsToolModule;
use crate::agent::tool_runtime::SshMcpModuleContext;
#[cfg(oxide_module_integration_ssh_mcp)]
use crate::agent::tool_runtime::SshMcpToolModule;
#[cfg(oxide_module_tool_stack_logs)]
use crate::agent::tool_runtime::StackLogsToolModule;
#[cfg(oxide_module_tool_tavily)]
use crate::agent::tool_runtime::TavilyToolModule;
#[cfg(oxide_module_tool_todos)]
use crate::agent::tool_runtime::TodosToolModule;
#[cfg(any(
    oxide_module_tool_sandbox_exec,
    oxide_module_tool_sandbox_fileops,
    oxide_module_tool_sandbox_recreate,
    oxide_module_manager_control_plane,
    oxide_module_integration_ssh_mcp,
    oxide_module_integration_mcp_jira,
    oxide_module_integration_mcp_mattermost,
    oxide_module_tool_agents_md,
    oxide_module_tool_compression,
    oxide_module_tool_delegation,
    oxide_module_tool_file_delivery,
    oxide_module_tool_media_audio,
    oxide_module_tool_media_image,
    oxide_module_tool_media_video,
    oxide_module_tool_reminder,
    oxide_module_tool_brave_search,
    oxide_module_tool_browser_live,
    oxide_module_tool_crw,
    oxide_module_tool_stack_logs,
    oxide_module_tool_tavily,
    oxide_module_tool_todos,
    oxide_module_tool_tts_kokoro,
    oxide_module_tool_tts_silero,
    oxide_module_tool_webfetch_md,
    oxide_module_tool_wiki_memory,
    oxide_module_tool_ytdlp,
))]
use crate::agent::tool_runtime::ToolModule;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::tool_runtime::WebCrawlerToolModule;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::tool_runtime::WebFetchMdToolModule;
#[cfg(oxide_module_tool_wiki_memory)]
use crate::agent::tool_runtime::WikiMemoryToolModule;
#[cfg(oxide_module_tool_ytdlp)]
use crate::agent::tool_runtime::YtdlpToolModule;
#[cfg(test)]
use crate::agent::tool_runtime::v1_tool_runtime_enabled_for_model;
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
            oxide_module_tool_sandbox_exec,
            oxide_module_tool_sandbox_fileops,
            oxide_module_tool_sandbox_recreate,
            oxide_module_manager_control_plane,
            oxide_module_integration_ssh_mcp,
            oxide_module_integration_mcp_jira,
            oxide_module_integration_mcp_mattermost,
            oxide_module_tool_agents_md,
            oxide_module_tool_compression,
            oxide_module_tool_delegation,
            oxide_module_tool_file_delivery,
            oxide_module_tool_media_audio,
            oxide_module_tool_media_image,
            oxide_module_tool_media_video,
            oxide_module_tool_reminder,
            oxide_module_tool_brave_search,
            oxide_module_tool_browser_live,
            oxide_module_tool_crw,
            oxide_module_tool_stack_logs,
            oxide_module_tool_tavily,
            oxide_module_tool_todos,
            oxide_module_tool_tts_kokoro,
            oxide_module_tool_tts_silero,
            oxide_module_tool_webfetch_md,
            oxide_module_tool_wiki_memory,
            oxide_module_tool_ytdlp
        )))]
        let _ = (registry, ctx);

        #[cfg(oxide_module_tool_agents_md)]
        self.register_tool_runtime_module(registry, &AgentsMdToolModule, ctx);
        #[cfg(oxide_module_integration_mcp_jira)]
        self.register_tool_runtime_module(registry, &JiraMcpToolModule, ctx);
        #[cfg(oxide_module_manager_control_plane)]
        self.register_tool_runtime_module(registry, &ManagerControlPlaneToolModule, ctx);
        #[cfg(oxide_module_integration_mcp_mattermost)]
        self.register_tool_runtime_module(registry, &MattermostMcpToolModule, ctx);
        #[cfg(oxide_module_tool_compression)]
        self.register_tool_runtime_module(registry, &CompressionToolModule, ctx);
        #[cfg(oxide_module_tool_delegation)]
        self.register_tool_runtime_module(registry, &DelegationToolModule, ctx);
        #[cfg(oxide_module_tool_file_delivery)]
        self.register_tool_runtime_module(registry, &FileDeliveryToolModule, ctx);
        #[cfg(oxide_module_tool_media_audio)]
        self.register_tool_runtime_module(registry, &MediaAudioToolModule, ctx);
        #[cfg(oxide_module_tool_media_image)]
        self.register_tool_runtime_module(registry, &MediaImageToolModule, ctx);
        #[cfg(oxide_module_tool_media_video)]
        self.register_tool_runtime_module(registry, &MediaVideoToolModule, ctx);
        #[cfg(oxide_module_tool_reminder)]
        self.register_tool_runtime_module(registry, &ReminderToolModule, ctx);
        #[cfg(oxide_module_tool_brave_search)]
        self.register_tool_runtime_module(registry, &BraveSearchToolModule, ctx);
        #[cfg(oxide_module_tool_browser_live)]
        self.register_tool_runtime_module(registry, &BrowserLiveToolModule, ctx);
        #[cfg(oxide_module_tool_crw)]
        self.register_tool_runtime_module(registry, &CrwSearchToolModule, ctx);
        #[cfg(oxide_module_integration_ssh_mcp)]
        self.register_tool_runtime_module(registry, &SshMcpToolModule, ctx);
        #[cfg(oxide_module_tool_stack_logs)]
        self.register_tool_runtime_module(registry, &StackLogsToolModule, ctx);
        #[cfg(oxide_module_tool_tavily)]
        self.register_tool_runtime_module(registry, &TavilyToolModule, ctx);
        #[cfg(oxide_module_tool_todos)]
        self.register_tool_runtime_module(registry, &TodosToolModule, ctx);
        #[cfg(oxide_module_tool_tts_kokoro)]
        self.register_tool_runtime_module(registry, &KokoroTtsToolModule, ctx);
        #[cfg(oxide_module_tool_tts_silero)]
        self.register_tool_runtime_module(registry, &SileroTtsToolModule, ctx);
        #[cfg(oxide_module_tool_webfetch_md)]
        self.register_tool_runtime_module(registry, &WebCrawlerToolModule, ctx);
        #[cfg(oxide_module_tool_webfetch_md)]
        self.register_tool_runtime_module(registry, &WebFetchMdToolModule, ctx);
        #[cfg(oxide_module_tool_wiki_memory)]
        self.register_tool_runtime_module(registry, &WikiMemoryToolModule, ctx);
        #[cfg(oxide_module_tool_ytdlp)]
        self.register_tool_runtime_module(registry, &YtdlpToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_exec)]
        self.register_tool_runtime_module(registry, &SandboxExecToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_fileops)]
        self.register_tool_runtime_module(registry, &SandboxFileOpsToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_recreate)]
        self.register_tool_runtime_module(registry, &SandboxRecreateToolModule, ctx);
    }

    #[cfg(any(
        oxide_module_tool_sandbox_exec,
        oxide_module_tool_sandbox_fileops,
        oxide_module_tool_sandbox_recreate,
        oxide_module_manager_control_plane,
        oxide_module_integration_ssh_mcp,
        oxide_module_integration_mcp_jira,
        oxide_module_integration_mcp_mattermost,
        oxide_module_tool_agents_md,
        oxide_module_tool_compression,
        oxide_module_tool_delegation,
        oxide_module_tool_file_delivery,
        oxide_module_tool_media_audio,
        oxide_module_tool_media_image,
        oxide_module_tool_media_video,
        oxide_module_tool_reminder,
        oxide_module_tool_brave_search,
        oxide_module_tool_browser_live,
        oxide_module_tool_crw,
        oxide_module_tool_stack_logs,
        oxide_module_tool_tavily,
        oxide_module_tool_todos,
        oxide_module_tool_tts_kokoro,
        oxide_module_tool_tts_silero,
        oxide_module_tool_webfetch_md,
        oxide_module_tool_wiki_memory,
        oxide_module_tool_ytdlp
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
            oxide_module_tool_sandbox_exec,
            oxide_module_tool_sandbox_fileops,
            oxide_module_tool_sandbox_recreate,
            oxide_module_manager_control_plane,
            oxide_module_integration_ssh_mcp,
            oxide_module_integration_mcp_jira,
            oxide_module_integration_mcp_mattermost,
            oxide_module_tool_agents_md,
            oxide_module_tool_compression,
            oxide_module_tool_file_delivery,
            oxide_module_tool_media_audio,
            oxide_module_tool_media_image,
            oxide_module_tool_media_video,
            oxide_module_tool_reminder,
            oxide_module_tool_brave_search,
            oxide_module_tool_browser_live,
            oxide_module_tool_crw,
            oxide_module_tool_stack_logs,
            oxide_module_tool_tavily,
            oxide_module_tool_todos,
            oxide_module_tool_tts_kokoro,
            oxide_module_tool_tts_silero,
            oxide_module_tool_webfetch_md,
            oxide_module_tool_wiki_memory,
            oxide_module_tool_ytdlp
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
            agents_md_context: self.agents_md.as_ref().map(|context| {
                AgentsMdModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_id.clone(),
                )
            }),
            manager_control_plane_context: self.manager_control_plane.as_ref().map(|context| {
                ManagerControlPlaneModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_lifecycle.clone(),
                )
            }),
            ssh_mcp_context: self.topic_infra.as_ref().map(|context| {
                SshMcpModuleContext::new(
                    Arc::clone(&context.storage),
                    context.user_id,
                    context.topic_id.clone(),
                    context.config.clone(),
                )
            }),
            browser_live_context: self.storage.as_ref().map(|storage| {
                let scope = self.session.memory_scope();
                BrowserLiveModuleContext::new(
                    Arc::clone(storage),
                    scope.user_id,
                    scope.context_key.clone(),
                )
            }),
            reminder_context: self.reminder_context.clone(),
            wiki_memory_store: self.wiki_memory_store.clone(),
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
}
