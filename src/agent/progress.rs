use super::providers::TodoList;
use serde::{Deserialize, Serialize};

/// Events that can occur during agent execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    /// Agent is thinking about the next step
    Thinking,
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
    /// Agent encountered an error
    Error(String),
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
}

/// A single step in the agent's execution process
#[derive(Debug, Clone)]
pub struct Step {
    /// Human-readable description of the step
    pub description: String,
    /// Current status of the step
    pub status: StepStatus,
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
            AgentEvent::Thinking => {
                self.current_iteration += 1;
                if let Some(last) = self.steps.last_mut() {
                    if last.status == StepStatus::InProgress {
                        last.status = StepStatus::Completed;
                    }
                }
                self.steps.push(Step {
                    description: format!(
                        "Анализ задачи (итерация {}/{})",
                        self.current_iteration, self.max_iterations
                    ),
                    status: StepStatus::InProgress,
                });
            }
            AgentEvent::ToolCall { name, .. } => {
                if let Some(last) = self.steps.last_mut() {
                    if last.status == StepStatus::InProgress {
                        last.status = StepStatus::Completed;
                    }
                }
                self.steps.push(Step {
                    description: format!("Выполнение: {name}"),
                    status: StepStatus::InProgress,
                });
            }
            AgentEvent::ToolResult { .. } => {
                if let Some(last) = self.steps.last_mut() {
                    if last.status == StepStatus::InProgress {
                        last.status = StepStatus::Completed;
                    }
                }
            }
            AgentEvent::Continuation { reason, count } => {
                if let Some(last) = self.steps.last_mut() {
                    if last.status == StepStatus::InProgress {
                        last.status = StepStatus::Completed;
                    }
                }
                self.steps.push(Step {
                    description: format!(
                        "🔄 Продолжение ({}/5): {}",
                        count,
                        crate::utils::truncate_str(reason, 50)
                    ),
                    status: StepStatus::InProgress,
                });
            }
            AgentEvent::TodosUpdated { todos } => {
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
            AgentEvent::FileToSend { file_name, .. } => {
                self.steps.push(Step {
                    description: format!("📤 Отправка файла: {file_name}"),
                    status: StepStatus::Completed,
                });
            }
            AgentEvent::Finished => {
                self.is_finished = true;
                for step in &mut self.steps {
                    if step.status == StepStatus::InProgress {
                        step.status = StepStatus::Completed;
                    }
                }
            }
            AgentEvent::Error(e) => {
                self.error = Some(e);
                if let Some(last) = self.steps.last_mut() {
                    if last.status == StepStatus::InProgress {
                        last.status = StepStatus::Failed;
                    }
                }
            }
        }
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

        for step in &self.steps {
            let icon = match step.status {
                StepStatus::Pending => "⬜",
                StepStatus::InProgress => "⏳",
                StepStatus::Completed => "✅",
                StepStatus::Failed => "❌",
            };
            lines.push(format!(
                "{icon} {}",
                html_escape::encode_text(&step.description)
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
