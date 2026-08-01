use super::AgentExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{SandboxRuntime, TodoList};
use crate::agent::tool_runtime::AgentsMdModuleContext;
#[cfg(oxide_module_tool_agents_md)]
use crate::agent::tool_runtime::AgentsMdToolModule;
#[cfg(oxide_module_tool_audio_stt)]
use crate::agent::tool_runtime::AudioSttToolModule;
use crate::agent::tool_runtime::BrowserLiveModuleContext;
#[cfg(oxide_module_tool_browser_live)]
use crate::agent::tool_runtime::BrowserLiveToolModule;
#[cfg(oxide_module_tool_compression)]
use crate::agent::tool_runtime::CompressionToolModule;
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
#[cfg(oxide_module_tool_reminder)]
use crate::agent::tool_runtime::ReminderToolModule;
#[cfg(oxide_module_tool_retrieve_tools)]
use crate::agent::tool_runtime::RetrieveToolsToolModule;
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
#[cfg(oxide_module_tool_todos)]
use crate::agent::tool_runtime::TodosToolModule;
#[cfg(oxide_module_tool_vision_image)]
use crate::agent::tool_runtime::VisionImageToolModule;
#[cfg(oxide_module_tool_vision_video)]
use crate::agent::tool_runtime::VisionVideoToolModule;
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
    oxide_module_tool_retrieve_tools,
    oxide_module_tool_file_delivery,
    oxide_module_tool_audio_stt,
    oxide_module_tool_vision_image,
    oxide_module_tool_vision_video,
    oxide_module_tool_reminder,
    oxide_module_tool_browser_live,
    oxide_module_tool_stack_logs,
    oxide_module_tool_todos,
    oxide_module_tool_tts_kokoro,
    oxide_module_tool_tts_silero,
    oxide_module_tool_web_search,
    oxide_module_tool_webfetch_md,
    oxide_module_tool_ytdlp,
))]
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::tool_runtime::WebCrawlerToolModule;
#[cfg(oxide_module_tool_webfetch_md)]
use crate::agent::tool_runtime::WebFetchMdToolModule;
#[cfg(oxide_module_tool_web_search)]
use crate::agent::tool_runtime::WebSearchToolModule;
#[cfg(oxide_module_tool_ytdlp)]
use crate::agent::tool_runtime::YtdlpToolModule;
#[cfg(test)]
use crate::agent::tool_runtime::v1_tool_runtime_enabled_for_model;
use crate::agent::tool_runtime::{
    BrowserSessionCleanup, CapabilityGroup, ToolCatalog, ToolCatalogEntry, ToolExecutor,
    ToolModuleContext, ToolModuleContextParts, ToolSurfaceHandle, ToolVisibility,
};
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
    oxide_module_tool_retrieve_tools,
    oxide_module_tool_file_delivery,
    oxide_module_tool_audio_stt,
    oxide_module_tool_vision_image,
    oxide_module_tool_vision_video,
    oxide_module_tool_reminder,
    oxide_module_tool_browser_live,
    oxide_module_tool_stack_logs,
    oxide_module_tool_todos,
    oxide_module_tool_tts_kokoro,
    oxide_module_tool_tts_silero,
    oxide_module_tool_web_search,
    oxide_module_tool_webfetch_md,
    oxide_module_tool_ytdlp
))]
use crate::agent::tool_runtime::{ToolModule, ToolName};
use crate::capabilities::ModuleId;
#[cfg(test)]
use crate::config::ModelInfo;
use crate::sandbox::SandboxScope;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::warn;

/// Output of building the tool runtime catalog, surface, and browser cleanup.
pub(super) struct ToolRuntimeBuild {
    pub browser_cleanup: Option<Arc<dyn BrowserSessionCleanup>>,
    pub surface_handle: Arc<ToolSurfaceHandle>,
    pub catalog: Arc<ToolCatalog>,
}

impl AgentExecutor {
    /// Build the full tool catalog for this executor state.
    ///
    /// Returns every registered tool definition — the complete executable set.
    /// This is the catalog used for admin/UI/snapshots and manual compaction
    /// token estimation. The model-visible surface (a subset) is resolved
    /// per-iteration during a run via `AgentRunnerContext::tools`.
    #[must_use]
    pub fn current_tool_catalog(&self) -> Vec<crate::llm::ToolDefinition> {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        self.build_tool_runtime(todos_arc, None).catalog.specs()
    }

    #[cfg(test)]
    pub(super) fn build_tool_catalog(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Arc<ToolCatalog> {
        self.build_tool_runtime(todos_arc, progress_tx).catalog
    }

    /// Build the executable catalog, lazy surface, and browser cleanup handle.
    #[must_use]
    pub(super) fn build_tool_runtime(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolRuntimeBuild {
        let mut catalog = ToolCatalog::new();

        let module_ctx = self.build_tool_module_context(Arc::clone(&todos_arc), progress_tx);
        let browser_cleanup = self.register_tool_runtime_modules(&mut catalog, &module_ctx);

        let surface_handle = module_ctx.tool_surface_handle();
        ToolRuntimeBuild {
            browser_cleanup,
            surface_handle,
            catalog: Arc::new(catalog),
        }
    }

    fn register_tool_runtime_modules(
        &self,
        catalog: &mut ToolCatalog,
        ctx: &ToolModuleContext,
    ) -> Option<Arc<dyn BrowserSessionCleanup>> {
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
            oxide_module_tool_retrieve_tools,
            oxide_module_tool_file_delivery,
            oxide_module_tool_audio_stt,
            oxide_module_tool_vision_image,
            oxide_module_tool_vision_video,
            oxide_module_tool_reminder,
            oxide_module_tool_browser_live,
            oxide_module_tool_stack_logs,
            oxide_module_tool_todos,
            oxide_module_tool_tts_kokoro,
            oxide_module_tool_tts_silero,
            oxide_module_tool_web_search,
            oxide_module_tool_webfetch_md,
            oxide_module_tool_ytdlp
        )))]
        let _ = (catalog, ctx);

        #[cfg(oxide_module_tool_agents_md)]
        self.register_tool_runtime_module(catalog, &AgentsMdToolModule, ctx);
        #[cfg(oxide_module_integration_mcp_jira)]
        self.register_tool_runtime_module(catalog, &JiraMcpToolModule, ctx);
        #[cfg(oxide_module_manager_control_plane)]
        self.register_tool_runtime_module(catalog, &ManagerControlPlaneToolModule, ctx);
        #[cfg(oxide_module_integration_mcp_mattermost)]
        self.register_tool_runtime_module(catalog, &MattermostMcpToolModule, ctx);
        #[cfg(oxide_module_tool_compression)]
        self.register_tool_runtime_module(catalog, &CompressionToolModule, ctx);
        #[cfg(oxide_module_tool_retrieve_tools)]
        self.register_tool_runtime_module(catalog, &RetrieveToolsToolModule, ctx);
        #[cfg(oxide_module_tool_delegation)]
        self.register_tool_runtime_module(catalog, &DelegationToolModule, ctx);
        #[cfg(oxide_module_tool_file_delivery)]
        self.register_tool_runtime_module(catalog, &FileDeliveryToolModule, ctx);
        #[cfg(oxide_module_tool_audio_stt)]
        self.register_tool_runtime_module(catalog, &AudioSttToolModule, ctx);
        #[cfg(oxide_module_tool_vision_image)]
        self.register_tool_runtime_module(catalog, &VisionImageToolModule, ctx);
        #[cfg(oxide_module_tool_vision_video)]
        self.register_tool_runtime_module(catalog, &VisionVideoToolModule, ctx);
        #[cfg(oxide_module_tool_reminder)]
        self.register_tool_runtime_module(catalog, &ReminderToolModule, ctx);
        #[cfg(oxide_module_tool_browser_live)]
        let browser_cleanup = self.register_browser_live_module(catalog, ctx);

        #[cfg(not(oxide_module_tool_browser_live))]
        let browser_cleanup: Option<Arc<dyn BrowserSessionCleanup>> = None;

        #[cfg(oxide_module_integration_ssh_mcp)]
        self.register_tool_runtime_module(catalog, &SshMcpToolModule, ctx);
        #[cfg(oxide_module_tool_stack_logs)]
        self.register_tool_runtime_module(catalog, &StackLogsToolModule, ctx);
        #[cfg(oxide_module_tool_todos)]
        self.register_tool_runtime_module(catalog, &TodosToolModule, ctx);
        #[cfg(oxide_module_tool_tts_kokoro)]
        self.register_tool_runtime_module(catalog, &KokoroTtsToolModule, ctx);
        #[cfg(oxide_module_tool_tts_silero)]
        self.register_tool_runtime_module(catalog, &SileroTtsToolModule, ctx);
        #[cfg(oxide_module_tool_webfetch_md)]
        self.register_tool_runtime_module(catalog, &WebCrawlerToolModule, ctx);
        #[cfg(oxide_module_tool_web_search)]
        self.register_tool_runtime_module(catalog, &WebSearchToolModule, ctx);
        #[cfg(oxide_module_tool_webfetch_md)]
        self.register_tool_runtime_module(catalog, &WebFetchMdToolModule, ctx);
        #[cfg(oxide_module_tool_ytdlp)]
        self.register_tool_runtime_module(catalog, &YtdlpToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_exec)]
        self.register_tool_runtime_module(catalog, &SandboxExecToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_fileops)]
        self.register_tool_runtime_module(catalog, &SandboxFileOpsToolModule, ctx);
        #[cfg(oxide_module_tool_sandbox_recreate)]
        self.register_tool_runtime_module(catalog, &SandboxRecreateToolModule, ctx);

        browser_cleanup
    }

    /// Register browser-live tools and return the shared provider `Arc` for
    /// RAII cleanup after the parent agent run ends.
    #[cfg(oxide_module_tool_browser_live)]
    fn register_browser_live_module(
        &self,
        catalog: &mut ToolCatalog,
        ctx: &ToolModuleContext,
    ) -> Option<Arc<dyn BrowserSessionCleanup>> {
        let module = BrowserLiveToolModule;
        let module_id = module.module_id();
        if !self.settings.is_module_enabled(module_id.as_str()) {
            return None;
        }
        let provider = module.shared_provider(ctx)?;
        let browser_executors = provider.tool_runtime_executors();

        // Record group→tools mapping for the lazy tool surface.
        if let Some(group) = module.capability_group() {
            let names: Vec<ToolName> = browser_executors.iter().map(|e| e.name()).collect();
            ctx.tool_surface_handle().record_group_tools(group, names);
        }

        self.register_tool_runtime_executors(
            catalog,
            browser_executors,
            module_id,
            module.capability_group(),
            module.visibility(),
        );
        Some(provider)
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
        oxide_module_tool_retrieve_tools,
        oxide_module_tool_file_delivery,
        oxide_module_tool_audio_stt,
        oxide_module_tool_vision_image,
        oxide_module_tool_vision_video,
        oxide_module_tool_reminder,
        oxide_module_tool_browser_live,
        oxide_module_tool_stack_logs,
        oxide_module_tool_todos,
        oxide_module_tool_tts_kokoro,
        oxide_module_tool_tts_silero,
        oxide_module_tool_web_search,
        oxide_module_tool_webfetch_md,
        oxide_module_tool_ytdlp
    ))]
    fn register_tool_runtime_module<M>(
        &self,
        catalog: &mut ToolCatalog,
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
        let executors = module.tool_runtime_executors(ctx);

        // Record group→tools mapping for the lazy tool surface.
        // Only deferred modules with a capability group are recorded.
        if let Some(group) = module.capability_group() {
            let names: Vec<ToolName> = executors.iter().map(|e| e.name()).collect();
            ctx.tool_surface_handle().record_group_tools(group, names);
        }

        self.register_tool_runtime_executors(
            catalog,
            executors,
            module_id,
            module.capability_group(),
            module.visibility(),
        );
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
            oxide_module_tool_retrieve_tools,
            oxide_module_tool_file_delivery,
            oxide_module_tool_audio_stt,
            oxide_module_tool_vision_image,
            oxide_module_tool_vision_video,
            oxide_module_tool_reminder,
            oxide_module_tool_browser_live,
            oxide_module_tool_stack_logs,
            oxide_module_tool_todos,
            oxide_module_tool_tts_kokoro,
            oxide_module_tool_tts_silero,
            oxide_module_tool_web_search,
            oxide_module_tool_webfetch_md,
            oxide_module_tool_ytdlp
        )),
        allow(dead_code)
    )]
    fn register_tool_runtime_executors(
        &self,
        catalog: &mut ToolCatalog,
        executors: Vec<Arc<dyn ToolExecutor>>,
        module_id: ModuleId,
        capability_group: Option<CapabilityGroup>,
        visibility: ToolVisibility,
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
            let entry = ToolCatalogEntry::new(
                Arc::clone(&executor),
                module_id,
                capability_group,
                visibility,
            );
            if let Err(error) = catalog.register(entry) {
                warn!(
                    tool_name = %tool_name,
                    error = %error,
                    "Skipping duplicate typed tool catalog entry"
                );
                continue;
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
        let tool_surface_handle = Arc::new(ToolSurfaceHandle::new());
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
            progress_tx: progress_tx.cloned(),
            inherited_model: self.model_override().cloned(),
            tool_surface_handle,
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
