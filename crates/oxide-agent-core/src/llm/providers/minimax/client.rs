//! MiniMax provider implementation using claudius SDK

use crate::config::{MINIMAX_CHAT_TEMPERATURE, MINIMAX_TOOL_TEMPERATURE};
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;
use claudius::{Anthropic, MessageCreateParams, Model, ToolChoice};

use super::messages::{new_user_message, to_claudius_messages};
use super::response::from_claudius_message;
use super::tools::to_tool_union_params;

/// MiniMax API base URL (Anthropic-compatible endpoint)
const MINIMAX_ANTHROPIC_URL: &str = "https://api.minimax.io/anthropic";

/// MiniMax provider using claudius SDK for Anthropic-compatible API
pub struct MiniMaxProvider {
    client: Anthropic,
}

impl MiniMaxProvider {
    /// Create a new MiniMax provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        let client = Anthropic::new(Some(api_key))
            .expect("Failed to create MiniMax client")
            .with_base_url(MINIMAX_ANTHROPIC_URL.to_string());
        Self { client }
    }

    /// Build params for a simple chat completion (no tools)
    fn build_chat_params(
        system_prompt: &str,
        messages: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<MessageCreateParams, LlmError> {
        // Build claudius messages: system + history + user message
        let mut claudius_messages = Vec::new();

        // Add history messages
        claudius_messages.extend(to_claudius_messages(messages));

        // Add the new user message
        claudius_messages.push(new_user_message(user_message));

        let model = Model::Custom(model_id.to_string());

        MessageCreateParams::new(max_tokens, claudius_messages, model)
            .with_system_string(system_prompt.to_string())
            .with_temperature(MINIMAX_CHAT_TEMPERATURE)
            .map_err(|e| LlmError::ApiError(format!("Invalid temperature: {}", e)))
    }

    /// Build params for a tool-enabled chat completion
    fn build_tool_params(
        system_prompt: &str,
        messages: &[Message],
        tools: &[crate::llm::ToolDefinition],
        model_id: &str,
        max_tokens: u32,
        temperature: Option<f32>,
    ) -> Result<MessageCreateParams, LlmError> {
        // Build claudius messages: system + history (including tool results)
        let claudius_messages = to_claudius_messages(messages);

        let model = Model::Custom(model_id.to_string());
        let tool_params = to_tool_union_params(tools);

        let params = MessageCreateParams::new(max_tokens, claudius_messages, model)
            .with_system_string(system_prompt.to_string())
            .with_temperature(temperature.unwrap_or(MINIMAX_TOOL_TEMPERATURE))
            .map_err(|e| LlmError::ApiError(format!("Invalid temperature: {}", e)))?
            .with_tools(tool_params);

        Ok(params.with_tool_choice(ToolChoice::auto()))
    }

    /// Send a request and handle errors
    async fn send_request(&self, params: MessageCreateParams) -> Result<ChatResponse, LlmError> {
        self.client
            .send(params)
            .await
            .map_err(|e| map_claudius_error(&e))
            .and_then(from_claudius_message)
    }
}

/// Map claudius errors to our LlmError type
fn map_claudius_error(e: &claudius::Error) -> LlmError {
    use claudius::Error;

    // Log error details at appropriate levels
    if e.is_retryable() {
        tracing::warn!(
            error = %e,
            request_id = ?e.request_id(),
            status_code = ?e.status_code(),
            is_retryable = true,
            "MiniMax retryable error"
        );
    } else if e.is_authentication() || e.is_permission() {
        tracing::error!(
            error = %e,
            "MiniMax auth/permission error"
        );
    } else {
        tracing::warn!(
            error = %e,
            "MiniMax error"
        );
    }

    match e {
        Error::Api {
            status_code: _,
            error_type,
            message,
            request_id,
        } => {
            let mut msg = message.clone();
            if let Some(req_id) = request_id {
                msg.push_str(&format!(" (Request ID: {})", req_id));
            }
            if let Some(err_type) = error_type {
                msg = format!("{}: {}", err_type, msg);
            }
            LlmError::ApiError(msg)
        }
        Error::RateLimit {
            message,
            retry_after,
        } => LlmError::RateLimit {
            wait_secs: *retry_after,
            message: message.clone(),
        },
        Error::Authentication { message } => {
            LlmError::ApiError(format!("Authentication failed: {}", message))
        }
        Error::Permission { message } => {
            LlmError::ApiError(format!("Permission denied: {}", message))
        }
        Error::BadRequest { message, .. } => {
            LlmError::ApiError(format!("Bad request: {}", message))
        }
        Error::Timeout { .. } => LlmError::NetworkError("Request timed out".to_string()),
        Error::Connection { .. } => LlmError::NetworkError("Connection error".to_string()),
        Error::ServiceUnavailable { .. } | Error::InternalServer { .. } => {
            LlmError::ApiError(e.to_string())
        }
        other => LlmError::Unknown(other.to_string()),
    }
}

#[async_trait]
impl LlmProvider for MiniMaxProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let params =
            Self::build_chat_params(system_prompt, history, user_message, model_id, max_tokens)?;

        let response = self.send_request(params).await?;

        response
            .content
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for MiniMax".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for MiniMax".to_string()))
    }

    /// Chat completion with tool calling support for agent mode
    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages: history,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode: _,
        } = request;

        let params = Self::build_tool_params(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            temperature,
        )?;

        self.send_request(params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolDefinition;
    use serde_json::json;

    #[test]
    fn build_chat_params_creates_valid_params() {
        let messages = vec![Message::user("Hello!")];
        let params = MiniMaxProvider::build_chat_params(
            "You are helpful.",
            &messages,
            "How are you?",
            "MiniMax-M2",
            4096,
        )
        .expect("should create params");

        // Verify params were created with correct values
        assert_eq!(params.max_tokens, 4096);
        assert!(params.messages.len() >= 2); // history + new message
    }

    #[test]
    fn build_tool_params_creates_valid_params() {
        let messages = vec![Message::user("Hello!")];
        let tools = vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                }
            }),
        }];

        let params = MiniMaxProvider::build_tool_params(
            "You are helpful.",
            &messages,
            &tools,
            "MiniMax-M2",
            4096,
            None,
        )
        .expect("should create params");

        assert_eq!(params.max_tokens, 4096);
        assert!(params.tools.is_some());
        assert!(params.tool_choice.is_some());
    }

    #[test]
    fn build_tool_params_without_tools_works() {
        let messages = vec![Message::user("Hello!")];

        let params = MiniMaxProvider::build_tool_params(
            "You are helpful.",
            &messages,
            &[],
            "MiniMax-M2",
            4096,
            None,
        )
        .expect("should create params");

        // No tools provided, so tools should be empty
        assert!(params.tools.as_ref().is_none_or(|t| t.is_empty()));
    }

    #[test]
    fn map_claudius_api_error() {
        let error = claudius::Error::Api {
            status_code: 400,
            error_type: Some("invalid_request".to_string()),
            message: "Bad request".to_string(),
            request_id: Some("req_123".to_string()),
        };

        let llm_error = map_claudius_error(&error);

        match llm_error {
            LlmError::ApiError(msg) => {
                assert!(msg.contains("Bad request"));
                assert!(msg.contains("req_123"));
            }
            _ => panic!("Expected ApiError"),
        }
    }

    #[test]
    fn map_claudius_rate_limit_error() {
        let error = claudius::Error::RateLimit {
            message: "Rate limited".to_string(),
            retry_after: Some(60),
        };

        let llm_error = map_claudius_error(&error);

        match llm_error {
            LlmError::RateLimit { wait_secs, message } => {
                assert_eq!(wait_secs, Some(60));
                assert_eq!(message, "Rate limited");
            }
            _ => panic!("Expected RateLimit error"),
        }
    }

    #[test]
    fn map_claudius_timeout_error() {
        let error = claudius::Error::Timeout {
            message: "Request timed out".to_string(),
            duration: None,
        };

        let llm_error = map_claudius_error(&error);

        assert!(matches!(llm_error, LlmError::NetworkError(_)));
    }

    #[test]
    fn map_claudius_authentication_error() {
        let error = claudius::Error::Authentication {
            message: "Invalid API key".to_string(),
        };

        let llm_error = map_claudius_error(&error);

        match llm_error {
            LlmError::ApiError(msg) => {
                assert!(msg.contains("Authentication failed"));
            }
            _ => panic!("Expected ApiError"),
        }
    }
}
