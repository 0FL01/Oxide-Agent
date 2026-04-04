//! Runner configuration and context types.

use crate::agent::compaction::CompactionService;
use crate::agent::context::AgentContext;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::TodoList;
use crate::agent::registry::ToolRegistry;
use crate::agent::session::PendingUserInput;
use crate::agent::skills::SkillRegistry;
use crate::config::{
    get_agent_max_iterations, get_agent_model, ModelInfo, AGENT_CONTINUATION_LIMIT,
};
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
    /// Whether this runner is for a sub-agent.
    pub is_sub_agent: bool,
    /// Soft timeout in seconds.
    pub timeout_secs: u64,
    /// Reserved output token budget for the active model.
    pub model_max_output_tokens: u32,
    /// Active provider name for the current model.
    pub model_provider: Option<String>,
    /// Optional weighted fallback routes for this execution.
    pub model_routes: Vec<ModelInfo>,
}

impl AgentRunnerConfig {
    /// Create a new config with explicit values.
    #[must_use]
    pub fn new(
        model_name: String,
        max_iterations: usize,
        continuation_limit: usize,
        timeout_secs: u64,
        model_max_output_tokens: u32,
    ) -> Self {
        Self {
            model_name,
            max_iterations,
            continuation_limit,
            is_sub_agent: false,
            timeout_secs,
            model_max_output_tokens,
            model_provider: None,
            model_routes: Vec::new(),
        }
    }

    /// Set whether this runner is for a sub-agent.
    #[must_use]
    pub fn with_sub_agent(mut self, is_sub_agent: bool) -> Self {
        self.is_sub_agent = is_sub_agent;
        self
    }

    /// Set the active provider name.
    #[must_use]
    pub fn with_model_provider(mut self, model_provider: impl Into<String>) -> Self {
        self.model_provider = Some(model_provider.into());
        self
    }

    /// Set weighted fallback routes for the execution.
    #[must_use]
    pub fn with_model_routes(mut self, model_routes: Vec<ModelInfo>) -> Self {
        self.model_routes = model_routes;
        self
    }
}

impl Default for AgentRunnerConfig {
    fn default() -> Self {
        Self::new(
            get_agent_model(),
            get_agent_max_iterations(),
            AGENT_CONTINUATION_LIMIT,
            crate::config::AGENT_TIMEOUT_SECS,
            0,
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
    /// Optional compaction service for pre-turn context maintenance.
    pub compaction_service: Option<&'a CompactionService>,
    /// Runner configuration.
    pub config: AgentRunnerConfig,
}

/// Terminal outcome of a runner execution.
pub enum AgentRunResult {
    /// The agent produced a final response for the user.
    Final(String),
    /// The agent paused because an external approval is required.
    WaitingForApproval,
    /// The agent paused because it requires additional user input.
    WaitingForUserInput(PendingUserInput),
}

/// Internal run state for the current loop execution.
pub(super) struct RunState {
    /// Current iteration index.
    pub iteration: usize,
    /// Number of forced continuations so far.
    pub continuation_count: usize,
    /// Number of consecutive structured output failures.
    pub structured_output_failures: usize,
    /// Number of applied compaction passes in this run.
    pub compaction_count: usize,
    /// Number of deterministic cleanup passes in this run.
    pub cleanup_count: usize,
    /// Whether the next pre-LLM turn should run manual compaction.
    pub force_manual_compaction: bool,
}

impl RunState {
    /// Create a new run state initialized to zero.
    pub(super) fn new() -> Self {
        Self {
            iteration: 0,
            continuation_count: 0,
            structured_output_failures: 0,
            compaction_count: 0,
            cleanup_count: 0,
            force_manual_compaction: false,
        }
    }

    /// Request a manual compaction pass before the next model call.
    pub(super) fn request_manual_compaction(&mut self) {
        self.force_manual_compaction = true;
    }

    /// Consume any pending manual compaction request.
    pub(super) fn take_manual_compaction_request(&mut self) -> bool {
        let requested = self.force_manual_compaction;
        self.force_manual_compaction = false;
        requested
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
