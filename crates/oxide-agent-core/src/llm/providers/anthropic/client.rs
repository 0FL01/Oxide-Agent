//! Anthropic Messages API provider implementation.

use crate::config::{ANTHROPIC_CHAT_TEMPERATURE, ANTHROPIC_TOOL_TEMPERATURE};
use crate::llm::providers::messages;
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;

/// Generic Anthropic Messages API provider.
pub struct AnthropicProvider {
    client: messages::MessagesClient,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider instance.
    #[must_use]
    pub fn new(api_key: String, http_client: reqwest::Client, api_base: String) -> Self {
        Self {
            client: messages::MessagesClient::from_base_url(
                http_client,
                &api_base,
                api_key,
                messages::MessagesProfile::anthropic(),
            ),
        }
    }

    /// Send a request and parse the Anthropic Messages response.
    async fn send_and_parse(&self, body: serde_json::Value) -> Result<ChatResponse, LlmError> {
        self.client.send_and_parse(&body).await
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let body = messages::request::build_completion_body(
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            ANTHROPIC_CHAT_TEMPERATURE,
            None,
        );

        let response = self.send_and_parse(body).await?;

        response
            .content
            .ok_or_else(|| LlmError::api_error("Empty response"))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Not implemented for Anthropic provider".to_string(),
        ))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Not implemented for Anthropic provider".to_string(),
        ))
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

        let body = messages::request::build_messages_body(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            temperature.unwrap_or(ANTHROPIC_TOOL_TEMPERATURE),
            None,
        );

        self.send_and_parse(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn run_capture_server(
        body: impl Into<String>,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 8192];
            let bytes_read = socket.read(&mut buffer).await.expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            let _ = sender.send(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{addr}"), receiver)
    }

    fn request_body(request: &str) -> serde_json::Value {
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("request contains body separator");
        serde_json::from_str(body).expect("request body is json")
    }

    #[test]
    fn build_completion_body_creates_valid_body() {
        let messages = vec![Message::user("Hello!")];
        let body = messages::request::build_completion_body(
            "You are helpful.",
            &messages,
            "How are you?",
            "claude-3-5-sonnet",
            4096,
            ANTHROPIC_CHAT_TEMPERATURE,
            None,
        );

        assert_eq!(body["model"], json!("claude-3-5-sonnet"));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(body["temperature"], json!(ANTHROPIC_CHAT_TEMPERATURE));
        assert_eq!(body["stream"], json!(false));
        assert!(
            !body["messages"]
                .as_array()
                .expect("messages array")
                .is_empty()
        );
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

        let body = messages::request::build_messages_body(
            "You are helpful.",
            &messages,
            &tools,
            "claude-3-5-sonnet",
            4096,
            ANTHROPIC_TOOL_TEMPERATURE,
            None,
        );

        assert_eq!(body["model"], json!("claude-3-5-sonnet"));
        assert_eq!(body["max_tokens"], json!(4096));
        assert!(body.get("tools").is_some());
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn build_messages_body_without_tools_omits_tool_fields() {
        let messages = vec![Message::user("Hello!")];

        let body = messages::request::build_messages_body(
            "You are helpful.",
            &messages,
            &[],
            "claude-3-5-sonnet",
            4096,
            ANTHROPIC_TOOL_TEMPERATURE,
            None,
        );

        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn parse_response_generates_fallback_id_for_empty_tool_id() {
        let response = messages::response::parse_response(
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
            messages::MessagesProfile::anthropic(),
        )
        .expect("response parses");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].wire_tool_call_id(),
            "anthropic_fallback_0"
        );
    }

    #[test]
    fn parse_response_parses_text_and_usage() {
        let response = messages::response::parse_response(
            json!({
                "content": [{ "type": "text", "text": "Hello!" }],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            }),
            messages::MessagesProfile::anthropic(),
        )
        .expect("response parses");

        assert_eq!(response.content, Some("Hello!".to_string()));
        assert_eq!(response.finish_reason, "stop");
        let usage = response.usage.expect("usage");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
    }

    #[tokio::test]
    async fn anthropic_provider_uses_messages_headers() {
        let (base_url, request_rx) = run_capture_server(
            r#"{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}"#,
        )
        .await;
        let provider = AnthropicProvider::new("key".to_string(), reqwest::Client::new(), base_url);

        let response = provider
            .complete_internal_text("system", &[], "hello", "claude-3-5-sonnet", 32)
            .await
            .expect("text response succeeds");
        let request = request_rx.await.expect("request captured");
        let lowercase = request.to_ascii_lowercase();

        assert_eq!(response, "ok");
        assert!(request.starts_with("POST /v1/messages HTTP/1.1"));
        assert!(lowercase.contains("anthropic-version: 2023-06-01"));
        assert!(lowercase.contains("x-api-key: key"));
        assert!(!lowercase.contains("authorization:"));
    }

    #[tokio::test]
    async fn anthropic_provider_text_delegates_to_messages() {
        let (base_url, request_rx) = run_capture_server(
            r#"{"content":[{"type":"text","text":"delegated"}],"stop_reason":"end_turn"}"#,
        )
        .await;
        let provider = AnthropicProvider::new("key".to_string(), reqwest::Client::new(), base_url);
        let history = vec![Message::user("old")];

        let response = provider
            .complete_internal_text("system", &history, "new", "claude-3-5-sonnet", 32)
            .await
            .expect("text response succeeds");
        let body = request_body(&request_rx.await.expect("request captured"));

        assert_eq!(response, "delegated");
        assert_eq!(body["system"], json!("system"));
        assert_eq!(body["model"], json!("claude-3-5-sonnet"));
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("old"));
        assert_eq!(body["messages"][1]["content"][0]["text"], json!("new"));
        assert_eq!(body["stream"], json!(false));
    }

    #[tokio::test]
    async fn anthropic_provider_tools_preserve_tool_use_and_tool_result_blocks() {
        let (base_url, request_rx) = run_capture_server(
            r#"{"content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}"#,
        )
        .await;
        let provider = AnthropicProvider::new("key".to_string(), reqwest::Client::new(), base_url);
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![
                    ToolCall::new(
                        "invoke-1",
                        ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                        },
                        false,
                    )
                    .with_correlation(
                        ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("toolu-1"),
                    ),
                ],
            ),
            Message::tool_with_correlation(
                "invoke-1",
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("toolu-1"),
                "read_file",
                "contents",
            ),
        ];
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({"type":"object"}),
        }];

        provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &history,
                tools: &tools,
                model_id: "claude-3-5-sonnet",
                max_tokens: 32,
                temperature: None,
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect("tool response succeeds");
        let body = request_body(&request_rx.await.expect("request captured"));

        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
        assert_eq!(body["messages"][0]["content"][1]["id"], json!("toolu-1"));
        assert_eq!(body["messages"][1]["role"], json!("user"));
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("tool_result")
        );
        assert_eq!(
            body["messages"][1]["content"][0]["tool_use_id"],
            json!("toolu-1")
        );
        assert_eq!(body["tools"][0]["input_schema"], json!({"type":"object"}));
        assert_eq!(body["tool_choice"], json!({"type":"auto"}));
        assert!(body.get("response_format").is_none());
    }
}
