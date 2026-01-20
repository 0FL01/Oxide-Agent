use super::loop_detection::LoopType;
use super::providers::TodoList;
use super::thoughts;
use serde::{Deserialize, Serialize};

/// Events that can occur during agent execution
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent is thinking about the next step
    Thinking {
        /// Current token count in memory
        tokens: usize,
    },
    /// Agent is calling a tool
    ToolCall {
        /// Tool name
        name: String,
        /// Tool input arguments
        input: String,
        /// Human-readable preview (e.g., command for execute_command)
        command_preview: Option<String>,
    },
    /// Agent received a tool result
    ToolResult {
        /// Tool name
        name: String,
        /// Tool execution output
        output: String,
    },
    /// Agent is continuing work due to incomplete todos
    Continuation {
        /// Reason for continuation
        reason: String,
        /// Number of continuations so far
        count: usize,
    },
    /// Todos list was updated
    TodosUpdated {
        /// Updated list of tasks
        todos: TodoList,
    },
    /// File to send to user
    FileToSend {
        /// Original file name
        file_name: String,
        /// Raw file content
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
    },
    /// File to send to user with delivery confirmation
    /// Used by ytdlp provider for automatic cleanup after successful delivery
    #[serde(skip)]
    FileToSendWithConfirmation {
        /// Original file name
        file_name: String,
        /// Raw file content
        content: Vec<u8>,
        /// Path in sandbox for cleanup after success
        sandbox_path: String,
        /// Channel to receive delivery confirmation
        confirmation_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
    },
    /// Agent has finished the task
    Finished,
    /// Agent is being cancelled (cleanup in progress)
    Cancelling {
        /// Tool that was interrupted
        tool_name: String,
    },
    /// Agent was cancelled by user
    Cancelled,
    /// Agent encountered an error
    Error(String),
    /// Agent's reasoning/thinking process (for models that support it)
    Reasoning {
        /// Short summary of reasoning
        summary: String,
    },
    /// Loop detected during execution
    LoopDetected {
        /// Type of loop detected
        loop_type: LoopType,
        /// Iteration when detected
        iteration: usize,
    },
    /// Narrative update from sidecar LLM
    Narrative {
        /// Short action-oriented headline
        headline: String,
        /// Detailed context explanation
        content: String,
    },
}

/// Current state of the agent's progress
#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    /// Index of current iteration
    pub current_iteration: usize,
    /// Maximum allowed iterations
    pub max_iterations: usize,
    /// List of steps executed so far
    pub steps: Vec<Step>,
    /// Optional list of todos/tasks
    pub current_todos: Option<TodoList>,
    /// Whether the agent has finished
    pub is_finished: bool,
    /// Optional error message
    pub error: Option<String>,
    /// Current agent thought/reasoning
    pub current_thought: Option<String>,
    /// Narrative headline from sidecar LLM
    pub narrative_headline: Option<String>,
    /// Narrative content from sidecar LLM
    pub narrative_content: Option<String>,
}

/// A single step in the agent's execution process
#[derive(Debug, Clone)]
pub struct Step {
    /// Human-readable description of the step
    pub description: String,
    /// Current status of the step
    pub status: StepStatus,
    /// Optional token count at this step
    pub tokens: Option<usize>,
    /// Tool name for grouping (None for non-tool steps like Thinking)
    pub tool_name: Option<String>,
}

/// Possible statuses for an execution step
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// Step is waiting to be executed
    Pending,
    /// Step is currently being executed
    InProgress,
    /// Step was completed successfully
    Completed,
    /// Step failed
    Failed,
}

impl ProgressState {
    /// Creates a new empty progress state
    #[must_use]
    pub fn new(max_iterations: usize) -> Self {
        Self {
            max_iterations,
            ..Default::default()
        }
    }

    /// Updates the progress state based on an agent event
    pub fn update(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Thinking { tokens } => self.handle_thinking(tokens),
            AgentEvent::ToolCall {
                name,
                input,
                command_preview,
            } => self.handle_tool_call(name, input, command_preview),
            AgentEvent::ToolResult { .. } => self.complete_last_step(),
            AgentEvent::Continuation { reason, count } => self.handle_continuation(reason, count),
            AgentEvent::TodosUpdated { todos } => self.handle_todos_update(todos),
            AgentEvent::FileToSend { file_name, .. } => self.handle_file_send(file_name),
            AgentEvent::FileToSendWithConfirmation { file_name, .. } => {
                self.handle_file_send(file_name)
            }
            AgentEvent::Finished => self.handle_finish(),
            AgentEvent::Cancelling { tool_name } => self.handle_cancelling(tool_name),
            AgentEvent::Cancelled => self.handle_cancelled(),
            AgentEvent::Error(e) => self.handle_error(e),
            AgentEvent::Reasoning { summary } => self.handle_reasoning(summary),
            AgentEvent::LoopDetected {
                loop_type,
                iteration,
            } => self.handle_loop_detected(loop_type, iteration),
            AgentEvent::Narrative { headline, content } => self.handle_narrative(headline, content),
        }
    }

    /// Helper: Complete the last in-progress step
    fn complete_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.status = StepStatus::Completed;
            }
        }
    }

    /// Helper: Mark the last in-progress step as failed
    fn fail_last_step(&mut self) {
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.status = StepStatus::Failed;
            }
        }
    }

    fn handle_thinking(&mut self, tokens: usize) {
        self.current_iteration += 1;
        self.complete_last_step();
        self.steps.push(Step {
            description: format!(
                "Task analysis (iteration {}/{})",
                self.current_iteration, self.max_iterations
            ),
            status: StepStatus::InProgress,
            tokens: Some(tokens),
            tool_name: None,
        });
    }

    fn handle_tool_call(&mut self, name: String, input: String, command_preview: Option<String>) {
        self.complete_last_step();

        // Try to infer a human-readable thought from tool call
        let inferred_thought = thoughts::infer_thought(&name, &input);

        // Update current thought with inferred thought or command preview
        if let Some(ref thought) = inferred_thought {
            self.current_thought = Some(thought.clone());
        } else if let Some(ref preview) = command_preview {
            self.current_thought = Some(thoughts::infer_thought_from_command(preview));
        }

        // Use command preview if available, otherwise show tool name
        let description = command_preview
            .map(|preview| format!("🔧 {}", crate::utils::truncate_str(preview, 60)))
            .unwrap_or_else(|| format!("Execution: {}", &name));

        self.steps.push(Step {
            description,
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: Some(name),
        });
    }

    fn handle_continuation(&mut self, reason: String, count: usize) {
        self.complete_last_step();
        self.steps.push(Step {
            description: format!(
                "🔄 Continuation ({}/{}): {}",
                count,
                crate::config::AGENT_CONTINUATION_LIMIT,
                crate::utils::truncate_str(reason, 50)
            ),
            status: StepStatus::InProgress,
            tokens: None,
            tool_name: None,
        });
    }

    fn handle_todos_update(&mut self, todos: TodoList) {
        let current_task = todos.current_task().map(|t| t.description.clone());
        let completed = todos.completed_count();
        let total = todos.items.len();

        self.current_todos = Some(todos);

        if let Some(task) = current_task {
            // Update step description with current task
            if let Some(last) = self.steps.last_mut() {
                if last.status == StepStatus::InProgress {
                    last.description = format!("📋 {task} ({completed}/{total})");
                }
            }
        }
    }

    fn handle_file_send(&mut self, file_name: String) {
        self.steps.push(Step {
            description: format!("📤 File send: {file_name}"),
            status: StepStatus::Completed,
            tokens: None,
            tool_name: Some("file_send".to_string()),
        });
    }

    fn handle_finish(&mut self) {
        self.is_finished = true;
        self.current_thought = None; // Clear thought on finish
        for step in &mut self.steps {
            if step.status == StepStatus::InProgress {
                step.status = StepStatus::Completed;
            }
        }
    }

    fn handle_reasoning(&mut self, summary: String) {
        // Only update if reasoning is meaningful (>20 chars)
        // Otherwise keep the previous inferred thought from tool call
        if summary.len() >= 20 {
            self.current_thought = Some(summary);
        }
    }

    fn handle_cancelling(&mut self, tool_name: String) {
        // Add a step showing cancellation is in progress
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.description = format!("⏹ Cancellation: {tool_name}...");
            }
        } else {
            self.steps.push(Step {
                description: format!("⏹ Cancellation: {tool_name}..."),
                status: StepStatus::InProgress,
                tokens: None,
                tool_name: None,
            });
        }
    }

    fn handle_cancelled(&mut self) {
        self.error = Some("Task cancelled by user".to_string());
        self.fail_last_step();
    }

    fn handle_error(&mut self, e: String) {
        self.error = Some(e);
        self.fail_last_step();
    }

    fn handle_loop_detected(&mut self, loop_type: LoopType, iteration: usize) {
        let label = match loop_type {
            LoopType::ToolCallLoop => "Recurring calls",
            LoopType::ContentLoop => "Recurring text",
            LoopType::CognitiveLoop => "Stuck",
        };
        self.error = Some(format!("Loop detected: {label} (iteration {iteration})"));
        self.fail_last_step();
    }

    fn handle_narrative(&mut self, headline: String, content: String) {
        self.narrative_headline = Some(headline);
        self.narrative_content = Some(content);
    }

    // Formatting is handled in the UI layer.
}
