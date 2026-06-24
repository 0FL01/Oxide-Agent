use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeSubmitRequest {
    pub content: String,
    #[serde(default = "empty_array")]
    pub attachments: Value,
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
    pub memory_generation_id: String,
    pub turn_id: String,
    pub input_id: String,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeStateResponse {
    pub principal_user_id: i64,
    pub active_memory_generation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeLinkTokenResponse {
    pub token: String,
    pub target_provider: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeGenerationResponse {
    pub memory_generation_id: String,
    pub generation_number: i64,
    pub status: String,
    pub source_generation_id: Option<String>,
    pub build_reason: String,
    pub build_policy: Value,
    pub source_scope: Value,
    pub comparison_report: Value,
    pub activated_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeGenerationsResponse {
    pub generations: Vec<ApiLifeGenerationResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeSoftResetRequest {
    #[serde(default)]
    pub seed_memory_ids: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiLifeLifecycleResponse {
    pub principal_user_id: i64,
    pub memory_generation_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeProfileResponse {
    pub principal_user_id: i64,
    pub profile_state: Value,
    pub operating_profile: Value,
    pub settings: Value,
    pub schema_version: i32,
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeMemoryItemResponse {
    pub memory_id: String,
    pub memory_generation_id: String,
    pub kind: String,
    pub authority: String,
    pub status: String,
    pub text: String,
    pub structured: Value,
    pub tags: Vec<String>,
    pub evidence_turn_ids: Vec<String>,
    pub sensitivity: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeMemoriesResponse {
    pub active_memory_generation_id: Option<String>,
    pub memories: Vec<ApiLifeMemoryItemResponse>,
    #[serde(default)]
    pub conflicts: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeTaskStateResponse {
    pub task_state_id: String,
    pub project_key: String,
    pub current_goal: String,
    pub why: Option<String>,
    pub current_state: Value,
    pub next_action: Option<String>,
    pub open_loops: Value,
    pub blockers: Value,
    pub status: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeTaskStatesResponse {
    pub task_states: Vec<ApiLifeTaskStateResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeFrictionPatternResponse {
    pub pattern_id: String,
    pub kind: String,
    pub trigger_descriptor: String,
    pub preferred_response: Value,
    pub authority: String,
    pub status: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeFrictionPatternsResponse {
    pub friction_patterns: Vec<ApiLifeFrictionPatternResponse>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeSupportProtocolResponse {
    pub protocol_id: String,
    pub name: String,
    pub trigger_descriptor: String,
    pub steps: Value,
    pub priority: i32,
    pub authority: String,
    pub status: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeSupportProtocolsResponse {
    pub support_protocols: Vec<ApiLifeSupportProtocolResponse>,
}

fn empty_array() -> Value {
    Value::Array(Vec::new())
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[cfg(test)]
mod tests {
    use super::{ApiLifeInputSensitivity, ApiLifeSubmitRequest};

    #[test]
    fn life_submit_request_defaults_optional_json_fields() {
        let request: ApiLifeSubmitRequest = serde_json::from_value(serde_json::json!({
            "content": "remember this"
        }))
        .expect("life submit request should deserialize with defaults");

        assert_eq!(request.attachments, serde_json::json!([]));
        assert_eq!(request.metadata, serde_json::json!({}));
        assert_eq!(request.sensitivity, ApiLifeInputSensitivity::Normal);
    }
}
