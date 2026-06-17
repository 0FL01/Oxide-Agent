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
    BrowserLive,
    Finished,
    TaskStatus,
    Progress,
    Keepalive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserLiveEventType {
    Session,
    Observation,
    Action,
    Verification,
    Recovery,
    Debug,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserLiveScreenshotRef {
    pub artifact_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default)]
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct BrowserLiveDebugBadges {
    #[serde(default)]
    pub network_failed_count: u32,
    #[serde(default)]
    pub network_request_count: u32,
    #[serde(default)]
    pub console_error_count: u32,
    #[serde(default)]
    pub console_warning_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserLiveEventPayload {
    pub event_type: BrowserLiveEventType,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<BrowserLiveScreenshotRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<BrowserLiveDebugBadges>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<String>>,
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

#[cfg(test)]
mod tests {
    use super::{
        BrowserLiveDebugBadges, BrowserLiveEventPayload, BrowserLiveEventType,
        BrowserLiveScreenshotRef, PersistedTaskEvent, TaskEventKind, UserMessageEventPayload,
    };
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

    #[test]
    fn browser_live_event_payload_serializes_artifact_refs_without_image_bytes() {
        let payload = BrowserLiveEventPayload {
            event_type: BrowserLiveEventType::Observation,
            session_id: "browser-1".to_string(),
            action_seq: Some(3),
            url: Some("https://example.test".to_string()),
            title: Some("Example".to_string()),
            action: Some("click".to_string()),
            confidence: Some(0.91),
            status: Some("action_verified".to_string()),
            blocked_reason: None,
            screenshot: Some(BrowserLiveScreenshotRef {
                artifact_uri: "artifact://browser/task/frame.png".to_string(),
                screenshot_id: Some("shot-3".to_string()),
                mime_type: Some("image/png".to_string()),
                width: Some(1365),
                height: Some(768),
                redacted: false,
            }),
            debug: Some(BrowserLiveDebugBadges {
                network_failed_count: 1,
                network_request_count: 0,
                console_error_count: 2,
                console_warning_count: 3,
            }),
            artifact_refs: Some(vec!["artifact://browser/task/final.png".to_string()]),
        };

        let value = serde_json::to_value(payload).expect("payload serializes");
        assert_eq!(value["event_type"], "observation");
        assert_eq!(
            value["screenshot"]["artifact_uri"],
            "artifact://browser/task/frame.png"
        );
        let serialized = serde_json::to_string(&value).expect("value serializes");
        assert!(!serialized.contains("base64"));
        assert!(!serialized.contains("data:image"));
    }
}
