//! Capability-oriented tool modules.

use super::ToolExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::{SandboxProvider, TodoList};
use crate::capabilities::ModuleId;
use crate::sandbox::SandboxScope;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};

#[cfg(feature = "tool-compression")]
use crate::agent::providers::CompressionProvider;
#[cfg(feature = "tool-file-delivery")]
use crate::agent::providers::FileHosterProvider;
#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
))]
use crate::agent::providers::FilteredToolProvider;
#[cfg(feature = "tool-stack-logs")]
use crate::agent::providers::StackLogsProvider;
#[cfg(feature = "tool-todos")]
use crate::agent::providers::TodosProvider;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::providers::WebFetchMdProvider;
#[cfg(feature = "tool-ytdlp")]
use crate::agent::providers::YtdlpProvider;

/// Runtime context passed to tool capability modules.
pub struct ToolModuleContext {
    todos: Arc<Mutex<TodoList>>,
    sandbox_scope: SandboxScope,
    sandbox_provider: Arc<SandboxProvider>,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl ToolModuleContext {
    /// Creates a tool module context.
    #[must_use]
    pub fn new(
        todos: Arc<Mutex<TodoList>>,
        sandbox_scope: SandboxScope,
        sandbox_provider: Arc<SandboxProvider>,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Self {
        Self {
            todos,
            sandbox_scope,
            sandbox_provider,
            progress_tx,
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
