use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::TaskAttachment;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeSubmitRequest {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
    #[serde(default)]
    pub sensitivity: ApiLifeInputSensitivity,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApiLifeInputSensitivity {
    #[default]
    Normal,
    Redacted,
    PrivateSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeSubmitResponse {
    pub principal_user_id: i64,
    pub turn_id: String,
    pub input_id: String,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeCancelRunResponse {
    pub run_id: String,
    pub status: String,
}

/// Request body for `POST /api/v1/life/large-input`.
///
/// Stages large text content as a file in the stable life sandbox and returns
/// a `TaskAttachment` that can be included in a subsequent
/// `ApiLifeSubmitRequest.attachments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeLargeInputRequest {
    pub content: String,
    #[serde(default)]
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeStateResponse {
    pub principal_user_id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeTurnResponse {
    pub turn_id: String,
    pub run_id: Option<String>,
    pub role: String,
    pub source_transport: String,
    pub source_ref: Option<String>,
    pub content: String,
    pub attachments: Value,
    pub transport_metadata: Value,
    pub redaction_state: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeTurnsResponse {
    pub turns: Vec<ApiLifeTurnResponse>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeEventResponse {
    pub event_id: String,
    pub run_id: String,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeEventsResponse {
    pub events: Vec<ApiLifeEventResponse>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

// ---------------------------------------------------------------------------
// SSE event payloads
// ---------------------------------------------------------------------------

/// `snapshot` SSE event: initial state sent on connect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeSseSnapshot {
    /// Active run if one is in progress, otherwise `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run: Option<ApiLifeRunSummary>,
}

/// Compact run summary used in SSE `snapshot` and `run_status` events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeRunSummary {
    pub run_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
}

/// `run_status` SSE event: emitted when the active run's status changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeSseRunStatus {
    pub run_id: String,
    pub status: String,
}

/// `keepalive` SSE event: heartbeat carrying current cursor positions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeSseKeepalive {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        ApiLifeEventsResponse, ApiLifeInputSensitivity, ApiLifeSubmitRequest, ApiLifeTurnsResponse,
    };

    #[test]
    fn life_submit_request_defaults_optional_json_fields() {
        let request: ApiLifeSubmitRequest = serde_json::from_value(serde_json::json!({
            "content": "remember this"
        }))
        .expect("life submit request should deserialize with defaults");

        assert!(request.attachments.is_empty());
        assert_eq!(request.metadata, serde_json::json!({}));
        assert_eq!(request.sensitivity, ApiLifeInputSensitivity::Normal);
    }

    #[test]
    fn life_submit_request_deserializes_typed_attachments() {
        let request: ApiLifeSubmitRequest = serde_json::from_value(serde_json::json!({
            "content": "analyze this",
            "attachments": [
                {
                    "file_name": "report.pdf",
                    "mime_type": "application/pdf",
                    "size_bytes": 1024,
                    "sandbox_path": "/workspace/uploads/report.pdf"
                }
            ]
        }))
        .expect("life submit request should deserialize typed attachments");

        assert_eq!(request.attachments.len(), 1);
        assert_eq!(request.attachments[0].file_name, "report.pdf");
        assert_eq!(
            request.attachments[0].sandbox_path,
            "/workspace/uploads/report.pdf"
        );
        assert_eq!(request.attachments[0].size_bytes, 1024);
    }

    #[test]
    fn life_turns_response_supports_paging_cursor() {
        let response: ApiLifeTurnsResponse = serde_json::from_value(serde_json::json!({
            "turns": []
        }))
        .expect("turns response should deserialize without next_cursor");
        assert!(response.next_cursor.is_none());

        let response: ApiLifeTurnsResponse = serde_json::from_value(serde_json::json!({
            "turns": [],
            "next_cursor": "1700000000000:abc"
        }))
        .expect("turns response should deserialize with next_cursor");
        assert_eq!(response.next_cursor.as_deref(), Some("1700000000000:abc"));
    }

    #[test]
    fn life_events_response_supports_paging_cursor() {
        let response: ApiLifeEventsResponse = serde_json::from_value(serde_json::json!({
            "events": []
        }))
        .expect("events response should deserialize without next_cursor");
        assert!(response.next_cursor.is_none());

        let response: ApiLifeEventsResponse = serde_json::from_value(serde_json::json!({
            "events": [],
            "next_cursor": "1700000000000:def:5"
        }))
        .expect("events response should deserialize with next_cursor");
        assert_eq!(response.next_cursor.as_deref(), Some("1700000000000:def:5"));
    }
}
