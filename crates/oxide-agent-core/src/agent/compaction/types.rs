//! Shared types for Agent Mode context compaction.

use crate::config::AGENT_COMPACT_THRESHOLD;
use crate::llm::ToolDefinition;

/// Trigger point for a compaction pipeline invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// Compaction check before the first model request of a run.
    PreRun,
    /// Compaction check before a later loop iteration.
    PreIteration,
    /// Explicit manual compaction requested by the operator or transport.
    Manual,
}

/// Static policy knobs for the compaction subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// Legacy token threshold kept for the Stage 1 transition period.
    pub legacy_compact_threshold: usize,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            legacy_compact_threshold: AGENT_COMPACT_THRESHOLD,
        }
    }
}

/// Immutable request payload describing a compaction checkpoint.
#[derive(Debug, Clone)]
pub struct CompactionRequest<'a> {
    /// Why the pipeline was invoked.
    pub trigger: CompactionTrigger,
    /// User-visible task text.
    pub task: &'a str,
    /// Current fully rendered system prompt.
    pub system_prompt: &'a str,
    /// Tool definitions exposed to the model.
    pub tools: &'a [ToolDefinition],
    /// Active model name for the main agent request.
    pub model_name: &'a str,
    /// Whether the current execution is a sub-agent.
    pub is_sub_agent: bool,
}

impl<'a> CompactionRequest<'a> {
    /// Build a request for a compaction checkpoint.
    #[must_use]
    pub const fn new(
        trigger: CompactionTrigger,
        task: &'a str,
        system_prompt: &'a str,
        tools: &'a [ToolDefinition],
        model_name: &'a str,
        is_sub_agent: bool,
    ) -> Self {
        Self {
            trigger,
            task,
            system_prompt,
            tools,
            model_name,
            is_sub_agent,
        }
    }
}

/// Observable result of a compaction checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionOutcome {
    /// Trigger that produced this outcome.
    pub trigger: CompactionTrigger,
    /// Whether the pipeline mutated hot memory.
    pub applied: bool,
    /// Token count before the pipeline ran.
    pub token_count_before: usize,
    /// Token count after the pipeline ran.
    pub token_count_after: usize,
}

impl CompactionOutcome {
    /// Build a no-op outcome.
    #[must_use]
    pub const fn noop(trigger: CompactionTrigger, token_count: usize) -> Self {
        Self {
            trigger,
            applied: false,
            token_count_before: token_count,
            token_count_after: token_count,
        }
    }
}
