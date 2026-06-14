//! MiniMax provider implementation using reqwest + shared Anthropic Messages helpers.

use crate::config::{MINIMAX_CHAT_TEMPERATURE, MINIMAX_TOOL_TEMPERATURE};
use crate::llm::support::http::send_json_request;
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;

/// MiniMax API base URL (Anthropic-compatible endpoint)
const MINIMAX_ANTHROPIC_URL: &str = "https://api.minimax.io/anthropic";

/// MiniMax provider using reqwest for Anthropic-compatible API
pub struct MiniMaxProvider {
    api_key: String,
    base_url: String,
    http_client: reqwest::Client,
}

impl MiniMaxProvider {
    /// Create a new MiniMax provider instance.
    #[must_use]
    pub fn new(api_key: String, http_client: reqwest::Client) -> Self {
        Self {
            api_key,
            base_url: MINIMAX_ANTHROPIC_URL.to_string(),
            http_client,
        }
    }

    /// Send a request and parse the Anthropic Messages response.
    async fn send_and_parse(&self, body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        let url = format!("{}/v1/messages", self.base_url);
        let extra_headers =
            super::anthropic_messages::request::anthropic_extra_headers(&self.api_key);

        let response = send_json_request(
            &self.http_client,
            &url,
            &body,
            None, // auth is via x-api-key header, not Authorization
            &extra_headers,
        )
        .await?;

        super::anthropic_messages::response::parse_response(
            response,
            super::anthropic_messages::AnthropicProfile::minimax(),
        )
    }
}

#[async_trait]
impl LlmProvider for MiniMaxProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let body = super::anthropic_messages::request::build_completion_body(
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            MINIMAX_CHAT_TEMPERATURE,
            None, // MiniMax does not use extended thinking
        );

        let response = self.send_and_parse(body).await?;

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
            reasoning_effort: _,
        } = request;

        let body = super::anthropic_messages::request::build_messages_body(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            temperature.unwrap_or(MINIMAX_TOOL_TEMPERATURE),
            None, // MiniMax does not use extended thinking
        );

        self.send_and_parse(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolDefinition;
    use serde_json::json;

    #[test]
    fn build_completion_body_creates_valid_body() {
        let messages = vec![Message::user("Hello!")];
        let body = super::super::anthropic_messages::request::build_completion_body(
            "You are helpful.",
            &messages,
            "How are you?",
            "MiniMax-M2",
            4096,
            MINIMAX_CHAT_TEMPERATURE,
            None,
        );

        assert_eq!(body["model"], json!("MiniMax-M2"));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(body["temperature"], json!(MINIMAX_CHAT_TEMPERATURE));
        assert_eq!(body["stream"], json!(false));
        assert!(body["messages"].as_array().unwrap().len() >= 1);
    }

    #[test]
    fn build_messages_body_with_tools_creates_valid_body() {
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

        let body = super::super::anthropic_messages::request::build_messages_body(
            "You are helpful.",
            &messages,
            &tools,
            "MiniMax-M2",
            4096,
            MINIMAX_TOOL_TEMPERATURE,
            None,
        );

        assert_eq!(body["model"], json!("MiniMax-M2"));
        assert_eq!(body["max_tokens"], json!(4096));
        assert!(body.get("tools").is_some());
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn build_messages_body_without_tools_omits_tool_fields() {
        let messages = vec![Message::user("Hello!")];

        let body = super::super::anthropic_messages::request::build_messages_body(
            "You are helpful.",
            &messages,
            &[],
            "MiniMax-M2",
            4096,
            MINIMAX_TOOL_TEMPERATURE,
            None,
        );

        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn parse_response_generates_fallback_id_for_empty_tool_id() {
        let response = super::super::anthropic_messages::response::parse_response(
            json!({
                "content": [
                    {
                        "type": "tool_use",
                        "id": "",
                        "name": "get_weather",
                        "input": {"city": "Moscow"}
                    }
                ],
                "stop_reason": "tool_use"
            }),
            super::super::anthropic_messages::AnthropicProfile::minimax(),
        )
        .expect("response parses");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].wire_tool_call_id(),
            "minimax_fallback_0"
        );
    }

    #[test]
    fn parse_response_parses_text_and_usage() {
        let response = super::super::anthropic_messages::response::parse_response(
            json!({
                "content": [{ "type": "text", "text": "Hello!" }],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            }),
            super::super::anthropic_messages::AnthropicProfile::minimax(),
        )
        .expect("response parses");

        assert_eq!(response.content, Some("Hello!".to_string()));
        assert_eq!(response.finish_reason, "stop");
        let usage = response.usage.expect("usage");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
    }
}
