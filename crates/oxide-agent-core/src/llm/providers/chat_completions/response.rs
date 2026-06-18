//! Response parsing for the shared Chat Completions wire path.

use super::profile::{
    ChatCompletionsProfile, ChatResponseContentPolicy, EmptyToolCallIdPolicy, RateLimitPolicy,
};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall};
use serde_json::Value;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChatCompletionsResponsePlan {
    pub(crate) profile: ChatCompletionsProfile,
}

impl ChatCompletionsResponsePlan {
    #[must_use]
    pub(crate) const fn new(profile: ChatCompletionsProfile) -> Self {
        Self { profile }
    }
}

pub(crate) trait ChatToolCallIdResolver {
    fn original_tool_call_id(&self, provider_id: &str) -> String;
    fn has_provider_tool_call_id(&self, provider_id: &str) -> bool;
}

#[cfg(feature = "llm-openai-base")]
impl ChatToolCallIdResolver for crate::llm::providers::openai_base::ToolCallIdMapper {
    fn original_tool_call_id(&self, provider_id: &str) -> String {
        self.to_original(provider_id)
    }

    fn has_provider_tool_call_id(&self, provider_id: &str) -> bool {
        self.has_mistral_id(provider_id)
    }
}

pub(crate) fn parse_chat_response(
    response: Value,
    profile: ChatCompletionsProfile,
    id_resolver: Option<&dyn ChatToolCallIdResolver>,
) -> Result<ChatResponse, LlmError> {
    if let Some(error) = extract_error_response(profile, &response) {
        return Err(LlmError::api_error(error));
    }

    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::api_error(format!(
                "Missing choices[0] in {} response{}",
                response_label(profile),
                response_shape_suffix(&response)
            ))
        })?;
    let message = choice.get("message").ok_or_else(|| {
        LlmError::api_error(format!(
            "Missing message in {} response",
            response_label(profile)
        ))
    })?;

    let (content, reasoning_content, tool_calls) = match profile.response_content {
        ChatResponseContentPolicy::StringOnly => {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .filter(|content| !content.is_empty())
                .map(ToString::to_string);
            let reasoning_content = parse_reasoning_content(message);
            let tool_calls = parse_message_tool_calls(message, profile, id_resolver)?;
            (content, reasoning_content, tool_calls)
        }
        ChatResponseContentPolicy::StringOrChunkArrayWithReasoning => {
            let (content, extracted_reasoning) = extract_message_content(message.get("content"));
            let reasoning_content =
                extracted_reasoning.or_else(|| parse_reasoning_content(message));
            let tool_calls = parse_message_tool_calls(message, profile, id_resolver)?;
            (content, reasoning_content, tool_calls)
        }
    };

    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::api_error(format!(
            "Empty {} response",
            response_label(profile)
        )));
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

pub(crate) fn parse_tool_calls(
    value: &Value,
    profile: ChatCompletionsProfile,
    id_resolver: Option<&dyn ChatToolCallIdResolver>,
) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(format!(
            "Invalid tool_calls format from {}",
            response_label(profile)
        )));
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
        let provider_id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());

        tool_calls.push(match provider_id {
            Some(provider_id) => correlated_tool_call(provider_id, id_resolver, name, arguments),
            None => empty_id_tool_call(profile, index, name, arguments),
        });
    }

    Ok(tool_calls)
}

fn parse_message_tool_calls(
    message: &Value,
    profile: ChatCompletionsProfile,
    id_resolver: Option<&dyn ChatToolCallIdResolver>,
) -> Result<Vec<ToolCall>, LlmError> {
    match message.get("tool_calls") {
        Some(value) if value.is_null() => Ok(Vec::new()),
        Some(value) if value.is_array() => parse_tool_calls(value, profile, id_resolver),
        Some(_) => Err(LlmError::JsonError(format!(
            "Invalid tool_calls format from {}",
            response_label(profile)
        ))),
        None => Ok(Vec::new()),
    }
}

fn correlated_tool_call(
    provider_id: &str,
    id_resolver: Option<&dyn ChatToolCallIdResolver>,
    name: &str,
    arguments: String,
) -> ToolCall {
    if let Some(resolver) = id_resolver
        && resolver.has_provider_tool_call_id(provider_id)
    {
        return CHAT_LIKE_TOOL_PROFILE.inbound_tool_call(
            resolver.original_tool_call_id(provider_id),
            Some(provider_id),
            None,
            name.to_string(),
            arguments,
        );
    }

    CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
        provider_id,
        None,
        name.to_string(),
        arguments,
    )
}

fn empty_id_tool_call(
    profile: ChatCompletionsProfile,
    index: usize,
    name: &str,
    arguments: String,
) -> ToolCall {
    match profile.empty_tool_call_id {
        EmptyToolCallIdPolicy::Uncorrelated => {
            debug!(
                provider = profile.label,
                tool_name = name,
                tool_index = index,
                "Chat Completions provider returned empty tool call ID"
            );
            CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name.to_string(), arguments)
        }
    }
}

pub(crate) fn normalize_tool_arguments(value: &Value) -> String {
    match value {
        Value::String(raw) => normalize_tool_arguments_str(raw),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

pub(crate) fn normalize_tool_arguments_str(raw: &str) -> String {
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

pub(crate) fn parse_usage(usage: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: usage.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: usage.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: usage.get("total_tokens")?.as_u64()? as u32,
        cached_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .map(|value| value as u32),
        cache_creation_tokens: None,
    })
}

pub(crate) fn parse_rate_limit_wait_secs(
    profile: ChatCompletionsProfile,
    message: &str,
    fallback: Option<u64>,
) -> Option<u64> {
    match profile.rate_limit {
        RateLimitPolicy::ZaiFlushTime => parse_zai_flush_time(message).or(fallback),
        RateLimitPolicy::OpenRouterResetMetadata => {
            parse_openrouter_rate_limit(message).or(fallback)
        }
        RateLimitPolicy::RetryAfterHeader => fallback,
    }
}

#[must_use]
pub(crate) fn parse_zai_flush_time(message: &str) -> Option<u64> {
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

pub(crate) fn parse_openrouter_rate_limit(body: &str) -> Option<u64> {
    let json: Value = serde_json::from_str(body).ok()?;
    let reset_ms = json
        .pointer("/error/metadata/headers/X-RateLimit-Reset")?
        .as_str()?
        .parse::<i64>()
        .ok()?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let wait_secs = (reset_ms - now_ms) / 1000;

    (wait_secs > 0).then_some(wait_secs as u64)
}

pub(crate) fn extract_error_response(
    profile: ChatCompletionsProfile,
    response: &Value,
) -> Option<String> {
    if profile.label != "opencode_go" {
        return None;
    }

    if let Some(error) = response.get("error") {
        if let Some(message) = non_empty_str(error.get("message")) {
            return Some(format_error_message(profile, error, message));
        }
        if let Some(message) = non_empty_str(Some(error)) {
            return Some(format!(
                "{} returned error response: {message}",
                response_label(profile)
            ));
        }
    }

    non_empty_str(response.get("message"))
        .or_else(|| non_empty_str(response.get("detail")))
        .map(|message| {
            format!(
                "{} returned error response: {message}",
                response_label(profile)
            )
        })
}

fn format_error_message(profile: ChatCompletionsProfile, error: &Value, message: &str) -> String {
    let label = non_empty_str(error.get("code")).or_else(|| non_empty_str(error.get("type")));
    match label {
        Some(label) => format!(
            "{} returned error response ({label}): {message}",
            response_label(profile)
        ),
        None => format!(
            "{} returned error response: {message}",
            response_label(profile)
        ),
    }
}

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
                        reasoning_segments.extend(extract_text_segments(item))
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

fn parse_reasoning_content(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .or_else(|| message.get("reasoning").and_then(Value::as_str))
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string)
        .or_else(|| join_segments(extract_text_segments(message.get("reasoning_content")?)))
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

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn response_shape_suffix(response: &Value) -> String {
    let Some(object) = response.as_object() else {
        return format!("; response_type={}", value_type_name(response));
    };
    if object.is_empty() {
        return "; top_level_keys=[]".to_string();
    }
    let keys = object
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    format!("; top_level_keys=[{keys}]")
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn response_label(profile: ChatCompletionsProfile) -> &'static str {
    match profile.label {
        "opencode_go" => "OpenCode Go",
        "openrouter" => "OpenRouter",
        "mistral" => "OpenAI-compatible provider",
        "zai" => "OpenAI-compatible provider",
        _ => "OpenAI-compatible provider",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestResolver;

    impl ChatToolCallIdResolver for TestResolver {
        fn original_tool_call_id(&self, provider_id: &str) -> String {
            if provider_id == "abc123xyz" {
                "call-original".to_string()
            } else {
                provider_id.to_string()
            }
        }

        fn has_provider_tool_call_id(&self, provider_id: &str) -> bool {
            provider_id == "abc123xyz"
        }
    }

    #[test]
    fn chat_completions_parse_tool_calls_preserves_wire_ids() {
        let calls = parse_tool_calls(
            &json!([{
                "id": "call-openai-1",
                "type": "function",
                "function": {"name": "search", "arguments": {"q": "oxide"}}
            }]),
            ChatCompletionsProfile::generic(),
            None,
        )
        .expect("tool calls parse");

        assert_ne!(calls[0].invocation_id().as_str(), "call-openai-1");
        assert_eq!(calls[0].wire_tool_call_id(), "call-openai-1");
        assert_eq!(calls[0].function.arguments, r#"{"q":"oxide"}"#);
    }

    #[test]
    fn chat_completions_parse_empty_tool_call_id_uses_profile_policy() {
        let calls = parse_tool_calls(
            &json!([{
                "id": "  ",
                "type": "function",
                "function": {"name": "search", "arguments": "{}"}
            }]),
            ChatCompletionsProfile::openrouter(),
            None,
        )
        .expect("tool calls parse");

        assert_eq!(
            calls[0].wire_tool_call_id(),
            calls[0].invocation_id().as_str()
        );
    }

    #[test]
    fn chat_completions_parse_mistral_tool_call_reverse_maps_id() {
        let parsed = parse_chat_response(
            json!({
                "choices": [{
                    "message": {
                        "content": null,
                        "tool_calls": [{
                            "id": "abc123xyz",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
            ChatCompletionsProfile::mistral(),
            Some(&TestResolver),
        )
        .expect("response parses");

        assert_eq!(
            parsed.tool_calls[0].invocation_id().as_str(),
            "call-original"
        );
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "abc123xyz");
    }

    #[test]
    fn chat_completions_parse_zai_chunk_array_content_and_reasoning() {
        let parsed = parse_chat_response(
            json!({
                "choices": [{
                    "message": {
                        "content": [
                            {"type": "thinking", "content": "reason"},
                            {"type": "text", "text": "answer"}
                        ]
                    },
                    "finish_reason": "stop"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
            }),
            ChatCompletionsProfile::zai(),
            None,
        )
        .expect("response parses");

        assert_eq!(parsed.content.as_deref(), Some("answer"));
        assert_eq!(parsed.reasoning_content.as_deref(), Some("reason"));
    }

    #[test]
    fn chat_completions_parse_openai_usage() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15,
            "prompt_tokens_details": {"cached_tokens": 7}
        }))
        .expect("usage parses");

        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.cached_tokens, Some(7));
    }

    #[test]
    fn chat_completions_parse_openrouter_rate_limit_metadata() {
        let reset_ms = chrono::Utc::now().timestamp_millis() + 120_000;
        let body = json!({
            "error": {
                "metadata": {
                    "headers": {"X-RateLimit-Reset": reset_ms.to_string()}
                }
            }
        })
        .to_string();

        let wait_secs =
            parse_rate_limit_wait_secs(ChatCompletionsProfile::openrouter(), &body, None)
                .expect("reset parses");
        assert!((115..=120).contains(&wait_secs));
    }
}
