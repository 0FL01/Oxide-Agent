pub(crate) mod module;
pub(crate) mod profile;
pub(crate) mod tool_ids;

pub(crate) use module::OpenAIBaseProviderModule;
pub(crate) use profile::OpenAICompatibleProfile;
pub(crate) use tool_ids::ToolCallIdMapper;

use std::sync::{Arc, Mutex};

use crate::config::OPENAI_BASE_CHAT_TEMPERATURE;
use crate::llm::providers::openai_base::profile::{MessageLayoutPolicy, ResponseContentPolicy};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{extract_text_content, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, MessageContentPart,
    TokenUsage, ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client as HttpClient;
use serde_json::{Value, json};
use tracing::debug;

/// LLM provider for generic OpenAI-compatible Chat Completions endpoints.
pub struct OpenAIBaseProvider {
    http_client: HttpClient,
    api_key: Option<String>,
    api_base: String,
    profile: OpenAICompatibleProfile,
    tool_id_mapper: Arc<Mutex<ToolCallIdMapper>>,
}

impl OpenAIBaseProvider {
    #[must_use]
    pub fn new(api_key: Option<String>, api_base: String) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            api_base,
            crate::llm::support::http::create_http_client(),
            OpenAICompatibleProfile::generic(),
        )
    }

    #[must_use]
    pub fn new_with_client(
        api_key: Option<String>,
        api_base: String,
        http_client: HttpClient,
    ) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            api_base,
            http_client,
            OpenAICompatibleProfile::generic(),
        )
    }

    #[must_use]
    pub fn new_with_client_and_profile(
        api_key: Option<String>,
        api_base: String,
        http_client: HttpClient,
        profile: OpenAICompatibleProfile,
    ) -> Self {
        Self {
            http_client,
            api_key,
            api_base,
            profile,
            tool_id_mapper: Arc::new(Mutex::new(ToolCallIdMapper::new())),
        }
    }

    fn chat_completions_url(&self) -> String {
        chat_completions_url(&self.api_base)
    }

    fn auth_header(&self) -> Option<String> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| format!("Bearer {key}"))
    }
}

fn chat_completions_url(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

/// Dispatch message preparation based on profile layout policy.
fn dispatch_structured_messages(
    system_prompt: &str,
    history: &[Message],
    profile: &OpenAICompatibleProfile,
    tool_id_mapper: &mut ToolCallIdMapper,
) -> Vec<Value> {
    match profile.message_layout {
        MessageLayoutPolicy::GenericOpenAI => prepare_structured_messages(system_prompt, history),
        MessageLayoutPolicy::MistralStrict => {
            prepare_structured_messages_mistral(system_prompt, history, tool_id_mapper)
        }
    }
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
                if let Some(reasoning_content) = msg
                    .reasoning_content
                    .as_deref()
                    .filter(|reasoning| !reasoning.trim().is_empty())
                {
                    message["reasoning_content"] = json!(reasoning_content);
                }
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
            _ => messages.push(json!({
                "role": "user",
                "content": openai_user_message_content(msg),
            })),
        }
    }

    messages
}

/// Mistral-strict message layout: history system messages are collected and
/// prepended before the main system prompt. Tool-call IDs are mapped through
/// the [`ToolCallIdMapper`] to 9-character alphanumeric format.
fn prepare_structured_messages_mistral(
    system_prompt: &str,
    history: &[Message],
    id_mapper: &mut ToolCallIdMapper,
) -> Vec<Value> {
    let mut history_systems = Vec::new();
    let mut other_messages = Vec::new();

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                history_systems.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            "assistant" => {
                let mut msg_obj = json!({
                    "role": "assistant",
                    "content": msg.content,
                });
                if let Some(tool_calls) = &msg.tool_calls
                    && !tool_calls.is_empty()
                {
                    let mapped_calls: Vec<Value> = tool_calls
                        .iter()
                        .filter_map(|tc| {
                            CHAT_LIKE_TOOL_PROFILE
                                .encode_tool_call(tc)
                                .and_then(|call| call.into_chat_like())
                                .map(|call| {
                                    let mistral_id = id_mapper.mistral_id_for(&call.id);
                                    json!({
                                        "id": mistral_id,
                                        "type": "function",
                                        "function": {
                                            "name": call.name,
                                            "arguments": call.arguments,
                                        }
                                    })
                                })
                        })
                        .collect();
                    msg_obj["tool_calls"] = json!(mapped_calls);
                }
                other_messages.push(msg_obj);
            }
            "tool" => {
                if let Some(result) = CHAT_LIKE_TOOL_PROFILE
                    .encode_tool_result(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    let mistral_id = id_mapper.mistral_id_for(&result.tool_call_id);
                    let mut tool_msg = json!({
                        "role": "tool",
                        "content": result.content,
                    });
                    tool_msg["tool_call_id"] = json!(mistral_id);
                    if let Some(name) = result.name {
                        tool_msg["name"] = json!(name);
                    }
                    other_messages.push(tool_msg);
                }
            }
            _ => {
                other_messages.push(json!({
                    "role": "user",
                    "content": msg.content,
                }));
            }
        }
    }

    let mut messages = Vec::with_capacity(history_systems.len() + 1 + other_messages.len());
    messages.extend(history_systems);
    messages.push(json!({
        "role": "system",
        "content": system_prompt,
    }));
    messages.extend(other_messages);
    messages
}

fn openai_user_message_content(message: &Message) -> Value {
    if message.content_parts.is_empty() {
        return json!(message.content);
    }

    let mut parts = Vec::new();
    if !message.content.is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }

    for part in &message.content_parts {
        match part {
            MessageContentPart::Image { mime_type, bytes } if !bytes.is_empty() => {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": image_data_url_with_mime(bytes, mime_type),
                    },
                }));
            }
            MessageContentPart::Image { .. } => {}
        }
    }

    if parts.is_empty() {
        json!(message.content)
    } else {
        json!(parts)
    }
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
    if json_mode && !has_tools {
        body["response_format"] = json!({ "type": "json_object" });
    }
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
    profile: &OpenAICompatibleProfile,
    tool_id_mapper: &mut ToolCallIdMapper,
) -> Value {
    let openai_tools = prepare_tools_json(tools);
    let has_tools = !openai_tools.is_empty();
    let mut body = json!({
        "model": model_id,
        "messages": dispatch_structured_messages(system_prompt, history, profile, tool_id_mapper),
        "max_tokens": max_tokens,
        "temperature": temperature.unwrap_or(profile.tool_temperature),
        "stream": false,
    });

    if has_tools {
        body["tools"] = json!(openai_tools);
        body["tool_choice"] = json!("auto");
    }

    maybe_apply_json_mode(&mut body, json_mode, has_tools);
    body
}

fn build_image_analysis_body(
    image_bytes: &[u8],
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
) -> Value {
    json!({
        "model": model_id,
        "messages": [
            {"role": "system", "content": system_prompt},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text_prompt},
                    {
                        "type": "image_url",
                        "image_url": {"url": image_data_url(image_bytes)}
                    }
                ]
            }
        ],
        "max_tokens": 4000,
        "temperature": OPENAI_BASE_CHAT_TEMPERATURE,
        "stream": false,
    })
}

fn image_data_url(image_bytes: &[u8]) -> String {
    image_data_url_with_mime(image_bytes, infer_image_mime_type(image_bytes))
}

fn image_data_url_with_mime(image_bytes: &[u8], mime_type: &str) -> String {
    let mime_type = normalized_image_mime_type(mime_type, image_bytes);
    format!("data:{mime_type};base64,{}", BASE64.encode(image_bytes))
}

fn normalized_image_mime_type(mime_type: &str, image_bytes: &[u8]) -> String {
    let trimmed = mime_type.trim();
    if trimmed.starts_with("image/") {
        trimmed.to_string()
    } else {
        infer_image_mime_type(image_bytes).to_string()
    }
}

fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
    if image_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        return "image/png";
    }
    if image_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if image_bytes.starts_with(b"RIFF") && image_bytes.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    "image/jpeg"
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

// ---------------------------------------------------------------------------
// Content array parsing (used by Mistral-style response content policy)
// ---------------------------------------------------------------------------

/// Extract text segments from a JSON value recursively.
///
/// Handles strings, arrays, and objects with `text`/`thinking`/`content`/`reasoning` keys.
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
        Value::Array(items) => items.iter().flat_map(extract_text_segments).collect(),
        Value::Object(map) => {
            if let Some(text) = map.get("text") {
                let extracted = extract_text_segments(text);
                if !extracted.is_empty() {
                    return extracted;
                }
            }

            ["thinking", "content", "reasoning"]
                .into_iter()
                .filter_map(|key| map.get(key))
                .flat_map(extract_text_segments)
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Join non-empty trimmed text segments with double newlines.
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

/// Extract message content and reasoning from response content value.
///
/// Handles both simple string content and array content with
/// `thinking`/`reasoning`/`text` chunks.
fn extract_message_content(content: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(content) = content else {
        return (None, None);
    };

    match content {
        Value::String(text) => (join_segments(vec![text.to_string()]), None),
        Value::Array(items) => {
            let mut content_segments = Vec::new();
            let mut reasoning_segments = Vec::new();

            for item in items {
                let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                    content_segments.extend(extract_text_segments(item));
                    continue;
                };

                match item_type {
                    "thinking" | "reasoning" => {
                        reasoning_segments.extend(extract_text_segments(item));
                    }
                    "text" => {
                        if let Some(text) = item.get("text") {
                            content_segments.extend(extract_text_segments(text));
                        }
                    }
                    _ => content_segments.extend(extract_text_segments(item)),
                }
            }

            (
                join_segments(content_segments),
                join_segments(reasoning_segments),
            )
        }
        _ => (join_segments(extract_text_segments(content)), None),
    }
}

/// Fall back to top-level `reasoning_content` field on the message object.
fn extract_reasoning_content(message: &Value) -> Option<String> {
    join_segments(extract_text_segments(message.get("reasoning_content")?))
}

// ---------------------------------------------------------------------------
// Tool call parsing
// ---------------------------------------------------------------------------

/// Generic tool-call parser (no mapper).
fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from OpenAI-compatible provider".to_string(),
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
                    "OpenAI-compatible provider returned empty tool call ID, generating fallback"
                );
                format!("openai_base_fallback_{index}")
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

/// Mistral-style tool-call parser with bidirectional ID reverse mapping.
///
/// Three cases:
/// - Empty/whitespace ID -> `inbound_uncorrelated_tool_call`
/// - Known mapping in mapper -> `inbound_tool_call` (restores original ID)
/// - Unknown mapping -> `inbound_provider_tool_call` (provider-correlated)
fn parse_tool_calls_with_mapper(message: &Value, id_mapper: &ToolCallIdMapper) -> Vec<ToolCall> {
    let Some(tool_calls_array) = message.get("tool_calls") else {
        return Vec::new();
    };
    let Some(array) = tool_calls_array.as_array() else {
        return Vec::new();
    };

    array
        .iter()
        .filter_map(|tc| {
            let provider_id = tc.get("id")?.as_str()?.to_string();
            let original_id = id_mapper.to_original(&provider_id);
            let has_known_mapping = id_mapper.has_mistral_id(&provider_id);

            let function = tc.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let arguments = function
                .get("arguments")
                .map(normalize_tool_arguments)
                .unwrap_or_else(|| "{}".to_string());

            Some(if provider_id.trim().is_empty() {
                CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name, arguments)
            } else if has_known_mapping {
                CHAT_LIKE_TOOL_PROFILE.inbound_tool_call(
                    original_id,
                    Some(&provider_id),
                    None,
                    name,
                    arguments,
                )
            } else {
                CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
                    &provider_id,
                    None,
                    name,
                    arguments,
                )
            })
        })
        .collect()
}

fn parse_chat_response(
    response: Value,
    profile: &OpenAICompatibleProfile,
    id_mapper: &ToolCallIdMapper,
) -> Result<ChatResponse, LlmError> {
    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::ApiError(
                "Missing choices[0] in OpenAI-compatible provider response".to_string(),
            )
        })?;
    let message = choice.get("message").ok_or_else(|| {
        LlmError::ApiError("Missing message in OpenAI-compatible provider response".to_string())
    })?;

    let (content, reasoning_content, tool_calls) = match profile.response_content {
        ResponseContentPolicy::StringOnly => {
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
                        "Invalid tool_calls format from OpenAI-compatible provider".to_string(),
                    ));
                }
                None => Vec::new(),
            };
            (content, reasoning_content, tool_calls)
        }
        ResponseContentPolicy::StringOrChunkArrayWithReasoning => {
            let (content, extracted_reasoning) = extract_message_content(message.get("content"));
            let reasoning_content =
                extracted_reasoning.or_else(|| extract_reasoning_content(message));
            let tool_calls = parse_tool_calls_with_mapper(message, id_mapper);
            (content, reasoning_content, tool_calls)
        }
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
            cached_tokens: usage
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(Value::as_u64)
                .map(|value| value as u32),
            cache_creation_tokens: None,
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
impl LlmProvider for OpenAIBaseProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let mut messages = {
            let mut mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
            let msgs =
                dispatch_structured_messages(system_prompt, history, &self.profile, &mut mapper);
            drop(mapper);
            msgs
        };
        messages.push(json!({
            "role": "user",
            "content": user_message,
        }));
        let body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": self.profile.chat_temperature,
            "stream": false,
        });
        let auth = self.auth_header();
        let res_json = send_json_request(
            &self.http_client,
            &self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not supported by OpenAI-compatible provider".to_string(),
        ))
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let body = build_image_analysis_body(&image_bytes, text_prompt, system_prompt, model_id);
        let auth = self.auth_header();
        let res_json = send_json_request(
            &self.http_client,
            &self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
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
            temperature,
            json_mode,
            reasoning_effort: _,
        } = request;
        let body = {
            let mut mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
            let body = build_tool_chat_body(
                system_prompt,
                history,
                tools,
                model_id,
                max_tokens,
                temperature,
                json_mode,
                &self.profile,
                &mut mapper,
            );
            drop(mapper);
            body
        };
        let auth = self.auth_header();
        let res_json = send_json_request(
            &self.http_client,
            &self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await?;
        let mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
        parse_chat_response(res_json, &self.profile, &mapper)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAIBaseProvider, OpenAICompatibleProfile, ToolCallIdMapper, build_image_analysis_body,
        build_tool_chat_body, chat_completions_url, infer_image_mime_type,
        normalize_tool_arguments_str, parse_chat_response,
    };
    use crate::llm::{Message, MessageContentPart, ToolCall, ToolCallFunction, ToolDefinition};
    use serde_json::json;

    fn sample_tool() -> ToolDefinition {
        ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }
    }

    fn generic_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::generic()
    }

    fn mistral_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::mistral()
    }

    #[test]
    fn chat_completions_url_accepts_base_or_endpoint() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8080/v1/"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8080/v1/chat/completions"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn auth_header_is_optional() {
        let unauthenticated = OpenAIBaseProvider::new(None, "http://localhost/v1".to_string());
        assert_eq!(unauthenticated.auth_header(), None);

        let authenticated = OpenAIBaseProvider::new(
            Some(" token ".to_string()),
            "http://localhost/v1".to_string(),
        );
        assert_eq!(authenticated.auth_header().as_deref(), Some("Bearer token"));
    }

    #[test]
    fn builds_tool_chat_body_with_tools_and_without_parallel_tool_calls() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "You are helpful.",
            &[],
            &[sample_tool()],
            "local-model",
            4096,
            None,
            true,
            &generic_profile(),
            &mut mapper,
        );

        assert_eq!(body["model"], json!("local-model"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert!(body.get("tools").is_some());
        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn adds_json_mode_only_without_tools() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "local-model",
            1024,
            None,
            true,
            &generic_profile(),
            &mut mapper,
        );

        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn encodes_native_image_parts_in_chat_messages() {
        let user = Message::user("What is this?").with_user_content_parts(vec![
            MessageContentPart::image("image/png", b"png".to_vec()),
        ]);
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[user],
            &[],
            "vision-model",
            1024,
            None,
            false,
            &generic_profile(),
            &mut mapper,
        );

        let content = &body["messages"][1]["content"];
        assert_eq!(content[0]["type"], json!("text"));
        assert_eq!(content[1]["type"], json!("image_url"));
        assert_eq!(
            content[1]["image_url"]["url"],
            json!("data:image/png;base64,cG5n")
        );
    }

    #[test]
    fn builds_image_analysis_body_with_data_url() {
        let body = build_image_analysis_body(b"jpg", "Describe", "System", "vision-model");

        assert_eq!(body["messages"][1]["content"][0]["text"], json!("Describe"));
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["url"],
            json!("data:image/jpeg;base64,anBn")
        );
    }

    #[test]
    fn infers_common_image_mime_types() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
        let jpeg = [0xFF, 0xD8, 0xFF];
        let gif = *b"GIF89a";
        let webp = [b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];

        assert_eq!(infer_image_mime_type(&png), "image/png");
        assert_eq!(infer_image_mime_type(&jpeg), "image/jpeg");
        assert_eq!(infer_image_mime_type(&gif), "image/gif");
        assert_eq!(infer_image_mime_type(&webp), "image/webp");
    }

    #[test]
    fn normalizes_tool_arguments() {
        assert_eq!(normalize_tool_arguments_str(""), "{}");
        assert_eq!(
            normalize_tool_arguments_str(r#"{"city":"Paris"}"#),
            r#"{"city":"Paris"}"#
        );
        assert_eq!(
            normalize_tool_arguments_str(r#""{\"city\":\"Paris\"}""#),
            r#"{"city":"Paris"}"#
        );
    }

    #[test]
    fn parses_tool_calls_and_usage() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": {"city": "Paris"}}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 7}
            }
        });

        let parsed = parse_chat_response(response, &generic_profile(), &ToolCallIdMapper::new())
            .expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_1");
        assert_eq!(parsed.usage.expect("usage").cached_tokens, Some(7));
    }

    // -----------------------------------------------------------------------
    // Mistral message layout policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_prepare_structured_messages_formats_tool_message() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![Message::tool(
            "call_abc123",
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages =
            prepare_structured_messages_mistral("You are helpful.", &history, &mut mapper);

        let tool_msg = &messages[1];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["content"], json!("{\"temperature\": 20}"));
        // "call_abc123" -> filter -> "callabc123" -> last 9 -> "allabc123"
        assert_eq!(tool_msg["tool_call_id"], json!("allabc123"));
        assert_eq!(tool_msg["name"], json!("get_weather"));
    }

    #[test]
    fn mistral_prepare_structured_messages_preserves_assistant_tool_calls() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![Message::assistant_with_tools(
            "I'll get the weather.",
            vec![ToolCall::new(
                "call_xyz".to_string(),
                ToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"Paris\"}".to_string(),
                },
                false,
            )],
        )];
        let messages =
            prepare_structured_messages_mistral("You are helpful.", &history, &mut mapper);

        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], json!("assistant"));
        assert_eq!(assistant_msg["content"], json!("I'll get the weather."));
        let tool_calls = assistant_msg["tool_calls"]
            .as_array()
            .expect("tool_calls should be present");
        assert_eq!(tool_calls.len(), 1);
        // "call_xyz" -> filter -> "callxyz" -> last 9 -> "callxyz" (7 chars)
        assert_eq!(tool_calls[0]["id"], json!("callxyz"));
        assert_eq!(tool_calls[0]["function"]["name"], json!("get_weather"));
    }

    #[test]
    fn mistral_system_messages_collected_before_main_system_prompt() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![
            Message {
                role: "system".to_string(),
                content: "History system instruction".to_string(),
                ..Message::user("")
            },
            Message::user("Hello"),
        ];
        let messages =
            prepare_structured_messages_mistral("Main system prompt", &history, &mut mapper);

        // Order: history system, main system, user
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[0]["content"], json!("History system instruction"));
        assert_eq!(messages[1]["role"], json!("system"));
        assert_eq!(messages[1]["content"], json!("Main system prompt"));
        assert_eq!(messages[2]["role"], json!("user"));
    }

    #[test]
    fn generic_messages_put_main_system_prompt_first() {
        use super::prepare_structured_messages;
        let history = vec![
            Message {
                role: "system".to_string(),
                content: "History system instruction".to_string(),
                ..Message::user("")
            },
            Message::user("Hello"),
        ];
        let messages = prepare_structured_messages("Main system prompt", &history);

        // Order: main system, history system, user
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[0]["content"], json!("Main system prompt"));
        assert_eq!(messages[1]["role"], json!("system"));
        assert_eq!(messages[1]["content"], json!("History system instruction"));
        assert_eq!(messages[2]["role"], json!("user"));
    }

    #[test]
    fn mistral_bidirectional_id_mapping_roundtrip() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let original_id = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";

        // Step 1: Register the original ID
        let mistral_id = mapper.register(original_id.to_string());
        assert_eq!(mistral_id, "43acb9e14");

        // Step 2: Prepare tool result message -- should use mapped ID
        let history = vec![Message::tool(
            original_id,
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages = prepare_structured_messages_mistral("sys", &history, &mut mapper);
        let tool_msg = &messages[1];
        assert_eq!(tool_msg["tool_call_id"], json!(mistral_id));
    }

    // -----------------------------------------------------------------------
    // Mistral response content policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_parse_content_array_with_reasoning_chunks() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": [
                        {"type": "thinking", "content": "Let me think about this"},
                        {"type": "text", "text": "Hello world"}
                    ],
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello world"));
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("Let me think about this")
        );
    }

    #[test]
    fn mistral_parse_top_level_reasoning_content_fallback() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Hello",
                    "reasoning_content": "Internal reasoning",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello"));
        // reasoning_content from string content is None; top-level fallback kicks in
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("Internal reasoning")
        );
    }

    #[test]
    fn mistral_parse_tool_calls_with_known_mapping() {
        let mut mapper = ToolCallIdMapper::new();
        let original_id = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";
        let mistral_id = mapper.register(original_id.to_string());
        assert_eq!(mistral_id, "43acb9e14");

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": mistral_id,
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\": \"Paris\"}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Known mapping: invocation_id restores original ID, wire ID is the mistral ID
        assert_eq!(parsed.tool_calls[0].invocation_id().as_str(), original_id);
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), mistral_id);
    }

    #[test]
    fn mistral_parse_unknown_tool_call_id_becomes_provider_correlated() {
        let mapper = ToolCallIdMapper::new(); // empty mapper = no known mappings

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "D681PevKs",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Unknown mapping: provider-correlated (uses provider ID as-is)
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "D681PevKs");
    }

    #[test]
    fn mistral_parse_empty_tool_call_id_becomes_uncorrelated() {
        let mapper = ToolCallIdMapper::new();

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "  ",
                        "type": "function",
                        "function": {"name": "run_code", "arguments": "{}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Empty ID: uncorrelated -- has no provider_tool_call_id (uses generated UUID)
        assert!(
            parsed.tool_calls[0]
                .tool_call_correlation
                .as_ref()
                .map_or(true, |c| c.provider_tool_call_id.is_none())
        );
        // wire_tool_call_id falls back to invocation_id (generated UUID)
        assert!(
            parsed.tool_calls[0]
                .wire_tool_call_id()
                .starts_with("call_")
        );
    }

    #[test]
    fn mistral_parse_cached_tokens_in_usage() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Hello",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 10,
                "total_tokens": 110,
                "prompt_tokens_details": {"cached_tokens": 42}
            }
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        let usage = parsed.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 10);
        assert_eq!(usage.cached_tokens, Some(42));
    }

    #[test]
    fn generic_parse_preserves_string_only_behavior() {
        // Generic profile (StringOnly) does NOT handle content arrays
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Simple text",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &generic_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Simple text"));
        assert!(parsed.reasoning_content.is_none());
    }
}
