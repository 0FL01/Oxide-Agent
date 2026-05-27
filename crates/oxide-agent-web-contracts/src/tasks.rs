use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    WaitingForUserInput,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskStatus {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::WaitingForUserInput
        )
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ProgressSnapshot {
    pub current_iteration: usize,
    pub max_iterations: usize,
    pub is_finished: bool,
    pub error: Option<String>,
    pub current_thought: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_todos: Option<Value>,
    pub last_compaction_status: Option<String>,
    pub repeated_compaction_warning: Option<String>,
    pub last_history_repair_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_token_snapshot: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PendingUserInputView {
    pub kind: UserInputKind,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserInputKind {
    Text,
    Url,
    File,
    UrlOrFile,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct WebTaskRecord {
    pub schema_version: u32,
    pub task_id: String,
    pub session_id: String,
    pub user_id: i64,
    pub status: TaskStatus,
    pub input_markdown: String,
    pub input_edited_at: Option<DateTime<Utc>>,
    pub final_response_markdown: Option<String>,
    pub error_message: Option<String>,
    pub pending_user_input: Option<PendingUserInputView>,
    pub last_progress: Option<ProgressSnapshot>,
    pub last_event_seq: u64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskSummary {
    pub task_id: String,
    pub status: TaskStatus,
    pub input_markdown: String,
    pub input_edited_at: Option<DateTime<Utc>>,
    pub final_response_markdown: Option<String>,
    pub error_message: Option<String>,
    pub pending_user_input: Option<PendingUserInputView>,
    pub last_event_seq: u64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskDetail {
    pub task_id: String,
    pub session_id: String,
    pub status: TaskStatus,
    pub input_markdown: String,
    pub input_edited_at: Option<DateTime<Utc>>,
    pub final_response_markdown: Option<String>,
    pub error_message: Option<String>,
    pub pending_user_input: Option<PendingUserInputView>,
    pub last_progress: Option<ProgressSnapshot>,
    pub last_event_seq: u64,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListTasksResponse {
    pub tasks: Vec<TaskSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateTaskRequest {
    pub input_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateTaskResponse {
    pub task: TaskSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EditTaskInputRequest {
    pub input_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct EditTaskInputResponse {
    pub task: TaskSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResumeTaskRequest {
    pub input_markdown: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResumeTaskResponse {
    pub task: TaskSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GetTaskResponse {
    pub task: TaskDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CancelTaskResponse {
    pub ok: bool,
    pub status: TaskStatus,
}

#[cfg(test)]
mod tests {
    use super::TaskStatus;

    #[test]
    fn task_status_serialization_matches_api_contract() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::WaitingForUserInput).expect("status serializes"),
            "\"waiting_for_user_input\""
        );
        assert!(TaskStatus::Running.is_active());
        assert!(TaskStatus::Completed.is_terminal());
    }
}
