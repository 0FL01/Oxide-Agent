use super::providers::TodoList;
use serde::{Deserialize, Serialize};

/// Events that can occur during agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// File to send to user via Telegram
    FileToSend {
        /// Original file name
        file_name: String,
        /// Raw file content
        #[serde(with = "serde_bytes")]
        content: Vec<u8>,
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
}

/// Maximum number of visible steps in Telegram progress report
const MAX_VISIBLE_STEPS: usize = 30;

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
            AgentEvent::ToolCall { name, .. } => self.handle_tool_call(name),
            AgentEvent::ToolResult { .. } => self.complete_last_step(),
            AgentEvent::Continuation { reason, count } => self.handle_continuation(reason, count),
            AgentEvent::TodosUpdated { todos } => self.handle_todos_update(todos),
            AgentEvent::FileToSend { file_name, .. } => self.handle_file_send(file_name),
            AgentEvent::Finished => self.handle_finish(),
            AgentEvent::Cancelling { tool_name } => self.handle_cancelling(tool_name),
            AgentEvent::Cancelled => self.handle_cancelled(),
            AgentEvent::Error(e) => self.handle_error(e),
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
                "Анализ задачи (итерация {}/{})",
                self.current_iteration, self.max_iterations
            ),
            status: StepStatus::InProgress,
            tokens: Some(tokens),
        });
    }

    fn handle_tool_call(&mut self, name: String) {
        self.complete_last_step();
        self.steps.push(Step {
            description: format!("Выполнение: {name}"),
            status: StepStatus::InProgress,
            tokens: None,
        });
    }

    fn handle_continuation(&mut self, reason: String, count: usize) {
        self.complete_last_step();
        self.steps.push(Step {
            description: format!(
                "🔄 Продолжение ({}/{}): {}",
                count,
                crate::config::AGENT_CONTINUATION_LIMIT,
                crate::utils::truncate_str(reason, 50)
            ),
            status: StepStatus::InProgress,
            tokens: None,
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
            description: format!("📤 Отправка файла: {file_name}"),
            status: StepStatus::Completed,
            tokens: None,
        });
    }

    fn handle_finish(&mut self) {
        self.is_finished = true;
        for step in &mut self.steps {
            if step.status == StepStatus::InProgress {
                step.status = StepStatus::Completed;
            }
        }
    }

    fn handle_cancelling(&mut self, tool_name: String) {
        // Add a step showing cancellation is in progress
        if let Some(last) = self.steps.last_mut() {
            if last.status == StepStatus::InProgress {
                last.description = format!("⏹ Прерывание: {tool_name}...");
            }
        } else {
            self.steps.push(Step {
                description: format!("⏹ Прерывание: {tool_name}..."),
                status: StepStatus::InProgress,
                tokens: None,
            });
        }
    }

    fn handle_cancelled(&mut self) {
        self.error = Some("Задача отменена пользователем".to_string());
        self.fail_last_step();
    }

    fn handle_error(&mut self, e: String) {
        self.error = Some(e);
        self.fail_last_step();
    }

    /// Formats the progress state into a HTML message for Telegram
    #[must_use]
    pub fn format_telegram(&self) -> String {
        let mut lines = Vec::new();
        lines.push("🤖 <b>Работа агента</b>\n".to_string());

        // Todos status if available
        if let Some(ref todos) = self.current_todos {
            if !todos.items.is_empty() {
                lines.push(format!(
                    "<b>План задач ({}/{}):</b>",
                    todos.completed_count(),
                    todos.items.len()
                ));
                for (i, item) in todos.items.iter().enumerate() {
                    lines.push(format!(
                        "{}. {} {}",
                        i + 1,
                        item.status,
                        html_escape::encode_text(&item.description)
                    ));
                }
                lines.push(String::new()); // Empty line separator
            }
        }

        // Tail-truncation: show only the last MAX_VISIBLE_STEPS steps
        let total_steps = self.steps.len();
        let skip_count = total_steps.saturating_sub(MAX_VISIBLE_STEPS);

        if skip_count > 0 {
            lines.push(format!("... <i>(скрыто {skip_count} шагов)</i> ..."));
        }

        for step in self.steps.iter().skip(skip_count) {
            let icon = match step.status {
                StepStatus::Pending => "⬜",
                StepStatus::InProgress => "⏳",
                StepStatus::Completed => "✅",
                StepStatus::Failed => "❌",
            };

            let tokens_str = step.tokens.map_or_else(String::new, |t| {
                format!(" [<b>{}</b>]", crate::utils::format_tokens(t))
            });

            lines.push(format!(
                "{icon} {}{}",
                html_escape::encode_text(&step.description),
                tokens_str
            ));
        }

        if self.is_finished {
            lines.push("\n✅ <b>Задача завершена</b>".to_string());
        } else if let Some(ref e) = self.error {
            lines.push(format!(
                "\n❌ <b>Ошибка:</b> {}",
                html_escape::encode_text(e)
            ));
        } else {
            lines.push("\n<i>Агент подбирает инструменты...</i>".to_string());
        }

        lines.join("\n")
    }
}
