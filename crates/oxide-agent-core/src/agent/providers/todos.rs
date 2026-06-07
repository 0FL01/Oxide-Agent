//! Todos Provider - manages agent task lists
//!
//! Provides `write_todos` tool for creating and managing task lists,
//! enabling proactive agent behavior for complex multi-step requests.

use crate::agent::progress::{AgentEvent, AgentEventSource};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc::Sender};
use tracing::info;

/// Status of a todo item
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task is waiting to be started
    #[default]
    Pending,
    /// Task is currently being worked on
    InProgress,
    /// Task is blocked until the user provides more input
    BlockedOnUser,
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
            Self::BlockedOnUser => write!(f, "⏸️"),
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

    /// Get the current task blocked on user input.
    #[must_use]
    pub fn blocked_task(&self) -> Option<&TodoItem> {
        self.items
            .iter()
            .find(|item| item.status == TodoStatus::BlockedOnUser)
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

    /// Count blocked-on-user items.
    #[must_use]
    pub fn blocked_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.status == TodoStatus::BlockedOnUser)
            .count()
    }

    /// Returns true when every incomplete item is blocked on user input.
    #[must_use]
    pub fn all_incomplete_items_blocked_on_user(&self) -> bool {
        !self.items.is_empty()
            && self.items.iter().any(|item| !item.is_done())
            && self
                .items
                .iter()
                .filter(|item| !item.is_done())
                .all(|item| item.status == TodoStatus::BlockedOnUser)
    }

    /// Format todos as a context string for injection into prompts
    #[must_use]
    pub fn to_context_string(&self) -> String {
        if self.items.is_empty() {
            return String::new();
        }

        let mut lines = vec!["## Current task list:".to_string()];

        for (i, item) in self.items.iter().enumerate() {
            lines.push(format!("{}. {} {}", i + 1, item.status, item.description));
        }

        let completed = self.completed_count();
        let total = self.items.len();
        lines.push(format!("\nProgress: {completed}/{total} completed"));

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

    /// Build typed runtime executors for todo tools.
    #[must_use]
    pub fn tool_runtime_executors(
        self: &Arc<Self>,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        self.tool_specs()
            .into_iter()
            .map(|spec| {
                Arc::new(TodosRuntimeExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    progress_tx: progress_tx.clone(),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_specs(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "write_todos".to_string(),
            description: "Create or update a list of tasks for the current request. \
                ABSOLUTELY use it for complex requests that require multiple steps \
                (research, comparison, analysis). Create a plan BEFORE starting work. \
                DO NOT GIVE a final answer until all tasks are completed."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "Full list of tasks (replaces previous list)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "description": {
                                    "type": "string",
                                    "description": "Task description"
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "blocked_on_user", "completed", "cancelled"],
                                    "description": "Task status. Only ONE task can be in_progress. Use blocked_on_user when waiting for the user before work can continue."
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

    async fn write_todos(
        &self,
        arguments: &str,
        progress_tx: Option<&Sender<AgentEvent>>,
    ) -> Result<String> {
        let args: WriteTodosArgs = serde_json::from_str(arguments)?;

        // Convert to TodoItems with XML sanitization to prevent UI corruption
        // LLM may include XML tags in task descriptions which would break formatting
        let items: Vec<TodoItem> = args
            .todos
            .into_iter()
            .map(|arg| TodoItem {
                description: crate::agent::sanitize_xml_tags(&arg.description),
                status: arg.status,
            })
            .collect();

        // Update the shared todo list and get state for response
        let (snapshot, completed, total, active_task, is_all_complete) = {
            let mut todos = self.todos.lock().await;
            todos.update(items);
            let snapshot = todos.clone();
            let completed = todos.completed_count();
            let total = todos.items.len();
            let active_task = todos
                .current_task()
                .map(|t| (t.description.clone(), false))
                .or_else(|| todos.blocked_task().map(|t| (t.description.clone(), true)));
            let is_all_complete = todos.is_complete();
            drop(todos);
            (snapshot, completed, total, active_task, is_all_complete)
        };

        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::TodosUpdated {
                    source: AgentEventSource::Root,
                    todos: snapshot,
                })
                .await;
        }

        info!(
            completed = completed,
            total = total,
            active_task = ?active_task,
            "Todos updated"
        );

        let response = active_task.map_or_else(
            || {
                if is_all_complete {
                    format!("✅ All tasks completed! ({completed}/{total})")
                } else {
                    format!("✅ Task list updated ({completed}/{total} completed)")
                }
            },
            |(active_task, blocked_on_user)| {
                let prefix = if blocked_on_user {
                    "⏸️ Waiting on user"
                } else {
                    "🔄 Current task"
                };
                format!(
                    "✅ Task list updated ({completed}/{total} completed)\n{prefix}: {active_task}"
                )
            },
        );

        Ok(response)
    }
}

struct TodosRuntimeExecutor {
    provider: Arc<TodosProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<Sender<AgentEvent>>,
}

#[async_trait]
impl ToolExecutor for TodosRuntimeExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .write_todos(&invocation.raw_arguments, self.progress_tx.as_ref())
            .await
            .map(|message| normalizer.success(&invocation, &message, ""))
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use serde_json::json;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    async fn recv_todos_update(
        rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>,
    ) -> Result<TodoList, Box<dyn std::error::Error>> {
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await?
            .ok_or("progress channel closed before todos update")?;
        match event {
            AgentEvent::TodosUpdated { todos, .. } => Ok(todos),
            _ => Err("expected TodosUpdated progress event".into()),
        }
    }

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(42),
            turn_id: TurnId::from("turn-test"),
            batch_id: ToolBatchId::from("batch-test"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-write-todos"),
            tool_call_id: ToolCallId::from("call-write-todos"),
            provider_tool_call_id: None,
            tool_name: ToolName::from("write_todos"),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    fn write_todos_executor(
        provider: &Arc<TodosProvider>,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Arc<dyn ToolExecutor> {
        provider
            .tool_runtime_executors(progress_tx)
            .into_iter()
            .find(|executor| executor.name().as_str() == "write_todos")
            .expect("write_todos runtime executor missing")
    }

    async fn execute_write_todos(
        provider: &Arc<TodosProvider>,
        arguments: &str,
        progress_tx: Option<Sender<AgentEvent>>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let output = write_todos_executor(provider, progress_tx)
            .execute(runtime_invocation(arguments))
            .await?;
        assert!(output.success, "write_todos failed: {output:?}");
        Ok(output.stdout.text.unwrap_or_default())
    }

    #[test]
    fn test_todo_status_display() {
        assert_eq!(format!("{}", TodoStatus::Pending), "⏳");
        assert_eq!(format!("{}", TodoStatus::InProgress), "🔄");
        assert_eq!(format!("{}", TodoStatus::BlockedOnUser), "⏸️");
        assert_eq!(format!("{}", TodoStatus::Completed), "✅");
        assert_eq!(format!("{}", TodoStatus::Cancelled), "❌");
    }

    #[test]
    fn test_todo_item_is_done() {
        let pending = TodoItem::new("test");
        assert!(!pending.is_done());

        let mut blocked = TodoItem::new("test");
        blocked.status = TodoStatus::BlockedOnUser;
        assert!(!blocked.is_done());

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
        list.items.push(TodoItem {
            description: "Blocked".to_string(),
            status: TodoStatus::BlockedOnUser,
        });

        assert_eq!(list.pending_count(), 3);
        assert_eq!(list.completed_count(), 1);
        assert_eq!(list.blocked_count(), 1);
    }

    #[test]
    fn test_todo_list_to_context_string() {
        let mut list = TodoList::new();
        list.items.push(TodoItem {
            description: "Search for information".to_string(),
            status: TodoStatus::Completed,
        });
        list.items.push(TodoItem {
            description: "Analyze data".to_string(),
            status: TodoStatus::InProgress,
        });

        let context = list.to_context_string();
        assert!(context.contains("Current task list"));
        assert!(context.contains("✅ Search for information"));
        assert!(context.contains("🔄 Analyze data"));
        assert!(context.contains("1/2 completed"));
    }

    #[test]
    fn test_all_incomplete_items_blocked_on_user() {
        let mut list = TodoList::new();
        list.items.push(TodoItem {
            description: "Need APK link".to_string(),
            status: TodoStatus::BlockedOnUser,
        });
        list.items.push(TodoItem {
            description: "Previous work done".to_string(),
            status: TodoStatus::Completed,
        });

        assert!(list.all_incomplete_items_blocked_on_user());
    }

    #[tokio::test]
    async fn test_todos_write_reports_blocked_task() -> Result<(), Box<dyn std::error::Error>> {
        let todos = Arc::new(Mutex::new(TodoList::new()));
        let provider = Arc::new(TodosProvider::new(Arc::clone(&todos)));

        let args = r#"{
            "todos": [
                {"description": "Need APK link", "status": "blocked_on_user"}
            ]
        }"#;

        let result = execute_write_todos(&provider, args, None).await?;
        assert!(result.contains("Waiting on user"));
        Ok(())
    }

    #[tokio::test]
    async fn test_todos_write() -> Result<(), Box<dyn std::error::Error>> {
        let todos = Arc::new(Mutex::new(TodoList::new()));
        let provider = Arc::new(TodosProvider::new(Arc::clone(&todos)));

        let args = r#"{
            "todos": [
                {"description": "Task 1", "status": "completed"},
                {"description": "Task 2", "status": "in_progress"},
                {"description": "Task 3", "status": "pending"}
            ]
        }"#;

        let result = execute_write_todos(&provider, args, None).await?;
        assert!(result.contains("Task list updated"));
        assert!(result.contains("1/3 completed"));
        assert!(result.contains("Task 2"));

        let list = todos.lock().await;
        assert_eq!(list.items.len(), 3);
        assert_eq!(list.pending_count(), 2);
        drop(list);
        Ok(())
    }

    #[tokio::test]
    async fn test_todos_write_emits_progress_update() -> Result<(), Box<dyn std::error::Error>> {
        let todos = Arc::new(Mutex::new(TodoList::new()));
        let provider = Arc::new(TodosProvider::new(Arc::clone(&todos)));
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(4);

        let args = r#"{
            "todos": [
                {"description": "Task 1", "status": "completed"},
                {"description": "Task 2", "status": "in_progress"}
            ]
        }"#;

        execute_write_todos(&provider, args, Some(progress_tx)).await?;

        let update = recv_todos_update(&mut progress_rx).await?;
        assert_eq!(update.items.len(), 2);
        assert_eq!(update.completed_count(), 1);
        assert_eq!(update.items[1].status, TodoStatus::InProgress);
        Ok(())
    }

    #[tokio::test]
    async fn test_todos_runtime_executor_emits_progress_update()
    -> Result<(), Box<dyn std::error::Error>> {
        let todos = Arc::new(Mutex::new(TodoList::new()));
        let provider = Arc::new(TodosProvider::new(Arc::clone(&todos)));
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(4);
        let executor = write_todos_executor(&provider, Some(progress_tx));

        let args = r#"{
            "todos": [
                {"description": "Runtime task", "status": "completed"}
            ]
        }"#;

        let output = executor.execute(runtime_invocation(args)).await?;
        assert!(output.success);

        let update = recv_todos_update(&mut progress_rx).await?;
        assert_eq!(update.items.len(), 1);
        assert_eq!(update.completed_count(), 1);
        assert_eq!(update.items[0].description, "Runtime task");
        Ok(())
    }
}
