//! Agent Mode module for iterative task execution
//!
//! This module provides an agent system that can:
//! - Accept tasks from users (text, voice, images)
//! - Decompose and execute tasks iteratively
//! - Report progress via Telegram message updates
//! - Manage conversation memory with auto-compaction

/// Executor for iterative task processing
pub mod executor;
/// Hook system for intercepting agent events
pub mod hooks;
/// Memory management with auto-compaction
pub mod memory;
/// Preprocessor for different input types (voice, photo, etc)
pub mod preprocessor;
/// Prompt composition for system prompts
pub mod prompt;
/// Tool provider trait
pub mod provider;
/// Built-in tool providers (Sandbox, Tavily, Todos)
pub mod providers;
/// Recovery module for malformed LLM responses
pub mod recovery;
/// Registry for managing available tools
pub mod registry;
/// Agent session management
pub mod session;
/// Skill system for modular prompts
pub mod skills;
/// Tool execution bridge with timeout and cancellation
pub mod tool_bridge;

/// Loop detection subsystem
pub mod loop_detection;

/// Progress tracking and Telegram status updates
pub mod progress;

pub use executor::AgentExecutor;
pub use hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use loop_detection::{LoopDetectedEvent, LoopDetectionService, LoopType};
pub use memory::AgentMemory;
pub use progress::{AgentEvent, ProgressState};
pub use provider::ToolProvider;
pub use providers::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use recovery::sanitize_xml_tags;
pub use registry::ToolRegistry;
pub use session::{AgentSession, AgentStatus};
pub use skills::SkillRegistry;
