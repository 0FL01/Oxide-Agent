//! Hook Types - events, results, and context for the hook system
//!
//! Defines the data structures used for agent lifecycle hooks.

use super::super::providers::TodoList;

/// Events in the agent lifecycle that can trigger hooks
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// Before the agent starts processing a user prompt
    BeforeAgent {
        /// The user's prompt
        prompt: String,
    },

    /// Before the agent begins a new iteration
    BeforeIteration {
        /// Iteration index
        iteration: usize,
    },

    /// After the agent produces a response (when no more tool calls)
    AfterAgent {
        /// The agent's response text
        response: String,
    },

    /// Before a tool is executed
    BeforeTool {
        /// Name of the tool being called
        tool_name: String,
        /// JSON arguments for the tool
        arguments: String,
    },

    /// After a tool has been executed
    AfterTool {
        /// Name of the tool that was called
        tool_name: String,
        /// Result returned by the tool
        result: String,
    },
}

/// Result of executing a hook
#[derive(Debug, Clone, Default)]
pub enum HookResult {
    /// Continue with normal execution
    #[default]
    Continue,

    /// Inject additional context into the next LLM request
    InjectContext(String),

    /// Force the agent to continue iterating instead of returning
    ForceIteration {
        /// Reason for forcing continuation (shown in logs)
        reason: String,
        /// Optional context to inject into the prompt
        context: Option<String>,
    },

    /// Block the action (for `BeforeTool` hooks)
    Block {
        /// Reason for blocking
        reason: String,
    },
}

/// Context provided to hooks during execution
pub struct HookContext<'a> {
    /// Current todo list
    pub todos: &'a TodoList,
    /// Current iteration number in the agent loop
    pub iteration: usize,
    /// Number of times the agent has been forced to continue
    pub continuation_count: usize,
    /// Maximum allowed continuations before stopping
    pub max_continuations: usize,
    /// Current token count in memory
    pub token_count: usize,
    /// Maximum allowed tokens for memory
    pub max_tokens: usize,
}

impl<'a> HookContext<'a> {
    /// Create a new hook context
    #[must_use]
    pub const fn new(
        todos: &'a TodoList,
        iteration: usize,
        continuation_count: usize,
        max_continuations: usize,
    ) -> Self {
        Self {
            todos,
            iteration,
            continuation_count,
            max_continuations,
            token_count: 0,
            max_tokens: usize::MAX,
        }
    }

    /// Add token usage metadata to the hook context.
    #[must_use]
    pub const fn with_tokens(mut self, token_count: usize, max_tokens: usize) -> Self {
        self.token_count = token_count;
        self.max_tokens = max_tokens;
        self
    }

    /// Check if we've reached the continuation limit
    #[must_use]
    pub const fn at_continuation_limit(&self) -> bool {
        self.continuation_count >= self.max_continuations
    }
}
