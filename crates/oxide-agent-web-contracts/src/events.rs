use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TaskAttachment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PersistedTaskEvent {
    pub schema_version: u32,
    pub task_id: String,
    pub session_id: String,
    pub user_id: i64,
    pub seq: u64,
    pub created_at: DateTime<Utc>,
    pub kind: TaskEventKind,
    pub summary: String,
    #[serde(default)]
    pub payload: Value,
    pub redacted: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventKind {
    UserMessage,
    Thinking,
    Reasoning,
    TokenSnapshotUpdated,
    ToolCall,
    ToolResult,
    TodosUpdated,
    FileToSend,
    Continuation,
    Cancelling,
    Cancelled,
    Error,
    LoopDetected,
    RuntimeCompactionStarted,
    RuntimeCompactionCompleted,
    RuntimeCompactionFailed,
    RuntimeCompactionSkipped,
    RepeatedCompactionWarning,
    HistoryRepairApplied,
    RateLimitRetrying,
    LlmRetrying,
    ProviderFailoverActivated,
    Milestone,
    Finished,
    TaskStatus,
    Progress,
    Keepalive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UserMessageEventPayload {
    #[serde(default)]
    pub input_markdown: String,
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskEventsResponse {
    pub events: Vec<PersistedTaskEvent>,
    #[serde(default)]
    pub first_seq: u64,
    pub last_seq: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SseConnectionState {
    Connected,
    Disconnected,
    Reconnecting,
    TerminalClosed,
}

#[cfg(test)]
mod tests {
    use super::{PersistedTaskEvent, TaskEventKind, UserMessageEventPayload};
    use crate::TaskAttachment;

    #[test]
    fn persisted_event_serializes_stable_kind_and_seq() {
        let event = PersistedTaskEvent {
            schema_version: 1,
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            user_id: 7,
            seq: 44,
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0)
                .expect("timestamp is valid"),
            kind: TaskEventKind::ToolResult,
            summary: "execute_command".to_string(),
            payload: serde_json::json!({ "success": true }),
            redacted: false,
            truncated: false,
        };

        let value = serde_json::to_value(event).expect("event serializes");
        assert_eq!(value["kind"], "tool_result");
        assert_eq!(value["seq"], 44);
        assert_eq!(value["payload"]["success"], true);
    }

    #[test]
    fn user_message_payload_serializes_attachments() {
        let payload = UserMessageEventPayload {
            input_markdown: "follow-up".to_string(),
            attachments: vec![TaskAttachment {
                file_name: "scope.txt".to_string(),
                mime_type: Some("text/plain".to_string()),
                size_bytes: 42,
                sandbox_path: "/workspace/uploads/scope.txt".to_string(),
            }],
        };

        let value = serde_json::to_value(payload).expect("payload serializes");
        assert_eq!(value["input_markdown"], "follow-up");
        assert_eq!(value["attachments"][0]["file_name"], "scope.txt");
        assert_eq!(
            value["attachments"][0]["sandbox_path"],
            "/workspace/uploads/scope.txt"
        );
    }
}
