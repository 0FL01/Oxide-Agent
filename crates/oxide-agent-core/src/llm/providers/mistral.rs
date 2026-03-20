use crate::config::{
    MISTRAL_CHAT_TEMPERATURE, MISTRAL_REASONING_TEMPERATURE, MISTRAL_TOOL_TEMPERATURE,
};
use crate::llm::{
    http_utils::{self, parse_retry_after},
    openai_compat, ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage,
};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};

const MISTRAL_REASONING_MODEL_ID: &str = "mistral-small-2603";
const MISTRAL_REASONING_EFFORT: &str = "high";

/// LLM provider implementation for Mistral AI
pub struct MistralProvider {
    client: Client<OpenAIConfig>,
    http_client: HttpClient,
    api_key: String,
}

impl MistralProvider {
    /// Create a new Mistral provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key.clone())
            .with_api_base("https://api.mistral.ai/v1");
        Self {
            client: Client::with_config(config),
            http_client: http_utils::create_http_client(),
            api_key,
        }
    }

    /// Create a new Mistral provider with a shared HTTP client
    ///
    /// This allows connection reuse across multiple providers,
    /// significantly reducing latency for sequential requests.
    #[must_use]
    pub fn new_with_client(api_key: String, http_client: HttpClient) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key.clone())
            .with_api_base("https://api.mistral.ai/v1");
        Self {
            client: Client::with_config(config),
            http_client,
            api_key,
        }
    }

    fn prepare_structured_messages(
        system_prompt: &str,
        history: &[Message],
    ) -> Vec<serde_json::Value> {
        // Collect all system messages from history to prepend them
        // (Mistral requires system role before any tool/user/assistant after tool)
        let mut history_systems = Vec::new();
        let mut other_messages = Vec::new();

        for msg in history {
            match msg.role.as_str() {
                "system" => {
                    history_systems.push(json!({
                        "role": "system",
                        "content": msg.content
                    }));
                }
                "assistant" => {
                    let content = msg.content.clone();
                    let tool_calls = msg.tool_calls.as_ref();

                    let mut msg_obj = json!({
                        "role": "assistant",
                        "content": content
                    });

                    if let Some(calls) = tool_calls {
                        if !calls.is_empty() {
                            let mistral_tool_calls: Vec<serde_json::Value> = calls
                                .iter()
                                .map(|tc| {
                                    json!({
                                        "id": tc.id,
                                        "type": "function",
                                        "function": {
                                            "name": tc.function.name,
                                            "arguments": tc.function.arguments
                                        }
                                    })
                                })
                                .collect();
                            msg_obj["tool_calls"] = json!(mistral_tool_calls);
                        }
                    }
                    other_messages.push(msg_obj);
                }
                "tool" => {
                    let mut tool_msg = json!({
                        "role": "tool",
                        "content": msg.content
                    });
                    if let Some(tool_call_id) = &msg.tool_call_id {
                        tool_msg["tool_call_id"] = json!(tool_call_id);
                    }
                    if let Some(name) = &msg.name {
                        tool_msg["name"] = json!(name);
                    }
                    other_messages.push(tool_msg);
                }
                _ => {
                    other_messages.push(json!({
                        "role": "user",
                        "content": msg.content
                    }));
                }
            }
        }

        // Build final message list: all systems first, then main system, then others
        let mut messages = Vec::new();
        messages.extend(history_systems);
        messages.push(json!({
            "role": "system",
            "content": system_prompt
        }));
        messages.extend(other_messages);

        messages
    }

    fn prepare_chat_messages(
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
    ) -> Vec<Value> {
        let mut messages = vec![json!({
            "role": "system",
            "content": system_prompt
        })];

        for msg in history {
            match msg.role.as_str() {
                "system" => messages.push(json!({
                    "role": "system",
                    "content": msg.content
                })),
                "assistant" => messages.push(json!({
                    "role": "assistant",
                    "content": msg.content
                })),
                "tool" => messages.push(json!({
                    "role": "user",
                    "content": format!("[Tool Output] {}", msg.content)
                })),
                _ => messages.push(json!({
                    "role": "user",
                    "content": msg.content
                })),
            }
        }

        messages.push(json!({
            "role": "user",
            "content": user_message
        }));

        messages
    }

    fn is_reasoning_model(model_id: &str) -> bool {
        model_id
            .trim()
            .eq_ignore_ascii_case(MISTRAL_REASONING_MODEL_ID)
    }

    fn chat_temperature(model_id: &str) -> f32 {
        if Self::is_reasoning_model(model_id) {
            MISTRAL_REASONING_TEMPERATURE
        } else {
            MISTRAL_CHAT_TEMPERATURE
        }
    }

    fn build_chat_completion_body(
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Value {
        let messages = Self::prepare_chat_messages(system_prompt, history, user_message);
        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": Self::chat_temperature(model_id)
        });

        if Self::is_reasoning_model(model_id) {
            body["reasoning_effort"] = json!(MISTRAL_REASONING_EFFORT);
        }

        body
    }

    fn build_tool_chat_body(
        system_prompt: &str,
        history: &[Message],
        tools: &[crate::llm::ToolDefinition],
        model_id: &str,
        max_tokens: u32,
    ) -> Value {
        let messages = Self::prepare_structured_messages(system_prompt, history);
        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": if Self::is_reasoning_model(model_id) {
                MISTRAL_REASONING_TEMPERATURE
            } else {
                MISTRAL_TOOL_TEMPERATURE
            },
            "tool_choice": "auto",
            "parallel_tool_calls": true
        });

        // Add tools array if provided
        if !tools.is_empty() {
            let mistral_tools: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters
                        }
                    })
                })
                .collect();
            body["tools"] = json!(mistral_tools);
        }

        if Self::is_reasoning_model(model_id) {
            body["reasoning_effort"] = json!(MISTRAL_REASONING_EFFORT);
        }

        body
    }

    fn extract_text_segments(value: &Value) -> Vec<String> {
        match value {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    Vec::new()
                } else {
                    vec![trimmed.to_string()]
                }
            }
            Value::Array(items) => items.iter().flat_map(Self::extract_text_segments).collect(),
            Value::Object(map) => {
                if let Some(text) = map.get("text") {
                    let extracted = Self::extract_text_segments(text);
                    if !extracted.is_empty() {
                        return extracted;
                    }
                }

                ["thinking", "content", "reasoning"]
                    .into_iter()
                    .filter_map(|key| map.get(key))
                    .flat_map(Self::extract_text_segments)
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn join_segments(segments: Vec<String>) -> Option<String> {
        let segments: Vec<_> = segments
            .into_iter()
            .map(|segment| segment.trim().to_string())
            .filter(|segment| !segment.is_empty())
            .collect();

        if segments.is_empty() {
            None
        } else {
            Some(segments.join("\n\n"))
        }
    }

    fn extract_message_content(content: Option<&Value>) -> (Option<String>, Option<String>) {
        let Some(content) = content else {
            return (None, None);
        };

        match content {
            Value::String(text) => (Self::join_segments(vec![text.to_string()]), None),
            Value::Array(items) => {
                let mut content_segments = Vec::new();
                let mut reasoning_segments = Vec::new();

                for item in items {
                    let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                        content_segments.extend(Self::extract_text_segments(item));
                        continue;
                    };

                    match item_type {
                        "thinking" | "reasoning" => {
                            reasoning_segments.extend(Self::extract_text_segments(item));
                        }
                        "text" => {
                            if let Some(text) = item.get("text") {
                                content_segments.extend(Self::extract_text_segments(text));
                            }
                        }
                        _ => content_segments.extend(Self::extract_text_segments(item)),
                    }
                }

                (
                    Self::join_segments(content_segments),
                    Self::join_segments(reasoning_segments),
                )
            }
            _ => (
                Self::join_segments(Self::extract_text_segments(content)),
                None,
            ),
        }
    }

    fn extract_reasoning_content(message: &Value) -> Option<String> {
        Self::join_segments(Self::extract_text_segments(
            message.get("reasoning_content")?,
        ))
    }

    fn parse_usage(response: &Value) -> Option<TokenUsage> {
        let usage = response.get("usage")?;
        Some(TokenUsage {
            prompt_tokens: usage.get("prompt_tokens")?.as_u64()? as u32,
            completion_tokens: usage.get("completion_tokens")?.as_u64()? as u32,
            total_tokens: usage.get("total_tokens")?.as_u64()? as u32,
        })
    }

    fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
        let choice = response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))?;

        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();

        let message = choice
            .get("message")
            .ok_or_else(|| LlmError::ApiError("Response is missing message".to_string()))?;

        let (content, extracted_reasoning) = Self::extract_message_content(message.get("content"));
        let reasoning_content =
            extracted_reasoning.or_else(|| Self::extract_reasoning_content(message));

        // Parse tool_calls from the response
        let tool_calls = Self::parse_tool_calls(message);

        // Allow empty content if there are tool_calls or reasoning_content
        if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
            return Err(LlmError::ApiError("Empty response".to_string()));
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content,
            usage: Self::parse_usage(&response),
        })
    }

    /// Parse tool_calls from Mistral API response message
    fn parse_tool_calls(message: &Value) -> Vec<crate::llm::ToolCall> {
        let Some(tool_calls_array) = message.get("tool_calls") else {
            return Vec::new();
        };

        let Some(array) = tool_calls_array.as_array() else {
            return Vec::new();
        };

        array
            .iter()
            .filter_map(|tc| {
                let id = tc.get("id")?.as_str()?.to_string();
                let function = tc.get("function")?;
                let name = function.get("name")?.as_str()?.to_string();
                let arguments = function
                    .get("arguments")
                    .and_then(|a| {
                        if let Some(s) = a.as_str() {
                            Some(s.to_string())
                        } else {
                            // If arguments is already an object, serialize to string
                            serde_json::to_string(a).ok()
                        }
                    })
                    .unwrap_or_default();

                Some(crate::llm::ToolCall {
                    id,
                    function: crate::llm::ToolCallFunction { name, arguments },
                    is_recovered: false,
                })
            })
            .collect()
    }

    async fn send_chat_request(&self, body: Value) -> Result<ChatResponse, LlmError> {
        let url = "https://api.mistral.ai/v1/chat/completions";

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();

            // Handle 429 Too Many Requests with Retry-After support
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait_secs = parse_retry_after(response.headers());
                let error_text = response.text().await.unwrap_or_default();
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: error_text,
                });
            }

            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "Mistral API error: {status} - {error_text}"
            )));
        }

        let response_json = response
            .json::<Value>()
            .await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        Self::parse_chat_response(response_json)
    }
}

#[async_trait]
impl LlmProvider for MistralProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        if Self::is_reasoning_model(model_id) {
            let body = Self::build_chat_completion_body(
                system_prompt,
                history,
                user_message,
                model_id,
                max_tokens,
            );
            let response = self.send_chat_request(body).await?;
            return response
                .content
                .ok_or_else(|| LlmError::ApiError("Empty response".to_string()));
        }

        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            MISTRAL_CHAT_TEMPERATURE,
        )
        .await
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
    }

    /// Chat completion with tool calling support for agent mode
    ///
    /// # Errors
    ///
    /// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
    /// or `LlmError::JsonError` if parsing fails.
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
            json_mode: _,
        } = request;
        let body = Self::build_tool_chat_body(system_prompt, history, tools, model_id, max_tokens);
        self.send_chat_request(body).await
    }
}

#[cfg(test)]
mod tests {
    use super::MistralProvider;
    use crate::config::{
        MISTRAL_CHAT_TEMPERATURE, MISTRAL_REASONING_TEMPERATURE, MISTRAL_TOOL_TEMPERATURE,
    };
    use crate::llm::Message;
    use serde_json::json;

    #[test]
    fn reasoning_model_chat_body_uses_reasoning_defaults() {
        let body = MistralProvider::build_chat_completion_body(
            "system",
            &[],
            "hello",
            "mistral-small-2603",
            4096,
        );

        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(body["temperature"], json!(MISTRAL_REASONING_TEMPERATURE));
    }

    #[test]
    fn regular_model_tool_body_keeps_existing_temperature() {
        let body =
            MistralProvider::build_tool_chat_body("system", &[], &[], "mistral-large-latest", 4096);

        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["temperature"], json!(MISTRAL_TOOL_TEMPERATURE));
    }

    #[test]
    fn regular_model_chat_body_keeps_existing_temperature() {
        let body = MistralProvider::build_chat_completion_body(
            "system",
            &[],
            "hello",
            "mistral-large-latest",
            4096,
        );

        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["temperature"], json!(MISTRAL_CHAT_TEMPERATURE));
    }

    #[test]
    fn parses_reasoning_chunks_into_content_and_reasoning() {
        let response = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": [
                                {
                                    "type": "text",
                                    "text": "step one"
                                },
                                {
                                    "type": "text",
                                    "text": "step two"
                                }
                            ]
                        },
                        {
                            "type": "text",
                            "text": "final answer"
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        });

        let parsed = MistralProvider::parse_chat_response(response).expect("response parses");

        assert_eq!(parsed.content.as_deref(), Some("final answer"));
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("step one\n\nstep two")
        );
        assert_eq!(parsed.usage.expect("usage").total_tokens, 30);
    }

    #[test]
    fn prepare_structured_messages_formats_tool_message() {
        let history = vec![Message::tool(
            "call_abc123",
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages = MistralProvider::prepare_structured_messages("You are helpful.", &history);

        let tool_msg = &messages[1];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["content"], json!("{\"temperature\": 20}"));
        assert_eq!(tool_msg["tool_call_id"], json!("call_abc123"));
        assert_eq!(tool_msg["name"], json!("get_weather"));
    }

    #[test]
    fn parses_tool_calls_from_response() {
        let response = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"location\":\"Moscow\"}"
                            }
                        },
                        {
                            "id": "call_def456",
                            "type": "function",
                            "function": {
                                "name": "get_time",
                                "arguments": "{}"
                            }
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        });

        let parsed = MistralProvider::parse_chat_response(response).expect("response parses");

        assert!(parsed.content.is_none());
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.tool_calls.len(), 2);

        assert_eq!(parsed.tool_calls[0].id, "call_abc123");
        assert_eq!(parsed.tool_calls[0].function.name, "get_weather");
        assert_eq!(
            parsed.tool_calls[0].function.arguments,
            "{\"location\":\"Moscow\"}"
        );

        assert_eq!(parsed.tool_calls[1].id, "call_def456");
        assert_eq!(parsed.tool_calls[1].function.name, "get_time");
        assert_eq!(parsed.tool_calls[1].function.arguments, "{}");
    }

    #[test]
    fn parses_tool_calls_with_interleaved_content() {
        let response = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "I'll check the weather for you.",
                    "tool_calls": [
                        {
                            "id": "call_xyz789",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"city\":\"London\"}"
                            }
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 15,
                "total_tokens": 35
            }
        });

        let parsed = MistralProvider::parse_chat_response(response).expect("response parses");

        assert_eq!(
            parsed.content.as_deref(),
            Some("I'll check the weather for you.")
        );
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "call_xyz789");
    }

    #[test]
    fn builds_tool_chat_body_with_tools_array() {
        use crate::llm::ToolDefinition;

        let tools = vec![
            ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather for a city".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }),
            },
            ToolDefinition {
                name: "get_time".to_string(),
                description: "Get current time".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ];

        let body = MistralProvider::build_tool_chat_body(
            "You are a helpful assistant.",
            &[],
            &tools,
            "mistral-large-latest",
            4096,
        );

        // Verify tools array is present
        let tools_array = body.get("tools").expect("tools array should be present");
        let tools_vec = tools_array.as_array().expect("tools should be an array");
        assert_eq!(tools_vec.len(), 2);

        // Verify first tool structure
        let first_tool = &tools_vec[0];
        assert_eq!(first_tool["type"], json!("function"));
        assert_eq!(first_tool["function"]["name"], json!("get_weather"));
        assert_eq!(
            first_tool["function"]["description"],
            json!("Get weather for a city")
        );

        // Verify tool_choice and parallel_tool_calls are set
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));

        // Verify response_format is NOT present
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn builds_tool_chat_body_without_tools() {
        let body = MistralProvider::build_tool_chat_body(
            "You are a helpful assistant.",
            &[],
            &[],
            "mistral-large-latest",
            4096,
        );

        // Verify tools array is NOT present when empty
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn prepare_structured_messages_preserves_assistant_tool_calls() {
        use crate::llm::{ToolCall, ToolCallFunction};

        let history = vec![Message::assistant_with_tools(
            "I'll get the weather.",
            vec![ToolCall {
                id: "call_xyz".to_string(),
                function: ToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"Paris\"}".to_string(),
                },
                is_recovered: false,
            }],
        )];
        let messages = MistralProvider::prepare_structured_messages("You are helpful.", &history);

        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], json!("assistant"));
        assert_eq!(assistant_msg["content"], json!("I'll get the weather."));
        assert!(assistant_msg.get("tool_calls").is_some());

        let tool_calls = assistant_msg["tool_calls"].as_array().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], json!("call_xyz"));
        assert_eq!(tool_calls[0]["function"]["name"], json!("get_weather"));
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            json!("{\"city\":\"Paris\"}")
        );
    }
}
