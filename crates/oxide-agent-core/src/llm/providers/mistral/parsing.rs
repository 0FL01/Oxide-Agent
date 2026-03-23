//! Response parsing utilities for Mistral API

use crate::llm::providers::mistral::id_mapper::ToolCallIdMapper;
use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall, ToolCallFunction};
use serde_json::Value;

/// Parse token usage from response
pub fn parse_usage(response: &Value) -> Option<TokenUsage> {
    let usage = response.get("usage")?;
    Some(TokenUsage {
        prompt_tokens: usage.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: usage.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: usage.get("total_tokens")?.as_u64()? as u32,
    })
}

/// Parse tool calls from Mistral API response message
///
/// Maps Mistral's 9-character IDs back to original UUID-based IDs using the mapper.
pub fn parse_tool_calls(message: &Value, id_mapper: &ToolCallIdMapper) -> Vec<ToolCall> {
    let Some(tool_calls_array) = message.get("tool_calls") else {
        return Vec::new();
    };

    let Some(array) = tool_calls_array.as_array() else {
        return Vec::new();
    };

    array
        .iter()
        .filter_map(|tc| {
            let mistral_id = tc.get("id")?.as_str()?.to_string();
            // Map back to original ID if known, otherwise use as-is
            let id = id_mapper.to_original(&mistral_id);

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

            Some(ToolCall {
                id,
                function: ToolCallFunction { name, arguments },
                is_recovered: false,
            })
        })
        .collect()
}

/// Legacy function without ID mapping (for backward compatibility)
#[deprecated(
    since = "0.1.0",
    note = "Use parse_tool_calls with id_mapper for proper Mistral compatibility"
)]
pub fn parse_tool_calls_legacy(message: &Value) -> Vec<ToolCall> {
    let dummy_mapper = ToolCallIdMapper::new();
    parse_tool_calls(message, &dummy_mapper)
}

/// Parse chat completion response
///
/// Maps Mistral's tool call IDs back to original IDs using the mapper.
pub fn parse_chat_response(
    response: Value,
    id_mapper: &ToolCallIdMapper,
) -> Result<ChatResponse, LlmError> {
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

    let (content, extracted_reasoning) = extract_message_content(message.get("content"));
    let reasoning_content = extracted_reasoning.or_else(|| extract_reasoning_content(message));

    // Parse tool_calls from the response, mapping IDs back to original
    let tool_calls = parse_tool_calls(message, id_mapper);

    // Allow empty content if there are tool_calls or reasoning_content
    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::ApiError("Empty response".to_string()));
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage: parse_usage(&response),
    })
}

/// Legacy function without ID mapping (for backward compatibility)
#[deprecated(
    since = "0.1.0",
    note = "Use parse_chat_response with id_mapper for proper Mistral compatibility"
)]
pub fn parse_chat_response_legacy(response: Value) -> Result<ChatResponse, LlmError> {
    let dummy_mapper = ToolCallIdMapper::new();
    parse_chat_response(response, &dummy_mapper)
}

/// Extract text segments from JSON value recursively
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

/// Join text segments into single string
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

/// Extract message content and reasoning from response
///
/// Handles both simple string content and array content with thinking/reasoning
pub fn extract_message_content(content: Option<&Value>) -> (Option<String>, Option<String>) {
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

/// Extract reasoning content from message
fn extract_reasoning_content(message: &Value) -> Option<String> {
    join_segments(extract_text_segments(message.get("reasoning_content")?))
}
