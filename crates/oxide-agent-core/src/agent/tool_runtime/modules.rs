//! Capability-oriented typed tool runtime modules.

use super::ToolExecutor;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::TodoList;
use crate::capabilities::ModuleId;
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, Mutex};

#[cfg(feature = "tool-todos")]
use crate::agent::providers::TodosProvider;

/// Runtime context passed to typed tool capability modules.
pub struct ToolRuntimeModuleContext {
    todos: Arc<Mutex<TodoList>>,
    progress_tx: Option<Sender<AgentEvent>>,
}

impl ToolRuntimeModuleContext {
    /// Creates a typed runtime module context.
    #[must_use]
    pub fn new(todos: Arc<Mutex<TodoList>>, progress_tx: Option<Sender<AgentEvent>>) -> Self {
        Self { todos, progress_tx }
    }

    /// Shared todo list state for modules that own todo tools.
    #[must_use]
    pub fn todos(&self) -> Arc<Mutex<TodoList>> {
        Arc::clone(&self.todos)
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
