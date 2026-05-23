//! Capability-oriented typed tool runtime modules.

use super::ToolExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::TodoList;
use crate::capabilities::ModuleId;
use crate::sandbox::SandboxScope;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};

#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
))]
use crate::agent::providers::SandboxProvider;
#[cfg(feature = "tool-todos")]
use crate::agent::providers::TodosProvider;

/// Runtime context passed to typed tool capability modules.
pub struct ToolRuntimeModuleContext {
    todos: Arc<Mutex<TodoList>>,
    sandbox_scope: SandboxScope,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl ToolRuntimeModuleContext {
    /// Creates a typed runtime module context.
    #[must_use]
    pub fn new(
        todos: Arc<Mutex<TodoList>>,
        sandbox_scope: SandboxScope,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Self {
        Self {
            todos,
            sandbox_scope,
            progress_tx,
        }
    }

    /// Shared todo list state for modules that own todo tools.
    #[must_use]
    pub fn todos(&self) -> Arc<Mutex<TodoList>> {
        Arc::clone(&self.todos)
    }

    /// Sandbox scope for modules that own sandbox tools.
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

/// Typed runtime capability module.
pub trait ToolRuntimeModule {
    /// Stable module ID corresponding to the compiled capability manifest.
    fn module_id(&self) -> ModuleId;

    /// Builds typed tool executors owned by this module.
    fn tool_runtime_executors(&self, ctx: &ToolRuntimeModuleContext) -> Vec<Arc<dyn ToolExecutor>>;
}

/// Capability module for the `write_todos` typed runtime tool.
#[cfg(feature = "tool-todos")]
pub struct TodosToolRuntimeModule;

#[cfg(feature = "tool-todos")]
impl ToolRuntimeModule for TodosToolRuntimeModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/todos")
    }

    fn tool_runtime_executors(&self, ctx: &ToolRuntimeModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        Arc::new(TodosProvider::new(ctx.todos())).tool_runtime_executors(ctx.progress_tx())
    }
}

/// Capability module for sandbox command execution.
#[cfg(feature = "tool-sandbox-exec")]
pub struct SandboxExecToolRuntimeModule;

#[cfg(feature = "tool-sandbox-exec")]
impl ToolRuntimeModule for SandboxExecToolRuntimeModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-exec")
    }

    fn tool_runtime_executors(&self, ctx: &ToolRuntimeModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(ctx, &["execute_command"])
    }
}

/// Capability module for sandbox file operations and file delivery.
#[cfg(feature = "tool-sandbox-fileops")]
pub struct SandboxFileOpsToolRuntimeModule;

#[cfg(feature = "tool-sandbox-fileops")]
impl ToolRuntimeModule for SandboxFileOpsToolRuntimeModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-fileops")
    }

    fn tool_runtime_executors(&self, ctx: &ToolRuntimeModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(
            ctx,
            &["write_file", "read_file", "send_file_to_user", "list_files"],
        )
    }
}

/// Capability module for sandbox recreation.
#[cfg(feature = "tool-sandbox-recreate")]
pub struct SandboxRecreateToolRuntimeModule;

#[cfg(feature = "tool-sandbox-recreate")]
impl ToolRuntimeModule for SandboxRecreateToolRuntimeModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/sandbox-recreate")
    }

    fn tool_runtime_executors(&self, ctx: &ToolRuntimeModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        sandbox_tool_runtime_executors(ctx, &["recreate_sandbox"])
    }
}

#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-recreate"
))]
fn sandbox_tool_runtime_executors(
    ctx: &ToolRuntimeModuleContext,
    owned_tool_names: &[&str],
) -> Vec<Arc<dyn ToolExecutor>> {
    let provider = if let Some(tx) = ctx.progress_tx() {
        SandboxProvider::new(ctx.sandbox_scope()).with_progress_tx(tx)
    } else {
        SandboxProvider::new(ctx.sandbox_scope())
    };

    Arc::new(provider)
        .tool_runtime_executors()
        .into_iter()
        .filter(|executor| {
            let name = executor.name();
            owned_tool_names.iter().any(|owned| *owned == name.as_str())
        })
        .collect()
}
