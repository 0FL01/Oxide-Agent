use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    Thinking,
    ToolCall {
        name: String,
        input: String,
    },
    ToolResult {
        name: String,
        output: String,
    },
    /// Agent is continuing work due to incomplete todos
    Continuation {
        reason: String,
        count: usize,
    },
    /// Todos list was updated
    TodosUpdated {
        current_task: Option<String>,
        completed: usize,
        total: usize,
    },
    Finished,
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    pub current_iteration: usize,
    pub max_iterations: usize,
    pub steps: Vec<Step>,
    pub is_finished: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub description: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ProgressState {
    pub fn new(max_iterations: usize) -> Self {
        Self {
            max_iterations,
            ..Default::default()
        }
    }

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
                    description: format!("Выполнение: {}", name),
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
                        if reason.len() > 50 {
                            &reason[..50]
                        } else {
                            &reason
                        }
                    ),
                    status: StepStatus::InProgress,
                });
            }
            AgentEvent::TodosUpdated {
                current_task,
                completed,
                total,
            } => {
                if let Some(task) = current_task {
                    // Update step description with current task
                    if let Some(last) = self.steps.last_mut() {
                        if last.status == StepStatus::InProgress {
                            last.description = format!("📋 {} ({}/{})", task, completed, total);
                        }
                    }
                }
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

    pub fn format_telegram(&self) -> String {
        let mut lines = Vec::new();
        lines.push("🤖 <b>Работа агента</b>\n".to_string());

        for step in &self.steps {
            let icon = match step.status {
                StepStatus::Pending => "⬜",
                StepStatus::InProgress => "⏳",
                StepStatus::Completed => "✅",
                StepStatus::Failed => "❌",
            };
            lines.push(format!("{} {}", icon, step.description));
        }

        if self.is_finished {
            lines.push("\n✅ <b>Задача завершена</b>".to_string());
        } else if let Some(ref e) = self.error {
            lines.push(format!("\n❌ <b>Ошибка:</b> {}", e));
        } else {
            lines.push("\n<i>Агент подбирает инструменты...</i>".to_string());
        }

        lines.join("\n")
    }
}
