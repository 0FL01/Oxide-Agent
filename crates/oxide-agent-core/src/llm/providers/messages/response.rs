//! Anthropic-compatible Messages response parsing, usage extraction, and errors.

use crate::llm::providers::protocol_profiles::ANTHROPIC_CLIENT_TOOL_PROFILE;
use crate::llm::{ChatResponse, LlmError, TokenUsage};
use serde_json::Value;

use super::MessagesProfile;

/// Parse an Anthropic Messages API JSON response into a `ChatResponse`.
///
/// Extracts text content, tool_use blocks, thinking/redacted_thinking blocks,
/// stop_reason, and usage. Falls back to `profile.empty_tool_id_fallback_prefix`
/// when the provider returns empty tool_use IDs.
pub(crate) fn parse_response(
    response: Value,
    profile: MessagesProfile,
) -> Result<ChatResponse, LlmError> {
    if let Some(error) = extract_error_response(&response) {
        return Err(LlmError::api_error(error));
    }

    let blocks = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            LlmError::api_error(format!(
                "Missing content blocks in {} messages response",
                profile.label
            ))
        })?;
    let mut content_parts = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for (index, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    content_parts.push(text.to_string());
                }
            }
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let input = block.get("input").unwrap_or(&Value::Null);
                let arguments = if input.is_null() {
                    "{}".to_string()
                } else {
                    serde_json::to_string(input).unwrap_or_default()
                };
                let wire_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| {
                        format!(
                            "{}{index}",
                            profile.empty_tool_id_fallback_prefix.unwrap_or("")
                        )
                    });
                tool_calls.push(ANTHROPIC_CLIENT_TOOL_PROFILE.inbound_provider_tool_call(
                    wire_id.as_str(),
                    None,
                    name.to_string(),
                    arguments,
                ));
            }
            Some("thinking") => {
                if let Some(thinking) = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|thinking| !thinking.is_empty())
                {
                    reasoning_parts.push(thinking.to_string());
                }
            }
            Some("redacted_thinking") => {
                if let Some(data) = block
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|data| !data.is_empty())
                {
                    reasoning_parts.push(data.to_string());
                }
            }
            _ => {}
        }
    }

    let content = (!content_parts.is_empty()).then(|| content_parts.join("\n"));
    let reasoning_content = (!reasoning_parts.is_empty()).then(|| reasoning_parts.join("\n"));
    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::api_error(format!(
            "Empty {} messages response",
            profile.label
        )));
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason: response
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(super::request::map_stop_reason)
            .unwrap_or_else(|| "unknown".to_string()),
        reasoning_content,
        usage: response.get("usage").and_then(parse_usage),
    })
}

/// Parse Anthropic-format `usage` JSON into `TokenUsage`.
///
/// Handles `input_tokens`, `output_tokens`, `cache_read_input_tokens`,
/// and `cache_creation_input_tokens`.
pub(crate) fn parse_usage(value: &Value) -> Option<TokenUsage> {
    let input_tokens = value.get("input_tokens")?.as_u64()? as u32;
    let completion_tokens = value.get("output_tokens")?.as_u64()? as u32;
    let cached_tokens = value
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let cache_creation_tokens = value
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let prompt_tokens = input_tokens
        .saturating_add(cached_tokens.unwrap_or_default())
        .saturating_add(cache_creation_tokens.unwrap_or_default());
    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens.saturating_add(completion_tokens),
        cached_tokens,
        cache_creation_tokens,
    })
}

/// Check if a model ID refers to a reasoning/thinking model.
///
/// Matches DeepSeek V4 family and MiMo V2 family.
pub(crate) fn is_reasoning_model(model_id: &str) -> bool {
    let lower = model_id.trim().to_ascii_lowercase();
    lower.starts_with("deepseek-v4") || lower.starts_with("mimo-v2")
}

/// Check if reasoning effort is explicitly disabled.
pub(crate) fn disables_reasoning(reasoning_effort: Option<&str>) -> bool {
    reasoning_effort
        .map(str::trim)
        .map(|effort| {
            effort.eq_ignore_ascii_case("none") || effort.eq_ignore_ascii_case("disabled")
        })
        .unwrap_or(false)
}

/// Extract an error message from an Anthropic-style error response envelope.
///
/// Handles `{"error": {"message": "...", "code": "..."}}` and
/// top-level `{"message": "..."}` / `{"detail": "..."}` formats.
pub(crate) fn extract_error_response(response: &Value) -> Option<String> {
    if let Some(error) = response.get("error") {
        if let Some(message) = non_empty_str(error.get("message")) {
            return Some(format_anthropic_error(error, message));
        }
        if let Some(message) = non_empty_str(Some(error)) {
            return Some(format!("Anthropic returned error response: {message}"));
        }
    }

    non_empty_str(response.get("message"))
        .or_else(|| non_empty_str(response.get("detail")))
        .map(|message| format!("Anthropic returned error response: {message}"))
}

fn format_anthropic_error(error: &Value, message: &str) -> String {
    let label = non_empty_str(error.get("code")).or_else(|| non_empty_str(error.get("type")));
    match label {
        Some(label) => format!("Anthropic returned error response ({label}): {message}"),
        None => format!("Anthropic returned error response: {message}"),
    }
}

/// Extract a non-empty, trimmed string from an `Option<&Value>`.
pub(crate) fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_response_extracts_text_tool_calls_reasoning_and_usage() {
        let response = parse_response(
            json!({
                "content": [
                    { "type": "thinking", "thinking": "internal reasoning" },
                    { "type": "text", "text": "Use a tool" },
                    {
                        "type": "tool_use",
                        "id": "toolu-1",
                        "name": "read_file",
                        "input": { "path": "Cargo.toml" }
                    }
                ],
                "stop_reason": "tool_use",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5
                }
            }),
            MessagesProfile::opencode_go(),
        )
        .expect("response parses");

        assert_eq!(response.content.as_deref(), Some("Use a tool"));
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("internal reasoning")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].wire_tool_call_id(), "toolu-1");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"path":"Cargo.toml"}"#
        );
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.usage.expect("usage").total_tokens, 15);
    }

    #[test]
    fn parse_response_generates_fallback_id_for_empty_tool_id() {
        let response = parse_response(
            json!({
                "content": [
                    {
                        "type": "tool_use",
                        "id": "  ",
                        "name": "read_file",
                        "input": {}
                    }
                ],
                "stop_reason": "tool_use"
            }),
            MessagesProfile::opencode_go(),
        )
        .expect("response parses");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].wire_tool_call_id(),
            "opencode_go_tool_use_0"
        );
    }

    #[test]
    fn parse_response_keeps_redacted_thinking_and_null_tool_input() {
        let response = parse_response(
            json!({
                "content": [
                    { "type": "redacted_thinking", "data": "encrypted-reasoning" },
                    {
                        "type": "tool_use",
                        "id": "toolu-null",
                        "name": "read_file",
                        "input": null
                    }
                ],
                "stop_reason": "tool_use"
            }),
            MessagesProfile::opencode_go(),
        )
        .expect("response parses");

        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("encrypted-reasoning")
        );
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].wire_tool_call_id(), "toolu-null");
        assert_eq!(response.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn parse_response_rejects_missing_or_empty_content_blocks() {
        let missing = parse_response(
            json!({ "stop_reason": "end_turn" }),
            MessagesProfile::anthropic(),
        )
        .expect_err("missing content should fail");
        assert!(missing.to_string().contains("Missing content blocks"));

        let empty = parse_response(
            json!({
                "content": [
                    { "type": "text", "text": "   " },
                    { "type": "tool_use", "id": "toolu-skip" }
                ],
                "stop_reason": "end_turn"
            }),
            MessagesProfile::anthropic(),
        )
        .expect_err("empty content should fail");
        assert!(
            empty
                .to_string()
                .contains("Empty Anthropic messages response")
        );
    }

    #[test]
    fn parse_usage_extracts_cache_fields() {
        let usage = parse_usage(&json!({
            "input_tokens": 3840,
            "output_tokens": 512,
            "cache_read_input_tokens": 2560,
            "cache_creation_input_tokens": 128
        }))
        .expect("usage should parse");

        assert_eq!(usage.prompt_tokens, 6528);
        assert_eq!(usage.total_tokens, 7040);
        assert_eq!(usage.cached_tokens, Some(2560));
        assert_eq!(usage.cache_creation_tokens, Some(128));
    }

    #[test]
    fn parse_usage_returns_none_when_no_cache_fields() {
        let usage = parse_usage(&json!({
            "input_tokens": 10,
            "output_tokens": 5
        }))
        .expect("usage should parse");

        assert_eq!(usage.cached_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
    }

    #[test]
    fn parse_usage_returns_none_when_no_input_tokens() {
        assert!(parse_usage(&json!({ "output_tokens": 5 })).is_none());
    }

    #[test]
    fn extract_error_response_handles_standard_envelope() {
        let error = extract_error_response(&json!({
            "error": {
                "message": "invalid request",
                "type": "invalid_request_error"
            }
        }));
        assert_eq!(
            error,
            Some(
                "Anthropic returned error response (invalid_request_error): invalid request"
                    .to_string()
            )
        );
    }

    #[test]
    fn extract_error_response_handles_top_level_message() {
        let error = extract_error_response(&json!({
            "message": "something went wrong"
        }));
        assert_eq!(
            error,
            Some("Anthropic returned error response: something went wrong".to_string())
        );
    }

    #[test]
    fn extract_error_response_handles_detail_and_error_code() {
        let detail = extract_error_response(&json!({
            "detail": "bad request"
        }));
        assert_eq!(
            detail,
            Some("Anthropic returned error response: bad request".to_string())
        );

        let coded = extract_error_response(&json!({
            "error": {
                "message": "slow down",
                "code": "rate_limit"
            }
        }));
        assert_eq!(
            coded,
            Some("Anthropic returned error response (rate_limit): slow down".to_string())
        );
    }

    #[test]
    fn extract_error_response_returns_none_for_successful_response() {
        let error = extract_error_response(&json!({
            "content": [{ "type": "text", "text": "hello" }],
            "stop_reason": "end_turn"
        }));
        assert!(error.is_none());
    }

    #[test]
    fn is_reasoning_model_matches_deepseek_v4_and_mimo_v2() {
        assert!(is_reasoning_model("deepseek-v4-flash"));
        assert!(is_reasoning_model("deepseek-v4-pro"));
        assert!(is_reasoning_model("mimo-v2.5"));
        assert!(is_reasoning_model("mimo-v2.5-pro"));
        assert!(!is_reasoning_model("deepseek-v3"));
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("MiniMax-M3"));
    }

    #[test]
    fn disables_reasoning_detects_none_and_disabled() {
        assert!(disables_reasoning(Some("none")));
        assert!(disables_reasoning(Some("disabled")));
        assert!(disables_reasoning(Some("NONE")));
        assert!(!disables_reasoning(Some("high")));
        assert!(!disables_reasoning(None));
    }

    #[test]
    fn non_empty_str_trims_whitespace() {
        assert_eq!(non_empty_str(Some(&json!("  hello  "))), Some("hello"));
        assert_eq!(non_empty_str(Some(&json!("   "))), None);
        assert_eq!(non_empty_str(Some(&json!(""))), None);
        assert_eq!(non_empty_str(None), None);
    }
}
