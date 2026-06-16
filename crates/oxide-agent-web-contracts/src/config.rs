use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::AgentEffort;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PublicConfigResponse {
    pub registration_enabled: bool,
    pub bootstrap_required: bool,
    pub build_version: String,
    pub max_task_input_chars: usize,
    pub large_input_attachments_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModelSelection {
    pub qualified_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserSettingsResponse {
    pub default_model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub default_agent_profile_id: Option<String>,
    #[serde(default)]
    pub default_effort: Option<AgentEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateUserSettingsRequest {
    #[serde(default)]
    pub default_model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub default_agent_profile_id: Option<String>,
    #[serde(default)]
    pub default_effort: Option<AgentEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentProfileView {
    pub agent_id: String,
    pub display_name: String,
    pub system_prompt: String,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListAgentProfilesResponse {
    pub profiles: Vec<AgentProfileView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateAgentProfileRequest {
    pub display_name: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CreateAgentProfileResponse {
    pub profile: AgentProfileView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateAgentProfileRequest {
    pub display_name: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UpdateAgentProfileResponse {
    pub profile: AgentProfileView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRouteProtocolView {
    OpenAiChatCompletions,
    AnthropicMessages,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRouteSourceView {
    Network,
    Cache,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ModelRouteView {
    pub provider_id: String,
    pub model_id: String,
    pub qualified_id: String,
    pub display_name: String,
    pub protocol: ModelRouteProtocolView,
    #[serde(default)]
    pub supports_image_input: bool,
    pub source: ModelRouteSourceView,
    pub fetched_at: DateTime<Utc>,
    pub runnable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListModelRoutesResponse {
    pub provider_id: String,
    pub provider_available: bool,
    pub default_model_id: Option<String>,
    pub routes: Vec<ModelRouteView>,
}

#[cfg(test)]
mod tests {
    use super::{ModelRouteView, ModelSelection, UpdateUserSettingsRequest};

    #[test]
    fn model_selection_uses_qualified_id_contract() {
        let request = UpdateUserSettingsRequest {
            default_model_selection: Some(ModelSelection {
                qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
            }),
            default_agent_profile_id: None,
            default_effort: None,
        };

        let value = serde_json::to_value(request).expect("settings request serializes");

        assert_eq!(
            value["default_model_selection"]["qualified_id"],
            "opencode-zen/deepseek-v4-flash-free"
        );
    }

    #[test]
    fn model_route_view_defaults_image_support_to_false() {
        let route: ModelRouteView = serde_json::from_value(serde_json::json!({
            "provider_id": "opencode-go",
            "model_id": "kimi-k2.6",
            "qualified_id": "opencode-go/kimi-k2.6",
            "display_name": "opencode-go/kimi-k2.6",
            "protocol": "open_ai_chat_completions",
            "source": "network",
            "fetched_at": "2026-06-04T20:00:00Z",
            "runnable": true
        }))
        .expect("old route payload without image support flag should deserialize");

        assert!(!route.supports_image_input);
    }
}
