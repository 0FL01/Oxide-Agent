//! Runner configuration and context types.

use crate::agent::context::AgentContext;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::TodoList;
use crate::agent::registry::ToolRegistry;
use crate::agent::skills::SkillRegistry;
use crate::config::{get_agent_model, AGENT_CONTINUATION_LIMIT, AGENT_MAX_ITERATIONS};
use crate::llm::{Message, ToolDefinition};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Configuration for the agent runner.
#[derive(Debug, Clone)]
pub struct AgentRunnerConfig {
    /// Model name to use for LLM calls.
    pub model_name: String,
    /// Maximum iterations before aborting.
    pub max_iterations: usize,
    /// Maximum forced continuations before stopping.
    pub continuation_limit: usize,
}

impl AgentRunnerConfig {
    /// Create a new config with explicit values.
    #[must_use]
    pub fn new(model_name: String, max_iterations: usize, continuation_limit: usize) -> Self {
        Self {
            model_name,
            max_iterations,
            continuation_limit,
        }
    }
}

impl Default for AgentRunnerConfig {
    fn default() -> Self {
        Self::new(
            get_agent_model(),
            AGENT_MAX_ITERATIONS,
            AGENT_CONTINUATION_LIMIT,
        )
    }
}

/// Context for running the agent loop.
pub struct AgentRunnerContext<'a> {
    /// Original task prompt.
    pub task: &'a str,
    /// System prompt for the model.
    pub system_prompt: &'a str,
    /// Available tools for the model.
    pub tools: &'a [ToolDefinition],
    /// Tool registry for executing tool calls.
    pub registry: &'a ToolRegistry,
    /// Progress event channel.
    pub progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
    /// Shared todo list state.
    pub todos_arc: &'a Arc<Mutex<TodoList>>,
    /// Task ID for loop detection correlation.
    pub task_id: &'a str,
    /// Messages for the current LLM conversation.
    pub messages: &'a mut Vec<Message>,
    /// Agent context abstraction (memory + cancellation).
    pub agent: &'a mut dyn AgentContext,
    /// Optional skill registry for dynamic skill injection.
    pub skill_registry: Option<&'a mut SkillRegistry>,
    /// Runner configuration.
    pub config: AgentRunnerConfig,
}

/// Internal run state for the current loop execution.
pub(super) struct RunState {
    /// Current iteration index.
    pub iteration: usize,
    /// Number of forced continuations so far.
    pub continuation_count: usize,
}

impl RunState {
    /// Create a new run state initialized to zero.
    pub(super) fn new() -> Self {
        Self {
            iteration: 0,
            continuation_count: 0,
        }
    }
}

/// Structured output parsing failure payload.
pub(super) struct StructuredOutputFailure {
    /// Parsing/validation error.
    pub error: crate::agent::structured_output::StructuredOutputError,
    /// Raw JSON string from the model.
    pub raw_json: String,
}

/// Final response payload from the model.
pub(super) struct FinalResponseInput {
    /// Final answer text.
    pub final_answer: String,
    /// Raw JSON string from the model.
    pub raw_json: String,
    /// Optional reasoning content from the model.
    pub reasoning: Option<String>,
}
