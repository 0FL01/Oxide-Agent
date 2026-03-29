use crate::config::NVIDIA_CHAT_TEMPERATURE;
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{extract_text_content, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};
use tracing::debug;

pub struct NvidiaProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
}

impl NvidiaProvider {
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: crate::llm::support::http::create_http_client(),
            api_key,
            api_base,
        }
    }

    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        Self {
            http_client,
            api_key,
            api_base,
        }
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.api_base.trim_end_matches('/'))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NvidiaModelCapabilities {
    pub supports_tool_calling: bool,
    pub supports_structured_output: bool,
}

const TOOL_CALLING_SUPPORTED_MODELS: &[&str] = &[
    "meta/llama-3.1-8b-base",
    "meta/llama-3.1-8b-instruct",
    "meta/llama-3.1-70b-instruct",
    "meta/llama-3.1-405b-instruct",
    "meta/llama-3.2-1b-instruct",
    "meta/llama-3.2-3b-instruct",
    "meta/llama-3.3-70b-instruct",
    "nvidia/llama3.1-nemotron-nano-4b-v1.1",
    "nvidia/llama-3.1-nemotron-nano-8b-v1",
    "nvidia/llama-3.1-nemotron-ultra-253b-v1",
    "nvidia/llama-3.3-nemotron-super-49b-v1",
    "mistralai/mistral-7b-instruct-v0.3",
    "mistralai/mixtral-8x22b-instruct-v01",
    "kakaocorp/kanana-1.5-8b-instruct-2505",
    "scb10x/llama3.1-typhoon2-8b-instruct",
    "scb10x/llama-3.1-typhoon2-70b-instruct",
];

const STRUCTURED_OUTPUT_UNSUPPORTED_PREFIXES: &[&str] =
    &["deepseek-ai/", "mistralai/mixtral-8x7b-instruct-v01"];

#[must_use]
pub(crate) fn model_capabilities(model_id: &str) -> NvidiaModelCapabilities {
    let model_id = model_id.trim().to_ascii_lowercase();

    let supports_tool_calling = TOOL_CALLING_SUPPORTED_MODELS
        .iter()
        .any(|candidate| model_id == *candidate)
        || model_id.contains("gpt-oss");

    let supports_structured_output = !STRUCTURED_OUTPUT_UNSUPPORTED_PREFIXES
        .iter()
        .any(|prefix| model_id.starts_with(prefix));

    NvidiaModelCapabilities {
        supports_tool_calling,
        supports_structured_output,
    }
}

fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt,
    })];

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                messages.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            "assistant" => {
                let mut message = json!({
                    "role": "assistant",
                    "content": msg.content,
                });

                if let Some(tool_calls) = &msg.tool_calls {
                    let api_tool_calls: Vec<Value> = tool_calls
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
                                        }
                                    })
                                })
                        })
                        .collect();

                    if !api_tool_calls.is_empty() {
                        message["tool_calls"] = json!(api_tool_calls);
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
                }
            })
        })
        .collect()
}

fn maybe_apply_json_mode(body: &mut Value, json_mode: bool, has_tools: bool) {
    if !json_mode || has_tools {
        return;
    }

    body["response_format"] = json!({
        "type": "json_object",
    });
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    json_mode: bool,
) -> Value {
    let messages = prepare_structured_messages(system_prompt, history);
    let nvidia_tools = prepare_tools_json(tools);
    let model_capabilities = model_capabilities(model_id);

    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": NVIDIA_CHAT_TEMPERATURE,
        "stream": false,
    });

    if !nvidia_tools.is_empty() {
        body["tools"] = json!(nvidia_tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(false);
    }

    if model_capabilities.supports_structured_output {
        maybe_apply_json_mode(&mut body, json_mode, !tools.is_empty());
    }

    body
}

fn normalize_tool_arguments(value: &Value) -> String {
    match value {
        Value::String(raw) => normalize_tool_arguments_str(raw),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn normalize_tool_arguments_str(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }

    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return trimmed.to_string();
    };

    match parsed {
        Value::String(inner) => match serde_json::from_str::<Value>(&inner) {
            Ok(inner_parsed) => serde_json::to_string(&inner_parsed).unwrap_or(inner),
            Err(_) => inner,
        },
        other => serde_json::to_string(&other).unwrap_or_else(|_| trimmed.to_string()),
    }
}

fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from NVIDIA NIM".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for (index, call) in array.iter().enumerate() {
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
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                debug!(
                    tool_name = name,
                    tool_index = index,
                    "NVIDIA NIM returned empty tool call ID, generating fallback"
                );
                format!("nim_fallback_{index}")
            });

        tool_calls.push(CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
            &wire_id,
            None,
            name.to_string(),
            arguments,
        ));
    }

    Ok(tool_calls)
}

fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::ApiError("Missing choices[0] in NVIDIA NIM response".to_string())
        })?;

    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::ApiError("Missing message in NVIDIA NIM response".to_string()))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let reasoning_content = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let tool_calls = match message.get("tool_calls") {
        Some(value) if value.is_null() => Vec::new(),
        Some(value) if value.is_array() => parse_tool_calls(value)?,
        Some(_) => {
            return Err(LlmError::JsonError(
                "Invalid tool_calls format from NVIDIA NIM".to_string(),
            ))
        }
        None => Vec::new(),
    };

    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::ApiError("Empty response".to_string()));
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let usage = response.get("usage").and_then(|usage| {
        Some(TokenUsage {
            prompt_tokens: usage.get("prompt_tokens")?.as_u64()? as u32,
            completion_tokens: usage.get("completion_tokens")?.as_u64()? as u32,
            total_tokens: usage.get("total_tokens")?.as_u64()? as u32,
        })
    });

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage,
    })
}

#[async_trait]
impl LlmProvider for NvidiaProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let url = self.chat_completions_url();

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history {
            messages.push(json!({"role": msg.role, "content": msg.content}));
        }
        messages.push(json!({"role": "user", "content": user_message}));

        let body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": NVIDIA_CHAT_TEMPERATURE,
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, &url, &body, Some(&auth), &[]).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not supported by NVIDIA NIM provider".to_string(),
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
            "Image analysis not supported by NVIDIA NIM provider".to_string(),
        ))
    }

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
            json_mode,
        } = request;

        let model_capabilities = model_capabilities(model_id);
        if !model_capabilities.supports_tool_calling {
            return Err(LlmError::ApiError(format!(
                "NVIDIA NIM tool calling is not supported for model `{model_id}`"
            )));
        }

        let url = self.chat_completions_url();
        let body = build_tool_chat_body(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            json_mode,
        );

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, &url, &body, Some(&auth), &[]).await?;
        parse_chat_response(res_json)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_tool_chat_body, model_capabilities, normalize_tool_arguments_str,
        parse_chat_response, parse_tool_calls, NvidiaProvider,
    };
    use crate::llm::{Message, ToolDefinition};
    use serde_json::json;

    fn sample_tool() -> ToolDefinition {
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
        }
    }

    #[test]
    fn trims_trailing_slash_when_building_chat_url() {
        let provider = NvidiaProvider::new(
            "test-key".to_string(),
            "https://integrate.api.nvidia.com/v1/".to_string(),
        );

        assert_eq!(
            provider.chat_completions_url(),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
    }

    #[test]
    fn builds_tool_chat_body_conservatively_for_nim() {
        let body = build_tool_chat_body(
            "You are helpful.",
            &[],
            &[sample_tool()],
            "nvidia/model",
            4096,
            true,
        );

        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(false));
        assert_eq!(body["stream"], json!(false));
        assert!(body.get("tools").is_some());
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn adds_response_format_when_json_mode_without_tools() {
        let body = build_tool_chat_body("You are helpful.", &[], &[], "nvidia/model", 4096, true);

        assert_eq!(body["response_format"]["type"], json!("json_object"));
    }

    #[test]
    fn parses_tool_calls_with_wire_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call_nim_1",
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Paris\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_eq!(tool_calls.len(), 1);
        assert_ne!(tool_calls[0].id, "call_nim_1");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call_nim_1");
        assert_eq!(tool_calls[0].function.arguments, "{\"city\":\"Paris\"}");
    }

    #[test]
    fn parses_tool_calls_with_empty_id_using_fallback() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "",
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "arguments": "{\"city\":\"Paris\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_eq!(tool_calls[0].wire_tool_call_id(), "nim_fallback_0");
    }

    #[test]
    fn normalizes_double_serialized_tool_arguments() {
        let normalized = normalize_tool_arguments_str("\"{\\\"city\\\":\\\"Paris\\\"}\"");
        assert_eq!(normalized, "{\"city\":\"Paris\"}");
    }

    #[test]
    fn parses_chat_response_with_reasoning_and_usage() {
        let response = parse_chat_response(json!({
            "choices": [
                {
                    "finish_reason": "stop",
                    "message": {
                        "content": "{\"thought\":\"done\",\"tool_call\":null,\"final_answer\":\"ok\"}",
                        "reasoning_content": "thinking"
                    }
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }))
        .expect("response parses");

        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.reasoning_content.as_deref(), Some("thinking"));
        assert_eq!(response.usage.expect("usage present").total_tokens, 15);
    }

    #[test]
    fn prepare_body_preserves_tool_history_shape() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tool",
                vec![crate::llm::ToolCall::new(
                    "invoke-1",
                    crate::llm::ToolCallFunction {
                        name: "get_weather".to_string(),
                        arguments: "{\"city\":\"Paris\"}".to_string(),
                    },
                    false,
                )],
            ),
            Message::tool("invoke-1", "get_weather", "sunny"),
        ];
        let body = build_tool_chat_body(
            "You are helpful.",
            &history,
            &[sample_tool()],
            "nvidia/model",
            4096,
            false,
        );

        assert_eq!(body["messages"][1]["role"], json!("assistant"));
        assert!(body["messages"][1].get("tool_calls").is_some());
        assert_eq!(body["messages"][2]["role"], json!("tool"));
        assert_eq!(body["messages"][2]["tool_call_id"], json!("invoke-1"));
    }

    #[test]
    fn model_capabilities_allow_known_tool_models() {
        let capabilities = model_capabilities("meta/llama-3.1-70b-instruct");

        assert!(capabilities.supports_tool_calling);
        assert!(capabilities.supports_structured_output);
    }

    #[test]
    fn model_capabilities_block_known_bad_structured_models() {
        let capabilities = model_capabilities("deepseek-ai/deepseek-r1");

        assert!(!capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
    }
}
