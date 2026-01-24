//! Agent Mode module for iterative task execution
//!
//! This module provides an agent system that can:
//! - Accept tasks from users (text, voice, images)
//! - Decompose and execute tasks iteratively
//! - Report progress via transport adapters
//! - Manage conversation memory with auto-compaction

/// Context abstractions for runner execution
pub mod context;
/// Executor for iterative task processing
pub mod executor;
/// Hook system for intercepting agent events
pub mod hooks;
/// Transport-agnostic agent identity types
pub mod identity;
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
/// Core agent runner (execution loop)
pub mod runner;
/// Agent session management
pub mod session;
/// Skill system for modular prompts
pub mod skills;
/// Structured output parsing and validation
pub mod structured_output;
/// Tool execution bridge with timeout and cancellation
pub mod tool_bridge;

/// Agent thought inference from tool calls
pub mod thoughts;

/// Narrator for human-readable status updates
pub mod narrator;

/// Loop detection subsystem
pub mod loop_detection;

/// Progress tracking and runtime events
pub mod progress;

pub use context::{AgentContext, EphemeralSession};
pub use executor::AgentExecutor;
pub use hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use identity::SessionId;
pub use loop_detection::{LoopDetectedEvent, LoopDetectionService, LoopType};
pub use memory::AgentMemory;
pub use progress::{AgentEvent, ProgressState};
pub use provider::ToolProvider;
pub use providers::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use recovery::sanitize_xml_tags;
pub use registry::ToolRegistry;
pub use runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
pub use session::{AgentSession, AgentStatus};
pub use skills::SkillRegistry;
