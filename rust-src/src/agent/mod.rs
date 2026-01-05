//! Agent Mode module for iterative task execution
//!
//! This module provides an agent system that can:
//! - Accept tasks from users (text, voice, images)
//! - Decompose and execute tasks iteratively
//! - Report progress via Telegram message updates
//! - Manage conversation memory with auto-compaction

pub mod executor;
pub mod hooks;
pub mod memory;
pub mod preprocessor;
pub mod provider;
pub mod providers;
pub mod registry;
pub mod session;

/// Progress tracking and Telegram status updates
pub mod progress;

pub use executor::AgentExecutor;
pub use hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use memory::AgentMemory;
pub use progress::{AgentEvent, ProgressState};
pub use provider::ToolProvider;
pub use providers::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use registry::ToolRegistry;
pub use session::{AgentSession, AgentStatus};
