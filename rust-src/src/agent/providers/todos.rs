//! Todos Provider - manages agent task lists
//!
//! Provides `write_todos` tool for creating and managing task lists,
//! enabling proactive agent behavior for complex multi-step requests.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};

/// Status of a todo item
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task is waiting to be started
    #[default]
    Pending,
    /// Task is currently being worked on
    InProgress,
    /// Task has been completed successfully
    Completed,
    /// Task has been cancelled
    Cancelled,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "⏳"),
            Self::InProgress => write!(f, "🔄"),
            Self::Completed => write!(f, "✅"),
            Self::Cancelled => write!(f, "❌"),
        }
    }
}

/// A single todo item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Description of the task
    pub description: String,
    /// Current status of the task
    pub status: TodoStatus,
}

impl TodoItem {
    /// Create a new pending todo item
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            status: TodoStatus::Pending,
        }
    }

    /// Check if this item is completed or cancelled
    #[must_use]
    pub const fn is_done(&self) -> bool {
        matches!(self.status, TodoStatus::Completed | TodoStatus::Cancelled)
    }
}

/// List of todos for the agent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TodoList {
    /// All todo items
    pub items: Vec<TodoItem>,
    /// When the list was last updated
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl TodoList {
    /// Create a new empty todo list
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if all todos are completed or cancelled
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(TodoItem::is_done)
    }

    /// Get the current in-progress task
    #[must_use]
    pub fn current_task(&self) -> Option<&TodoItem> {
        self.items
            .iter()
            .find(|item| item.status == TodoStatus::InProgress)
    }

    /// Count pending and in-progress items
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.items.iter().filter(|item| !item.is_done()).count()
    }

    /// Count completed items
    #[must_use]
    pub fn completed_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == TodoStatus::Completed)
            .count()
    }

    /// Format todos as a context string for injection into prompts
    #[must_use]
    pub fn to_context_string(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }

        let mut lines = vec!["## Текущий список задач:".to_string()];

        for (i, item) in self.items.iter().enumerate() {
            lines.push(format!("{}. {} {}", i + 1, item.status, item.description));
        }

        let completed = self.completed_count();
        let total = self.items.len();
        lines.push(format!("\nПрогресс: {completed}/{total} выполнено"));

        lines.join("\n")
    }

    /// Clear all todos
    pub fn clear(&mut self) {
        self.items.clear();
        self.updated_at = None;
    }

    /// Update the list with new items
    pub fn update(&mut self, items: Vec<TodoItem>) {
        self.items = items;
        self.updated_at = Some(Utc::now());
    }
}

/// Arguments for `write_todos` tool
#[derive(Debug, Deserialize)]
struct WriteTodosArgs {
    todos: Vec<TodoItemArg>,
}

#[derive(Debug, Deserialize)]
struct TodoItemArg {
    description: String,
    status: TodoStatus,
}

/// Provider for managing todo lists
pub struct TodosProvider {
    /// Shared todo list state
    todos: Arc<Mutex<TodoList>>,
}

impl TodosProvider {
    /// Create a new todos provider with shared state
    pub const fn new(todos: Arc<Mutex<TodoList>>) -> Self {
        Self { todos }
    }

    /// Get a clone of the current todo list
    pub async fn get_todos(&self) -> TodoList {
        self.todos.lock().await.clone()
    }
}

#[async_trait]
impl ToolProvider for TodosProvider {
    fn name(&self) -> &'static str {
        "todos"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "write_todos".to_string(),
            description: "Создать или обновить список задач для текущего запроса. \
                ОБЯЗАТЕЛЬНО используй для сложных запросов, требующих нескольких шагов \
                (исследование, сравнение, анализ). Создай план ПЕРЕД началом работы. \
                НЕ ДАВАЙ финальный ответ, пока все задачи не выполнены."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Полный список задач (заменяет предыдущий список)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": {
                                    "type": "string",
                                    "description": "Описание задачи"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                                    "description": "Статус задачи. Только ОДНА задача может быть in_progress."
                                }
                            },
                            "required": ["description", "status"]
                        }
                    }
                },
                "required": ["todos"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == "write_todos"
    }

    async fn execute(&self, tool_name: &str, arguments: &str) -> Result<String> {
        debug!(tool = tool_name, "Executing todos tool");

        if tool_name != "write_todos" {
            anyhow::bail!("Unknown todos tool: {tool_name}");
        }

        let args: WriteTodosArgs = serde_json::from_str(arguments)?;

        // Convert to TodoItems
        let items: Vec<TodoItem> = args
            .todos
            .into_iter()
            .map(|arg| TodoItem {
                description: arg.description,
                status: arg.status,
            })
            .collect();

        // Update the shared todo list and get state for response
        let (completed, total, current, is_all_complete) = {
            let mut todos = self.todos.lock().await;
            todos.update(items);
            let completed = todos.completed_count();
            let total = todos.items.len();
            let current = todos.current_task().map(|t| t.description.clone());
            let is_all_complete = todos.is_complete();
            drop(todos);
            (completed, total, current, is_all_complete)
        };

        info!(
            completed = completed,
            total = total,
            current = ?current,
            "Todos updated"
        );

        let response = current.map_or_else(
            || {
                if is_all_complete {
                    format!("✅ Все задачи выполнены! ({completed}/{total})")
                } else {
                    format!("✅ Список задач обновлён ({completed}/{total} выполнено)")
                }
            },
            |current_task| {
                format!(
                    "✅ Список задач обновлён ({completed}/{total} выполнено)\n🔄 Текущая задача: {current_task}"
                )
            },
        );

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_todo_status_display() {
        assert_eq!(format!("{}", TodoStatus::Pending), "⏳");
        assert_eq!(format!("{}", TodoStatus::InProgress), "🔄");
        assert_eq!(format!("{}", TodoStatus::Completed), "✅");
        assert_eq!(format!("{}", TodoStatus::Cancelled), "❌");
    }

    #[test]
    fn test_todo_item_is_done() {
        let pending = TodoItem::new("test");
        assert!(!pending.is_done());

        let mut completed = TodoItem::new("test");
        completed.status = TodoStatus::Completed;
        assert!(completed.is_done());

        let mut cancelled = TodoItem::new("test");
        cancelled.status = TodoStatus::Cancelled;
        assert!(cancelled.is_done());
    }

    #[test]
    fn test_todo_list_is_complete() {
        let empty = TodoList::new();
        assert!(!empty.is_complete()); // Empty list is not complete

        let mut list = TodoList::new();
        list.items.push(TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Completed,
        });
        list.items.push(TodoItem {
            description: "Task 2".to_string(),
            status: TodoStatus::Completed,
        });
        assert!(list.is_complete());

        list.items.push(TodoItem {
            description: "Task 3".to_string(),
            status: TodoStatus::Pending,
        });
        assert!(!list.is_complete());
    }

    #[test]
    fn test_todo_list_pending_count() {
        let mut list = TodoList::new();
        list.items.push(TodoItem {
            description: "Done".to_string(),
            status: TodoStatus::Completed,
        });
        list.items.push(TodoItem {
            description: "Pending".to_string(),
            status: TodoStatus::Pending,
        });
        list.items.push(TodoItem {
            description: "In Progress".to_string(),
            status: TodoStatus::InProgress,
        });

        assert_eq!(list.pending_count(), 2);
        assert_eq!(list.completed_count(), 1);
    }

    #[test]
    fn test_todo_list_to_context_string() {
        let mut list = TodoList::new();
        list.items.push(TodoItem {
            description: "Поиск информации".to_string(),
            status: TodoStatus::Completed,
        });
        list.items.push(TodoItem {
            description: "Анализ данных".to_string(),
            status: TodoStatus::InProgress,
        });

        let context = list.to_context_string();
        assert!(context.contains("Текущий список задач"));
        assert!(context.contains("✅ Поиск информации"));
        assert!(context.contains("🔄 Анализ данных"));
        assert!(context.contains("1/2 выполнено"));
    }

    #[tokio::test]
    async fn test_todos_provider_execute() {
        let todos = Arc::new(Mutex::new(TodoList::new()));
        let provider = TodosProvider::new(todos.clone());

        let args = r#"{
            "todos": [
                {"description": "Task 1", "status": "completed"},
                {"description": "Task 2", "status": "in_progress"},
                {"description": "Task 3", "status": "pending"}
            ]
        }"#;

        let result = provider
            .execute("write_todos", args)
            .await
            .expect("Failed to execute todos tool");
        assert!(result.contains("Список задач обновлён"));
        assert!(result.contains("1/3 выполнено"));
        assert!(result.contains("Task 2"));

        let list = todos.lock().await;
        assert_eq!(list.items.len(), 3);
        assert_eq!(list.pending_count(), 2);
        drop(list);
    }
}
