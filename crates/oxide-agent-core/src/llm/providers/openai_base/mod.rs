pub(crate) mod module;
pub(crate) mod profile;
pub(crate) mod tool_ids;
pub(crate) mod transcription;

pub(crate) use module::MistralProviderModule;
pub(crate) use module::OpenAIBaseProviderModule;
pub(crate) use profile::OpenAICompatibleProfile;
pub(crate) use tool_ids::ToolCallIdMapper;

use std::sync::{Arc, Mutex};

use crate::config::OPENAI_BASE_CHAT_TEMPERATURE;
use crate::llm::providers::openai_base::profile::{
    JsonModePolicy, MessageLayoutPolicy, ReasoningPolicy, ResponseContentPolicy, StreamPolicy,
    ThinkingPolicy,
};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{
    APP_USER_AGENT, extract_text_content, parse_retry_after, send_json_request,
};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, MessageContentPart,
    TokenUsage, ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use futures_util::StreamExt;
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

    /// Convenience constructor for a Mistral-profiled provider.
    #[must_use]
    pub fn new_mistral(api_key: Option<String>, http_client: HttpClient) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            "https://api.mistral.ai/v1".to_string(),
            http_client,
            OpenAICompatibleProfile::mistral(),
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
    if should_use_native_json_mode(json_mode, has_tools) {
        body["response_format"] = json!({ "type": "json_object" });
    }
}

fn should_use_native_json_mode(json_mode: bool, has_tools: bool) -> bool {
    json_mode && !has_tools
}

fn apply_profile_request_policies(
    body: &mut Value,
    profile: &OpenAICompatibleProfile,
    json_mode: bool,
    has_tools: bool,
) {
    let native_json_mode = should_use_native_json_mode(json_mode, has_tools);

    match profile.thinking {
        ThinkingPolicy::None => {}
        ThinkingPolicy::ZaiEnabledUnlessJsonMode => {
            let thinking_type = if native_json_mode {
                "disabled"
            } else {
                "enabled"
            };
            body["thinking"] = json!({ "type": thinking_type });
        }
    }

    match profile.streaming {
        StreamPolicy::NonStreaming => {}
        StreamPolicy::ZaiUnlessNativeJsonMode => {
            body["stream"] = json!(!native_json_mode);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
    reasoning_effort: Option<&str>,
    profile: &OpenAICompatibleProfile,
    tool_id_mapper: &mut ToolCallIdMapper,
) -> Value {
    let openai_tools = prepare_tools_json(tools);
    let has_tools = !openai_tools.is_empty();

    // Temperature: caller override, or reasoning-model default, or tool default.
    let effective_temperature = if profile.is_reasoning_model(model_id) {
        temperature.unwrap_or(profile.reasoning_temperature)
    } else {
        temperature.unwrap_or(profile.tool_temperature)
    };

    let mut body = json!({
        "model": model_id,
        "messages": dispatch_structured_messages(system_prompt, history, profile, tool_id_mapper),
        "max_tokens": max_tokens,
        "temperature": effective_temperature,
        "stream": false,
    });

    if has_tools {
        body["tools"] = json!(openai_tools);
        body["tool_choice"] = json!("auto");
    }

    // parallel_tool_calls: explicit profile value (e.g. Mistral always sends true).
    if let Some(parallel) = profile.parallel_tool_calls {
        body["parallel_tool_calls"] = json!(parallel);
    }

    // reasoning_effort: only for models matching the profile's reasoning policy.
    if let ReasoningPolicy::Mistral { default_effort, .. } = profile.reasoning
        && profile.is_reasoning_model(model_id)
    {
        let effort = reasoning_effort.unwrap_or(default_effort);
        body["reasoning_effort"] = json!(effort);
    }

    // JSON mode: dispatch on profile policy.
    if matches!(profile.json_mode, JsonModePolicy::Standard) {
        maybe_apply_json_mode(&mut body, json_mode, has_tools);
    }

    apply_profile_request_policies(&mut body, profile, json_mode, has_tools);

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
    let usage = response.get("usage").and_then(parse_token_usage);

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage,
    })
}

fn parse_token_usage(usage: &Value) -> Option<TokenUsage> {
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
}

fn should_stream_chat_response(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

/// Parse ZAI flush time from a rate-limit error message or JSON error body.
///
/// ZAI returns reset time as `next_flush_time` in text such as
/// `Usage limit reached. Your limit will reset at 1710000000`. The timestamp can
/// be Unix seconds, Unix milliseconds, or an RFC3339 datetime string.
#[must_use]
pub fn parse_zai_flush_time(message: &str) -> Option<u64> {
    let message_lower = message.to_ascii_lowercase();

    if let Some(caps) = regex::Regex::new(r"\b(\d{10,13})\b")
        .ok()
        .and_then(|regex| regex.captures(&message_lower))
        && let Some(ts_str) = caps.get(1)
    {
        let ts = ts_str.as_str();
        let ts_value: i64 = ts.parse().ok()?;
        let ts_seconds = if ts.len() > 10 {
            ts_value / 1000
        } else {
            ts_value
        };
        let wait_secs = ts_seconds - chrono::Utc::now().timestamp();
        if wait_secs > 0 {
            return Some(wait_secs as u64);
        }
    }

    if let Some(caps) =
        regex::Regex::new(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)")
            .ok()
            .and_then(|regex| regex.captures(message))
        && let Some(dt_str) = caps.get(1)
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(dt_str.as_str())
    {
        let duration = dt.signed_duration_since(chrono::Utc::now());
        if duration.num_seconds() > 0 {
            return Some(duration.num_seconds() as u64);
        }
    }

    None
}

fn apply_profile_rate_limit_wait(error: LlmError, profile: &OpenAICompatibleProfile) -> LlmError {
    match error {
        LlmError::RateLimit { wait_secs, message } if profile.name == "zai" => {
            LlmError::RateLimit {
                wait_secs: parse_zai_flush_time(&message).or(wait_secs),
                message,
            }
        }
        other => other,
    }
}

fn profile_rate_limit_wait_secs(
    profile: &OpenAICompatibleProfile,
    message: &str,
    fallback: Option<u64>,
) -> Option<u64> {
    if profile.name == "zai" {
        parse_zai_flush_time(message).or(fallback)
    } else {
        fallback
    }
}

#[derive(Default)]
struct StreamingChatAccumulator {
    content: String,
    reasoning_content: String,
    finish_reason: String,
    usage: Option<TokenUsage>,
    pending_tool_calls: Vec<PendingStreamingToolCall>,
}

#[derive(Default)]
struct PendingStreamingToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

async fn send_streaming_chat_request(
    client: &HttpClient,
    url: &str,
    body: &Value,
    auth_header: Option<&str>,
    profile: &OpenAICompatibleProfile,
) -> Result<ChatResponse, LlmError> {
    let mut request = client
        .post(url)
        .json(body)
        .header("User-Agent", APP_USER_AGENT);
    if let Some(auth) = auth_header {
        request = request.header("Authorization", auth);
    }

    let response = request
        .send()
        .await
        .map_err(|error| LlmError::NetworkError(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let retry_after_secs = (status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            .then(|| parse_retry_after(response.headers()))
            .flatten();
        let error_text = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::RateLimit {
                wait_secs: profile_rate_limit_wait_secs(profile, &error_text, retry_after_secs),
                message: error_text,
            });
        }
        return Err(LlmError::ApiError(format!(
            "API error: {status} - {error_text}"
        )));
    }

    parse_streaming_chat_response(response).await
}

async fn parse_streaming_chat_response(
    response: reqwest::Response,
) -> Result<ChatResponse, LlmError> {
    let mut state = StreamingChatAccumulator {
        finish_reason: "unknown".to_string(),
        ..StreamingChatAccumulator::default()
    };
    let mut buffer = String::new();
    let mut pending_bytes = Vec::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| LlmError::NetworkError(error.to_string()))?;
        pending_bytes.extend_from_slice(&chunk);
        if let Some(decoded) = decode_utf8_prefix(&mut pending_bytes)? {
            buffer.push_str(&decoded);
        }
        normalize_newlines_in_place(&mut buffer);

        while let Some(boundary) = buffer.find("\n\n") {
            let raw_event = buffer[..boundary].to_string();
            buffer = buffer[(boundary + 2)..].to_string();
            process_chat_sse_event(&raw_event, &mut state)?;
        }
    }

    if !pending_bytes.is_empty() {
        let tail = String::from_utf8(pending_bytes)
            .map_err(|error| LlmError::JsonError(error.to_string()))?;
        buffer.push_str(&tail);
        normalize_newlines_in_place(&mut buffer);
    }

    if !buffer.trim().is_empty() {
        process_chat_sse_event(&buffer, &mut state)?;
    }

    finish_streaming_chat_response(state)
}

fn process_chat_sse_event(
    raw_event: &str,
    state: &mut StreamingChatAccumulator,
) -> Result<(), LlmError> {
    let payload = raw_event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(());
    }

    let chunk: Value =
        serde_json::from_str(&payload).map_err(|error| LlmError::JsonError(error.to_string()))?;

    if let Some(choice) = chunk.get("choices").and_then(|choices| choices.get(0)) {
        if let Some(delta) = choice.get("delta") {
            if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
                state.reasoning_content.push_str(reasoning);
            }
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                state.content.push_str(text);
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                apply_streaming_tool_call_delta(tool_calls, &mut state.pending_tool_calls);
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            state.finish_reason = reason.to_string();
        }
    }

    if let Some(usage) = chunk.get("usage").and_then(parse_token_usage) {
        state.usage = Some(usage);
    }

    Ok(())
}

fn apply_streaming_tool_call_delta(
    tool_calls: &[Value],
    pending: &mut Vec<PendingStreamingToolCall>,
) {
    for call in tool_calls {
        if let Some(call_type) = call.get("type").and_then(Value::as_str)
            && call_type != "function"
        {
            continue;
        }

        let entry_index = streaming_tool_call_index(call, pending);
        while pending.len() <= entry_index {
            pending.push(PendingStreamingToolCall::default());
        }

        let entry = &mut pending[entry_index];
        if let Some(id) = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            entry.id = Some(id.to_string());
        }

        if let Some(function) = call.get("function") {
            if let Some(name) = function
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
            {
                entry.name = Some(name.to_string());
            }
            if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                entry.arguments.push_str(arguments);
            }
        }
    }
}

fn streaming_tool_call_index(call: &Value, pending: &[PendingStreamingToolCall]) -> usize {
    if let Some(index) = call.get("index").and_then(Value::as_u64) {
        return index as usize;
    }

    if let Some(id) = call.get("id").and_then(Value::as_str) {
        if let Some((index, _)) = pending
            .iter()
            .enumerate()
            .find(|(_, pending_call)| pending_call.id.as_deref() == Some(id))
        {
            return index;
        }

        if pending
            .last()
            .and_then(|pending_call| pending_call.id.as_deref())
            != Some(id)
        {
            return pending.len();
        }
    }

    pending.len().saturating_sub(1)
}

fn finish_streaming_chat_response(
    state: StreamingChatAccumulator,
) -> Result<ChatResponse, LlmError> {
    let tool_calls = finalize_streaming_tool_calls(state.pending_tool_calls);
    if state.content.is_empty() && state.reasoning_content.is_empty() && tool_calls.is_empty() {
        return Err(LlmError::EmptyResponse(
            " from OpenAI-compatible streaming response".to_string(),
        ));
    }

    Ok(ChatResponse {
        content: (!state.content.is_empty()).then_some(state.content),
        tool_calls,
        finish_reason: state.finish_reason,
        reasoning_content: (!state.reasoning_content.is_empty()).then_some(state.reasoning_content),
        usage: state.usage,
    })
}

fn finalize_streaming_tool_calls(pending: Vec<PendingStreamingToolCall>) -> Vec<ToolCall> {
    pending
        .into_iter()
        .filter_map(|call| {
            let name = call.name?;
            let arguments = if call.arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                call.arguments
            };
            Some(match call.id {
                Some(id) if !id.trim().is_empty() => CHAT_LIKE_TOOL_PROFILE
                    .inbound_provider_tool_call(id.as_str(), None, name, arguments),
                _ => CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name, arguments),
            })
        })
        .collect()
}

fn decode_utf8_prefix(pending_bytes: &mut Vec<u8>) -> Result<Option<String>, LlmError> {
    match std::str::from_utf8(pending_bytes) {
        Ok(valid) => {
            let decoded = valid.to_string();
            pending_bytes.clear();
            Ok((!decoded.is_empty()).then_some(decoded))
        }
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            if let Some(error_len) = error.error_len() {
                return Err(LlmError::JsonError(format!(
                    "invalid utf-8 in OpenAI-compatible stream at {valid_up_to} (len {error_len})"
                )));
            }

            if valid_up_to == 0 {
                return Ok(None);
            }

            let decoded = String::from_utf8(pending_bytes[..valid_up_to].to_vec())
                .map_err(|error| LlmError::JsonError(error.to_string()))?;
            pending_bytes.drain(..valid_up_to);
            Ok(Some(decoded))
        }
    }
}

fn normalize_newlines_in_place(buffer: &mut String) {
    if buffer.contains('\r') {
        *buffer = buffer.replace("\r\n", "\n");
    }
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

        // Temperature: reasoning-model default, or chat default.
        let effective_temperature = if self.profile.is_reasoning_model(model_id) {
            self.profile.reasoning_temperature
        } else {
            self.profile.chat_temperature
        };

        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": effective_temperature,
            "stream": false,
        });

        // reasoning_effort: only for models matching the profile's reasoning policy.
        if let ReasoningPolicy::Mistral { default_effort, .. } = self.profile.reasoning
            && self.profile.is_reasoning_model(model_id)
        {
            body["reasoning_effort"] = json!(default_effort);
        }
        let auth = self.auth_header();
        let res_json = send_json_request(
            &self.http_client,
            &self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &self.profile))?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let Some(audio_profile) = self.profile.audio_transcription else {
            return Err(LlmError::Unknown(format!(
                "Audio transcription not supported by {} profile",
                self.profile.name,
            )));
        };

        transcription::transcribe_audio(
            &self.http_client,
            self.api_key.as_deref(),
            &self.api_base,
            audio_bytes,
            mime_type,
            model_id,
            &audio_profile,
            self.profile.name,
        )
        .await
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
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &self.profile))?;
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
            reasoning_effort,
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
                reasoning_effort,
                &self.profile,
                &mut mapper,
            );
            drop(mapper);
            body
        };
        let auth = self.auth_header();
        if should_stream_chat_response(&body) {
            return send_streaming_chat_request(
                &self.http_client,
                &self.chat_completions_url(),
                &body,
                auth.as_deref(),
                &self.profile,
            )
            .await;
        }

        let res_json = send_json_request(
            &self.http_client,
            &self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &self.profile))?;
        let mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
        parse_chat_response(res_json, &self.profile, &mapper)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAIBaseProvider, OpenAICompatibleProfile, StreamingChatAccumulator, ToolCallIdMapper,
        build_image_analysis_body, build_tool_chat_body, chat_completions_url,
        finalize_streaming_tool_calls, finish_streaming_chat_response, infer_image_mime_type,
        normalize_tool_arguments_str, parse_chat_response, parse_zai_flush_time,
        process_chat_sse_event, send_streaming_chat_request,
    };
    use crate::llm::{
        ChatWithToolsRequest, LlmError, LlmProvider, Message, MessageContentPart, ToolCall,
        ToolCallFunction, ToolDefinition,
    };
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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

    fn zai_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::zai()
    }

    async fn run_single_response_server(
        body: impl Into<String>,
        content_type: &'static str,
    ) -> String {
        run_single_status_response_server("200 OK", body, content_type, &[]).await
    }

    async fn run_single_status_response_server(
        status: &'static str,
        body: impl Into<String>,
        content_type: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await.expect("read request");
            let extra_headers = headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        format!("http://{addr}/v1")
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
            None,
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
            None,
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
            None,
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
                .is_none_or(|c| c.provider_tool_call_id.is_none())
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

    // -----------------------------------------------------------------------
    // Checkpoint 5: Request tweaks -- temperatures, parallel_tool_calls,
    // reasoning_effort, JSON mode
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_tool_body_includes_parallel_tool_calls_and_tool_choice() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-large-latest",
            4096,
            None,
            false,
            None,
            &mistral_profile(),
            &mut mapper,
        );

        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(body.get("tools").is_some());
    }

    #[test]
    fn mistral_reasoning_model_tool_body_includes_reasoning_effort() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-small-2603",
            4096,
            None,
            false,
            None,
            &mistral_profile(),
            &mut mapper,
        );

        assert_eq!(body["reasoning_effort"], json!("high"));
        // Reasoning model uses reasoning_temperature (0.7), not tool_temperature
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.7).abs() < 1e-6);
    }

    #[test]
    fn mistral_regular_model_tool_body_uses_tool_temperature() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-large-latest",
            4096,
            None,
            false,
            None,
            &mistral_profile(),
            &mut mapper,
        );

        // Regular model: no reasoning_effort
        assert!(body.get("reasoning_effort").is_none());
        // tool_temperature = 0.7
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.7).abs() < 1e-6);
    }

    #[test]
    fn mistral_tool_body_explicit_temperature_overrides_default() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "mistral-large-latest",
            4096,
            Some(0.23),
            false,
            None,
            &mistral_profile(),
            &mut mapper,
        );

        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.23).abs() < 1e-6);
    }

    #[test]
    fn mistral_reasoning_model_tool_body_explicit_effort_overrides_default() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-small-2603",
            4096,
            None,
            false,
            Some("none"),
            &mistral_profile(),
            &mut mapper,
        );

        assert_eq!(body["reasoning_effort"], json!("none"));
    }

    #[test]
    fn generic_tool_body_no_parallel_or_reasoning() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "some-model",
            4096,
            None,
            false,
            None,
            &generic_profile(),
            &mut mapper,
        );

        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn zai_tool_body_sets_stream_and_enabled_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "glm-4.7",
            4096,
            None,
            false,
            None,
            &zai_profile(),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert!(body.get("response_format").is_none());
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.95).abs() < 1e-6);
    }

    #[test]
    fn zai_plain_body_without_json_streams_with_enabled_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "glm-4.7",
            1024,
            None,
            false,
            None,
            &zai_profile(),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn zai_native_json_body_disables_thinking_and_streaming() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "glm-4.7",
            1024,
            None,
            true,
            None,
            &zai_profile(),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn zai_json_with_tools_does_not_use_native_json_mode() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "glm-4.7",
            1024,
            None,
            true,
            None,
            &zai_profile(),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn generic_tool_body_does_not_send_zai_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "some-model",
            4096,
            None,
            false,
            None,
            &generic_profile(),
            &mut mapper,
        );

        assert!(body.get("thinking").is_none());
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn zai_sse_aggregates_content_reasoning_finish_and_usage() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"content":"hel","reasoning_content":"think "}}]}"#,
            &mut state,
        )
        .expect("first event parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"content":"lo","reasoning_content":"again"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":3}}}"#,
            &mut state,
        )
        .expect("second event parses");
        process_chat_sse_event("data: [DONE]", &mut state).expect("done event ignored");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.content.as_deref(), Some("hello"));
        assert_eq!(response.reasoning_content.as_deref(), Some("think again"));
        assert_eq!(response.finish_reason, "stop");
        let usage = response.usage.expect("usage captured");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 14);
        assert_eq!(usage.cached_tokens, Some(3));
    }

    #[test]
    fn zai_sse_aggregates_fragmented_tool_arguments_and_preserves_id() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-zai-1","type":"function","function":{"name":"search","arguments":"{\"q"}}]}}]}"#,
            &mut state,
        )
        .expect("first tool delta parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":\"oxi"}}]}}]}"#,
            &mut state,
        )
        .expect("second tool delta parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"de\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut state,
        )
        .expect("final tool delta parses");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.tool_calls.len(), 1);
        assert_ne!(
            response.tool_calls[0].invocation_id().as_str(),
            "call-zai-1"
        );
        assert_eq!(response.tool_calls[0].wire_tool_call_id(), "call-zai-1");
        assert_eq!(response.tool_calls[0].function.name, "search");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"q":"oxide"}"#
        );
    }

    #[test]
    fn zai_sse_empty_response_errors_cleanly() {
        let err = finish_streaming_chat_response(StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        })
        .expect_err("empty stream should fail");

        assert!(err.to_string().contains("Empty response"));
    }

    #[test]
    fn streaming_tool_calls_handle_empty_id_as_uncorrelated() {
        let tool_calls = finalize_streaming_tool_calls(vec![super::PendingStreamingToolCall {
            id: Some("".to_string()),
            name: Some("search".to_string()),
            arguments: "{}".to_string(),
        }]);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(
            tool_calls[0].wire_tool_call_id(),
            tool_calls[0].invocation_id().as_str()
        );
    }

    #[test]
    fn parse_zai_flush_time_unix_timestamp() {
        let future_ts = (chrono::Utc::now().timestamp() + 300).to_string();
        let message = format!("Usage limit reached. Your limit will reset at {future_ts}");

        let wait_secs = parse_zai_flush_time(&message).expect("unix timestamp parses");
        assert!((wait_secs as i64 - 300).abs() < 5, "~300 seconds");
    }

    #[test]
    fn parse_zai_flush_time_milliseconds() {
        let future_ms = (chrono::Utc::now().timestamp_millis() + 300_000).to_string();
        let message = format!("Usage limit reached. Your limit will reset at {future_ms}");

        let wait_secs = parse_zai_flush_time(&message).expect("millisecond timestamp parses");
        assert!((wait_secs as i64 - 300).abs() < 5, "~300 seconds");
    }

    #[test]
    fn parse_zai_flush_time_iso_datetime() {
        let future_dt = chrono::Utc::now() + chrono::Duration::minutes(5);
        let message = format!(
            "Usage limit reached. Your limit will reset at {}",
            future_dt.format("%Y-%m-%dT%H:%M:%SZ")
        );

        let wait_secs = parse_zai_flush_time(&message).expect("ISO datetime parses");
        assert!(wait_secs >= 200, "~5 minutes");
    }

    #[test]
    fn parse_zai_flush_time_no_timestamp() {
        let wait_secs = parse_zai_flush_time("Rate limit exceeded. Please try again later.");
        assert_eq!(wait_secs, None);
    }

    #[tokio::test]
    async fn zai_chat_with_tools_uses_sse_transport() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\",\"reasoning_content\":\"reason\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let api_base = run_single_response_server(body, "text/event-stream").await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );
        let tools = vec![sample_tool()];

        let response = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &tools,
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: false,
                reasoning_effort: None,
            })
            .await
            .expect("SSE response parses");

        assert_eq!(response.content.as_deref(), Some("hello"));
        assert_eq!(response.reasoning_content.as_deref(), Some("reason"));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.expect("usage").total_tokens, 5);
    }

    #[tokio::test]
    async fn zai_native_json_chat_uses_non_streaming_transport() {
        let body = r#"{"choices":[{"message":{"content":"{\"ok\":true}","role":"assistant"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#;
        let api_base = run_single_response_server(body, "application/json").await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );

        let response = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &[],
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect("JSON response parses");

        assert_eq!(response.content.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.expect("usage").total_tokens, 3);
    }

    #[tokio::test]
    async fn zai_streaming_429_uses_next_flush_time() {
        let future_ts = chrono::Utc::now().timestamp() + 240;
        let body = format!(
            r#"{{"error":{{"message":"Usage limit reached. Your limit will reset at {future_ts}"}}}}"#
        );
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            body,
            "application/json",
            &[],
        )
        .await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );
        let tools = vec![sample_tool()];

        let err = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &tools,
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: false,
                reasoning_effort: None,
            })
            .await
            .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, message } => {
                let wait_secs = wait_secs.expect("next_flush_time should be parsed");
                assert!((wait_secs as i64 - 240).abs() < 5, "~240 seconds");
                assert!(message.contains("Usage limit reached"));
            }
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zai_native_json_429_uses_next_flush_time() {
        let future_ts = chrono::Utc::now().timestamp() + 180;
        let body = format!(
            r#"{{"error":{{"message":"Usage limit reached. Your limit will reset at {future_ts}"}}}}"#
        );
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            body,
            "application/json",
            &[],
        )
        .await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );

        let err = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &[],
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, message } => {
                let wait_secs = wait_secs.expect("next_flush_time should be parsed");
                assert!((wait_secs as i64 - 180).abs() < 5, "~180 seconds");
                assert!(message.contains("Usage limit reached"));
            }
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn generic_streaming_429_uses_retry_after_header() {
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            r#"{"error":"rate limit"}"#,
            "application/json",
            &[("Retry-After", "17")],
        )
        .await;

        let err = send_streaming_chat_request(
            &reqwest::Client::new(),
            &format!("{api_base}/chat/completions"),
            &json!({"stream": true}),
            None,
            &generic_profile(),
        )
        .await
        .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, .. } => assert_eq!(wait_secs, Some(17)),
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[test]
    fn json_mode_not_added_when_tools_present() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "local-model",
            1024,
            None,
            true, // json_mode = true
            None,
            &mistral_profile(),
            &mut mapper,
        );

        // response_format should NOT be present when tools are present
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn json_mode_added_without_tools_for_mistral_profile() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "local-model",
            1024,
            None,
            true, // json_mode = true
            None,
            &mistral_profile(),
            &mut mapper,
        );

        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }
}
