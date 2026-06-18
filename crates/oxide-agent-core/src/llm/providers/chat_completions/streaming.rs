//! Chat Completions SSE parsing boundary.
//!
//! ChatGPT Responses/Codex streaming remains in `providers::chatgpt`; this
//! module is only for OpenAI-compatible Chat Completions event schemas.

use super::profile::{ChatCompletionsProfile, ChatStreamingPolicy};
use super::response::parse_usage;
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::sse;
use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall};
use futures_util::StreamExt;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChatCompletionsStreamingPlan {
    pub(crate) policy: ChatStreamingPolicy,
}

impl ChatCompletionsStreamingPlan {
    #[must_use]
    pub(crate) const fn new(policy: ChatStreamingPolicy) -> Self {
        Self { policy }
    }
}

#[derive(Default)]
pub(crate) struct StreamingChatAccumulator {
    pub(crate) content: String,
    pub(crate) reasoning_content: String,
    pub(crate) finish_reason: String,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) pending_tool_calls: Vec<PendingStreamingToolCall>,
}

#[derive(Default)]
pub(crate) struct PendingStreamingToolCall {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
}

pub(crate) async fn parse_streaming_chat_response(
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
        let chunk = chunk.map_err(LlmError::from_reqwest_error)?;
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

pub(crate) fn process_chat_sse_event(
    raw_event: &str,
    state: &mut StreamingChatAccumulator,
) -> Result<(), LlmError> {
    let payload = sse::data_payload(raw_event);
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

    if let Some(usage) = chunk.get("usage").and_then(parse_usage) {
        state.usage = Some(usage);
    }

    Ok(())
}

pub(crate) fn finish_streaming_chat_response(
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

pub(crate) fn finalize_streaming_tool_calls(
    pending: Vec<PendingStreamingToolCall>,
) -> Vec<ToolCall> {
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

pub(crate) fn decode_utf8_prefix(pending_bytes: &mut Vec<u8>) -> Result<Option<String>, LlmError> {
    sse::decode_utf8_prefix(pending_bytes, "OpenAI-compatible stream")
}

pub(crate) fn normalize_newlines_in_place(buffer: &mut String) {
    sse::normalize_newlines_in_place(buffer);
}

#[must_use]
pub(crate) fn should_stream(profile: ChatCompletionsProfile, native_json_mode: bool) -> bool {
    match profile.streaming {
        ChatStreamingPolicy::NonStreaming => false,
        ChatStreamingPolicy::ZaiUnlessNativeJsonMode => !native_json_mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;

    #[test]
    fn chat_completions_stream_accumulates_content_and_reasoning() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"reasoning_content":"think ","content":"hello"}}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
            &mut state,
        )
        .expect("first chunk parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"reasoning_content":"more","content":" world"},"finish_reason":"stop"}]}"#,
            &mut state,
        )
        .expect("second chunk parses");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.content.as_deref(), Some("hello world"));
        assert_eq!(response.reasoning_content.as_deref(), Some("think more"));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(
            response.usage.as_ref().map(|usage| usage.total_tokens),
            Some(3)
        );
    }

    #[test]
    fn chat_completions_stream_accumulates_tool_call_deltas() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"search","arguments":"{\"q\":"}}]}}]}"#,
            &mut state,
        )
        .expect("first tool delta parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"oxide\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut state,
        )
        .expect("second tool delta parses");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].wire_tool_call_id(), "call_1");
        assert_eq!(response.tool_calls[0].function.name, "search");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"q":"oxide"}"#
        );
    }

    #[test]
    fn chat_completions_stream_zai_disabled_for_native_json() {
        let profile = ChatCompletionsProfile::zai();

        assert!(should_stream(profile, false));
        assert!(!should_stream(profile, true));
        assert!(!should_stream(ChatCompletionsProfile::generic(), false));
    }
}
