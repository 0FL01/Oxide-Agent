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
/// Memory management for agent conversation history
pub mod memory;
/// Preprocessor for different input types (voice, photo, etc)
pub mod preprocessor;
/// Execution profile parsing and policy helpers
pub mod profile;
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
/// Task-local tool execution runtime metadata
pub(crate) mod tool_runtime;

/// Agent thought inference from tool calls
pub mod thoughts;

/// Narrator for human-readable status updates
pub mod narrator;

/// Loop detection subsystem
pub mod loop_detection;

/// Progress tracking and runtime events
pub mod progress;

pub use compaction::{
    classify_hot_memory, estimate_request_budget, externalize_hot_memory,
    persist_compacted_history_chunk, prune_hot_memory, rebuild_hot_context, AgentMessageKind,
    ArchiveChunk, ArchivePersistenceOutcome, ArchiveRecord, ArchiveRef, ArchiveSink,
    BudgetEstimate, BudgetState, ClassifiedMemoryEntry, CompactionClassSummary, CompactionOutcome,
    CompactionPolicy, CompactionRequest, CompactionRetention, CompactionScope, CompactionService,
    CompactionSnapshot, CompactionSummarizer, CompactionSummarizerConfig, CompactionSummary,
    CompactionTrigger, ExternalizationOutcome, ExternalizedPayloadRecord, HotMemoryBudget,
    NoopArchiveSink, NoopPayloadSink, PayloadSink, PruneOutcome, RebuildOutcome, RecentRawWindow,
    SummaryGenerationOutcome,
};
pub use context::{AgentContext, EphemeralSession};
pub use executor::{AgentExecutionOutcome, AgentExecutor};
pub use hooks::{CompletionCheckHook, Hook, HookContext, HookEvent, HookRegistry, HookResult};
pub use identity::SessionId;
pub use loop_detection::{LoopDetectedEvent, LoopDetectionService, LoopType};
pub use memory::{AgentMemory, ExternalizedPayload, PrunedArtifact};
pub use profile::{
    dm_default_blocked_tools, dm_tool_policy, manager_default_blocked_tools, parse_agent_profile,
    topic_agent_all_hooks, topic_agent_default_blocked_tools, topic_agent_manageable_hooks,
    topic_agent_protected_hooks, AgentExecutionProfile, HookAccessPolicy, ParsedAgentProfile,
    ToolAccessPolicy,
};
pub use progress::{AgentEvent, ProgressState, RepeatedCompactionKind};
pub use provider::ToolProvider;
pub use providers::{SshApprovalGrant, SshApprovalRequestView};
pub use providers::{TodoItem, TodoList, TodoStatus, TodosProvider};
pub use recovery::sanitize_xml_tags;
pub use registry::ToolRegistry;
pub use runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
pub use session::{
    AgentMemoryCheckpoint, AgentMemoryScope, AgentSession, AgentStatus, PendingSshReplay,
    PendingUserInput, RuntimeContextInbox, RuntimeContextInjection, UserInputKind,
};
pub use skills::SkillRegistry;
