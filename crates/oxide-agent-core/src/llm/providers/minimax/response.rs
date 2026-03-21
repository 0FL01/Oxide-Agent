//! Response parsing utilities for MiniMax provider

use claudius::{ContentBlock, ThinkingBlock};

use crate::llm::{ChatResponse, TokenUsage, ToolCall, ToolCallFunction};

/// Convert claudius Message to our ChatResponse
///
/// Extracts text content, tool calls, reasoning/thinking content,
/// and usage information from the response.
pub fn from_claudius_message(msg: claudius::Message) -> Result<ChatResponse, crate::llm::LlmError> {
    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut reasoning_content: Option<String> = None;

    for block in msg.content {
        match block {
            ContentBlock::Text(text_block) => {
                let text = text_block.text.trim().to_string();
                if !text.is_empty() {
                    content = Some(text);
                }
            }
            ContentBlock::ToolUse(tool_use) => {
                tool_calls.push(ToolCall {
                    id: tool_use.id,
                    function: ToolCallFunction {
                        name: tool_use.name,
                        arguments: serde_json::to_string(&tool_use.input).unwrap_or_default(),
                    },
                    is_recovered: false,
                });
            }
            ContentBlock::Thinking(thinking) => {
                // Extended thinking content
                reasoning_content = Some(extract_thinking_content(&thinking));
            }
            ContentBlock::RedactedThinking(redacted) => {
                // Redacted thinking - data is a String, not iterable
                let text = redacted.data.trim().to_string();
                if !text.is_empty() {
                    reasoning_content = Some(text);
                }
            }
            // Other block types we don't need to handle specially
            _ => {}
        }
    }

    let finish_reason = msg
        .stop_reason
        .map(|sr| match sr {
            claudius::StopReason::EndTurn => "stop".to_string(),
            claudius::StopReason::ToolUse => "tool_calls".to_string(),
            claudius::StopReason::StopSequence => "stop".to_string(),
            claudius::StopReason::MaxTokens => "length".to_string(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Convert usage information
    let usage = TokenUsage {
        prompt_tokens: msg.usage.input_tokens as u32,
        completion_tokens: msg.usage.output_tokens as u32,
        total_tokens: (msg.usage.input_tokens + msg.usage.output_tokens) as u32,
    };

    // Allow empty content if there are tool_calls or reasoning_content
    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(crate::llm::LlmError::ApiError("Empty response".to_string()));
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage: Some(usage),
    })
}

/// Extract thinking content from ThinkingBlock
fn extract_thinking_content(thinking: &ThinkingBlock) -> String {
    // thinking field is a String, not a collection
    thinking.thinking.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use claudius::{StopReason, TextBlock, ThinkingBlock, ToolUseBlock, Usage};

    fn make_text_block(text: &str) -> ContentBlock {
        ContentBlock::Text(TextBlock::new(text.to_string()))
    }

    #[test]
    fn parses_text_response() {
        let msg = claudius::Message::new(
            "msg_123".to_string(),
            vec![make_text_block("Hello, world!")],
            claudius::Model::Custom("MiniMax-M2".to_string()),
            Usage::new(10, 5),
        )
        .with_stop_reason(StopReason::EndTurn);

        let response = from_claudius_message(msg).expect("should parse");

        assert_eq!(response.content, Some("Hello, world!".to_string()));
        assert!(response.tool_calls.is_empty());
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn parses_tool_call_response() {
        let msg = claudius::Message::new(
            "msg_123".to_string(),
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "call_abc",
                "get_weather",
                serde_json::json!({"city": "Moscow"}),
            ))],
            claudius::Model::Custom("MiniMax-M2".to_string()),
            Usage::new(10, 5),
        )
        .with_stop_reason(StopReason::ToolUse);

        let response = from_claudius_message(msg).expect("should parse");

        assert!(response.content.is_none());
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.tool_calls[0].id, "call_abc");
        assert_eq!(response.tool_calls[0].function.name, "get_weather");
        assert_eq!(response.finish_reason, "tool_calls");
    }

    #[test]
    fn parses_thinking_content() {
        let thinking_text = "Let me think...\nThe answer is 42.";
        let msg = claudius::Message::new(
            "msg_123".to_string(),
            vec![
                ContentBlock::Thinking(ThinkingBlock::new(thinking_text, "sig123")),
                make_text_block("The answer is 42."),
            ],
            claudius::Model::Custom("MiniMax-M2".to_string()),
            Usage::new(10, 5),
        )
        .with_stop_reason(StopReason::EndTurn);

        let response = from_claudius_message(msg).expect("should parse");

        assert_eq!(response.content, Some("The answer is 42.".to_string()));
        let reasoning = response
            .reasoning_content
            .expect("reasoning_content should be present");
        assert!(reasoning.contains("Let me think..."));
    }

    #[test]
    fn parses_usage_information() {
        let msg = claudius::Message::new(
            "msg_123".to_string(),
            vec![make_text_block("Hi!")],
            claudius::Model::Custom("MiniMax-M2".to_string()),
            Usage::new(10, 5),
        )
        .with_stop_reason(StopReason::EndTurn);

        let response = from_claudius_message(msg).expect("should parse");

        let usage = response.usage.expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn errors_on_empty_response() {
        let msg = claudius::Message::new(
            "msg_123".to_string(),
            vec![],
            claudius::Model::Custom("MiniMax-M2".to_string()),
            Usage::new(10, 5),
        );

        let result = from_claudius_message(msg);
        assert!(result.is_err());
    }
}
