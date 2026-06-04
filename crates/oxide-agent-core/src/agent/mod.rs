//! Agent Mode module for iterative task execution
//!
//! This module provides an agent system that can:
//! - Accept tasks from users (text, voice, images)
//! - Decompose and execute tasks iteratively
//! - Report progress via transport adapters
//! - Manage conversation memory for long-running agent tasks

/// Context abstractions for runner execution
/// Context compaction orchestration and extension points
pub mod compaction;
/// Context abstractions for runner execution
pub mod context;
/// Executor for iterative task processing
pub mod executor;
/// Hook system for intercepting agent events
pub mod hooks;
/// Transport-agnostic agent identity types
pub mod identity;
/// Core Agent Mode input intent classification helpers.
pub mod input_intent;
/// Memory management for agent conversation history
pub mod memory;
/// Task-local memory behavior signals for wiki updates.
pub mod memory_behavior;
/// Preprocessor for different input types (voice, photo, etc)
pub mod preprocessor;
/// Execution profile parsing and policy helpers
pub mod profile;
/// Prompt composition for system prompts
pub mod prompt;
/// Built-in tool providers (Sandbox, Tavily, Todos)
pub mod providers;
/// Recovery module for malformed LLM responses
pub mod recovery;
/// Core agent runner (execution loop)
pub mod runner;
/// Agent session management
pub mod session;
/// Structured output parsing and validation
pub mod structured_output;
/// Deterministic summaries for noisy dead-end tool failures.
pub(crate) mod tool_failure_summary;
/// Async parallel tool runtime foundations.
pub mod tool_runtime;
/// Durable LLM Wiki memory primitives.
pub mod wiki_memory;

/// Agent thought inference from tool calls
pub mod thoughts;

/// Loop detection subsystem
pub mod loop_detection;

/// Progress tracking and runtime events
pub mod progress;

pub use compaction::{
    AgentMessageKind, ArchiveRef, BudgetEstimate, BudgetState, CompactRequestContext,
    CompactRunOutcome, CompactSummaryBackend, CompactSummaryError, CompactSummaryRequest,
    CompactSummaryResult, CompactedSummaryMetadata, CompactionBackend, CompactionController,
    CompactionControllerError, CompactionPhase, CompactionReason, CompactionTrigger,
    HotMemoryBudget, LocalLlmSummary,
};
pub use context::{AgentContext, EphemeralSession};
pub use executor::{
    AgentExecutionEffort, AgentExecutionOptions, AgentExecutionOutcome, AgentExecutor,
    AgentUserInput,
};
pub use hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use identity::SessionId;
pub use input_intent::{
    classify_agent_input_intent, AgentInputIntentClassification, AgentInputIntentSnapshot,
    AgentInputSessionStatus,
};
pub use loop_detection::{LoopDetectedEvent, LoopDetectionService, LoopType};
pub use memory::{
    AgentMemory, AgentMessageAttachment, AgentMessageAttachmentKind, ExternalizedPayload,
    PrunedArtifact,
};
pub use memory_behavior::{
    MemoryBehaviorRuntime, ToolDerivedMemoryDraft, ToolDerivedMemoryKind, TopicMemoryPolicy,
};
pub use profile::{
    dm_default_blocked_tools, dm_tool_policy, manager_default_blocked_tools, parse_agent_profile,
    topic_agent_all_hooks, topic_agent_default_blocked_tools, topic_agent_manageable_hooks,
    topic_agent_protected_hooks, AgentExecutionProfile, HookAccessPolicy, ParsedAgentProfile,
    ToolAccessPolicy,
};
pub use progress::{AgentEvent, ProgressState, RepeatedCompactionKind};
pub use providers::{SshApprovalGrant, SshApprovalRequestView};
pub use providers::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use recovery::sanitize_xml_tags;
pub use runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
pub use session::{
    AgentMemoryCheckpoint, AgentMemoryScope, AgentSession, AgentStatus, PendingUserInput,
    RuntimeContextInbox, RuntimeContextInjection, UserInputKind,
};
pub use wiki_memory::WikiStore;
