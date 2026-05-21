use crate::config::OPENCODE_GO_CHAT_TEMPERATURE;
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{create_http_client, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};

/// LLM provider implementation for OpenCode Go's OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenCodeGoProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
}

impl OpenCodeGoProvider {
    /// Create a new OpenCode Go provider instance.
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: create_http_client(),
            api_key,
            api_base,
        }
    }

    /// Create a new OpenCode Go provider with a shared HTTP client.
    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        Self {
            http_client,
            api_key,
            api_base,
        }
    }
}

#[async_trait]
impl LlmProvider for OpenCodeGoProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let body =
            build_chat_completion_body(system_prompt, history, user_message, model_id, max_tokens);
        let auth = format!("Bearer {}", self.api_key);
        let response =
            send_json_request(&self.http_client, &self.api_base, &body, Some(&auth), &[]).await?;
        let parsed = parse_chat_response(response)?;

        parsed.content.ok_or_else(|| {
            LlmError::ApiError(
                "OpenCode Go returned no text content for chat_completion".to_string(),
            )
        })
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not supported by OpenCode Go".to_string(),
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
            "Image analysis not supported by OpenCode Go".to_string(),
        ))
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        } = request;
        let body = build_tool_chat_body(
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        );
        let auth = format!("Bearer {}", self.api_key);
        let response =
            send_json_request(&self.http_client, &self.api_base, &body, Some(&auth), &[]).await?;

        parse_chat_response(response)
    }
}

fn normalize_model_id(model_id: &str) -> &str {
    let trimmed = model_id.trim();
    trimmed.strip_prefix("opencode-go/").unwrap_or(trimmed)
}

fn build_chat_completion_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let mut messages = prepare_structured_messages(system_prompt, history);
    messages.push(json!({
        "role": "user",
        "content": user_message,
    }));

    json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": OPENCODE_GO_CHAT_TEMPERATURE,
        "stream": false,
    })
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
) -> Value {
    let messages = prepare_structured_messages(system_prompt, history);
    let openai_tools = prepare_tools_json(tools);
    let has_tools = !openai_tools.is_empty();

    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
        "stream": false,
    });

    if has_tools {
        body["tools"] = json!(openai_tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }

    if should_use_native_json_mode(json_mode, has_tools) {
        body["response_format"] = json!({ "type": "json_object" });
    }

    body
}

fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len() + 1);

    if !system_prompt.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                if !msg.content.trim().is_empty() {
                    messages.push(json!({
                        "role": "system",
                        "content": msg.content,
                    }));
                }
            }
            "assistant" => {
                let mut message = json!({
                    "role": "assistant",
                    "content": msg.content,
                });

                if let Some(tool_calls) = &msg.tool_calls {
                    let encoded_tool_calls: Vec<Value> = tool_calls
                        .iter()
                        .filter_map(|tool_call| {
                            CHAT_LIKE_TOOL_PROFILE
                                .encode_tool_call(tool_call)
                                .and_then(|call| call.into_chat_like())
                                .map(|call| {
                                    json!({
                                        "id": call.id,
                                        "type": "function",
                                        "function": {
                                            "name": call.name,
                                            "arguments": call.arguments,
                                        },
                                    })
                                })
                        })
                        .collect();

                    if !encoded_tool_calls.is_empty() {
                        message["tool_calls"] = json!(encoded_tool_calls);
                    }
                }

                messages.push(message);
            }
            "tool" => {
                if let Some(result) = CHAT_LIKE_TOOL_PROFILE
                    .encode_tool_result(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": result.tool_call_id,
                        "content": result.content,
                    }));
                }
            }
            _ => {
                messages.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
        }
    }

    messages
}

fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            })
        })
        .collect()
}

fn should_use_native_json_mode(json_mode: bool, has_tools: bool) -> bool {
    json_mode && !has_tools
}

fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::ApiError("Missing choices[0] in OpenCode Go response".to_string())
        })?;
    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::ApiError("Missing message in OpenCode Go response".to_string()))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string);
    let reasoning_content = parse_reasoning_content(message);
    let tool_calls = match message.get("tool_calls") {
        Some(value) if value.is_null() => Vec::new(),
        Some(value) if value.is_array() => parse_tool_calls(value)?,
        Some(_) => {
            return Err(LlmError::JsonError(
                "Invalid tool_calls format from OpenCode Go".to_string(),
            ))
        }
        None => Vec::new(),
    };

    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::ApiError("Empty OpenCode Go response".to_string()));
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let usage = response.get("usage").and_then(parse_usage);

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage,
    })
}

fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from OpenCode Go".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for call in array {
        let Some(function) = call.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        let arguments = function
            .get("arguments")
            .map(normalize_tool_arguments)
            .unwrap_or_else(|| "{}".to_string());
        let wire_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());

        tool_calls.push(match wire_id {
            Some(wire_id) => CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
                wire_id,
                None,
                name.to_string(),
                arguments,
            ),
            None => {
                CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name.to_string(), arguments)
            }
        });
    }

    Ok(tool_calls)
}

fn normalize_tool_arguments(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn parse_reasoning_content(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .or_else(|| message.get("reasoning").and_then(Value::as_str))
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string)
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: value.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_chat_completion_body, build_tool_chat_body, normalize_model_id, parse_chat_response,
        parse_tool_calls, prepare_structured_messages, prepare_tools_json,
    };
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition};
    use serde_json::json;

    fn read_file_tool() -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    #[test]
    fn normalizes_opencode_go_prefixed_model_id() {
        assert_eq!(
            normalize_model_id("opencode-go/deepseek-v4-flash"),
            "deepseek-v4-flash"
        );
        assert_eq!(
            normalize_model_id(" deepseek-v4-flash "),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn chat_completion_body_uses_raw_model_id() {
        let body = build_chat_completion_body(
            "system",
            &[Message::user("history")],
            "hello",
            "opencode-go/deepseek-v4-flash",
            32000,
        );

        assert_eq!(body["model"], json!("deepseek-v4-flash"));
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][2]["content"], json!("hello"));
    }

    #[test]
    fn tool_request_body_includes_function_names() {
        let tools = vec![read_file_tool()];
        let body = build_tool_chat_body(
            "system",
            &[],
            &tools,
            "deepseek-v4-flash",
            32000,
            Some(0.2),
            true,
        );

        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn json_mode_without_tools_sets_response_format() {
        let body = build_tool_chat_body("system", &[], &[], "deepseek-v4-flash", 32000, None, true);

        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn structured_history_preserves_wire_tool_ids() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![ToolCall::new(
                    "invoke-opencode-1",
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-opencode-1")
                        .with_provider_tool_call_id("call-opencode-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-opencode-1",
                ToolCallCorrelation::new("invoke-opencode-1")
                    .with_provider_tool_call_id("call-opencode-1"),
                "read_file",
                "contents",
            ),
        ];

        let messages = prepare_structured_messages("system", &history);

        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("call-opencode-1"));
        assert_eq!(messages[2]["tool_call_id"], json!("call-opencode-1"));
    }

    #[test]
    fn parse_tool_calls_preserves_provider_wire_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-opencode-2",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"Cargo.toml\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_ne!(tool_calls[0].invocation_id().as_str(), "call-opencode-2");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call-opencode-2");
    }

    #[test]
    fn parse_tool_calls_accepts_object_arguments() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-opencode-3",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": { "path": "Cargo.toml" }
                }
            }
        ]))
        .expect("tool calls parse");

        assert_eq!(tool_calls[0].function.arguments, r#"{"path":"Cargo.toml"}"#);
    }

    #[test]
    fn parse_chat_response_extracts_content_reasoning_tool_calls_and_usage() {
        let response = parse_chat_response(json!({
            "choices": [{
                "message": {
                    "content": null,
                    "reasoning_content": "internal reasoning",
                    "tool_calls": [{
                        "id": "call-opencode-4",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }))
        .expect("response parses");

        assert_eq!(response.content, None);
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("internal reasoning")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.usage.expect("usage").total_tokens, 15);
    }

    #[test]
    fn prepare_tools_json_uses_nested_function_schema() {
        let tools = prepare_tools_json(&[read_file_tool()]);

        assert_eq!(tools[0]["function"]["name"], json!("read_file"));
        assert!(tools[0].get("name").is_none());
    }
}
