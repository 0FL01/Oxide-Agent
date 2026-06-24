use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiLifeSubmitRequest {
    pub content: String,
    #[serde(default = "empty_array")]
    pub attachments: Value,
    #[serde(default = "empty_object")]
    pub metadata: Value,
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

fn empty_array() -> Value {
    Value::Array(Vec::new())
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

#[cfg(test)]
mod tests {
    use super::ApiLifeSubmitRequest;

    #[test]
    fn life_submit_request_defaults_optional_json_fields() {
        let request: ApiLifeSubmitRequest = serde_json::from_value(serde_json::json!({
            "content": "remember this"
        }))
        .expect("life submit request should deserialize with defaults");

        assert_eq!(request.attachments, serde_json::json!([]));
        assert_eq!(request.metadata, serde_json::json!({}));
    }
}
