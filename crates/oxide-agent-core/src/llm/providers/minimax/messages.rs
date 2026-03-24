//! Message conversion utilities for MiniMax provider

use claudius::{
    ContentBlock, MessageParam, MessageParamContent, MessageRole, TextBlock, ToolResultBlock,
    ToolUseBlock,
};
use serde_json::Value;

use crate::llm::providers::tool_call_adapter::ProviderToolCallAdapter;
use crate::llm::providers::tool_result_encoder::{ProviderToolResultEncoder, ToolResultEncoder};
use crate::llm::{Message, ToolProtocol, ToolTransport};

const MINIMAX_TOOL_ADAPTER: ProviderToolCallAdapter = ProviderToolCallAdapter::new(
    ToolProtocol::AnthropicClientTools,
    ToolTransport::ClientRoundTrip,
);
const MINIMAX_TOOL_RESULT_ENCODER: ProviderToolResultEncoder = ProviderToolResultEncoder::new(
    ToolProtocol::AnthropicClientTools,
    ToolTransport::ClientRoundTrip,
);

/// Convert our Message to claudius MessageParam
///
/// Handles system, user, assistant, and tool messages with proper
/// conversion of tool calls and tool results.
#[must_use]
pub fn to_claudius_message(msg: &Message) -> MessageParam {
    let role = match msg.role.as_str() {
        "system" => MessageRole::User, // No System role in Anthropic, send as User
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        "tool" => MessageRole::User, // Tools return as user messages in Anthropic API
        _ => MessageRole::User,
    };

    let content = build_message_content(msg);

    MessageParam { role, content }
}

/// Build message content based on message type
fn build_message_content(msg: &Message) -> MessageParamContent {
    match msg.role.as_str() {
        "system" | "user" => {
            // Simple text message
            MessageParamContent::String(msg.content.clone())
        }
        "assistant" => {
            let mut content_blocks = Vec::new();

            // Add text content if present
            if !msg.content.is_empty() {
                content_blocks.push(ContentBlock::Text(TextBlock::new(msg.content.clone())));
            }

            // Add tool use blocks if present
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let input: Value = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(Value::Object(serde_json::Map::new()));

                    content_blocks.push(ContentBlock::ToolUse(ToolUseBlock::new(
                        MINIMAX_TOOL_ADAPTER.assistant_tool_call_id(tc),
                        tc.function.name.clone(),
                        input,
                    )));
                }
            }

            // If no content and no tool calls, add empty text to satisfy API
            if content_blocks.is_empty() {
                content_blocks.push(ContentBlock::Text(TextBlock::new(String::new())));
            }

            // Wrap in Array variant
            MessageParamContent::Array(content_blocks)
        }
        "tool" => MessageParamContent::Array(vec![anthropic_tool_result_block(msg)]),
        _ => MessageParamContent::String(msg.content.clone()),
    }
}

fn anthropic_tool_result_block(msg: &Message) -> ContentBlock {
    match MINIMAX_TOOL_RESULT_ENCODER
        .encode(msg)
        .and_then(|result| result.into_anthropic())
    {
        Some(result) => ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: result.tool_use_id,
            content: Some(result.content.into()),
            is_error: result.is_error,
            cache_control: None,
        }),
        None => ContentBlock::ToolResult(ToolResultBlock {
            tool_use_id: String::new(),
            content: Some(msg.content.clone().into()),
            is_error: None,
            cache_control: None,
        }),
    }
}

/// Convert a slice of messages to claudius MessageParams
#[must_use]
pub fn to_claudius_messages(messages: &[Message]) -> Vec<MessageParam> {
    let mut params = Vec::with_capacity(messages.len());
    let mut index = 0;

    while index < messages.len() {
        let msg = &messages[index];

        if msg.role == "tool" {
            let mut blocks = Vec::new();
            let mut cursor = index;

            while cursor < messages.len() && messages[cursor].role == "tool" {
                blocks.push(anthropic_tool_result_block(&messages[cursor]));
                cursor += 1;
            }

            if !blocks.is_empty() {
                params.push(MessageParam {
                    role: MessageRole::User,
                    content: MessageParamContent::Array(blocks),
                });
                index = cursor;
                continue;
            }
        }

        params.push(to_claudius_message(msg));
        index += 1;
    }

    params
}

/// Create a user message param
#[must_use]
pub fn new_user_message(content: &str) -> MessageParam {
    MessageParam::new(
        MessageParamContent::String(content.to_string()),
        MessageRole::User,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCall, ToolCallCorrelation, ToolCallFunction};

    #[test]
    fn converts_user_message() {
        let msg = Message::user("Hello, world!");
        let param = to_claudius_message(&msg);

        assert!(matches!(param.role, MessageRole::User));
        if let MessageParamContent::String(text) = &param.content {
            assert_eq!(text, "Hello, world!");
        } else {
            panic!("Expected String content");
        }
    }

    #[test]
    fn converts_assistant_message_with_text() {
        let msg = Message::assistant("Hello!");
        let param = to_claudius_message(&msg);

        assert!(matches!(param.role, MessageRole::Assistant));
        if let MessageParamContent::Array(blocks) = &param.content {
            assert!(!blocks.is_empty());
            if let ContentBlock::Text(text_block) = &blocks[0] {
                assert_eq!(text_block.text, "Hello!");
            } else {
                panic!("Expected Text content block");
            }
        } else {
            panic!("Expected Array content");
        }
    }

    #[test]
    fn converts_assistant_message_with_tool_calls() {
        let msg = Message::assistant_with_tools(
            "I'll check the weather.",
            vec![ToolCall::new(
                "invoke-weather-1".to_string(),
                ToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: r#"{"city":"Moscow"}"#.to_string(),
                },
                false,
            )
            .with_correlation(
                ToolCallCorrelation::new("invoke-weather-1")
                    .with_provider_tool_call_id("call_abc123"),
            )],
        );
        let param = to_claudius_message(&msg);

        assert!(matches!(param.role, MessageRole::Assistant));
        if let MessageParamContent::Array(blocks) = &param.content {
            // Should have both text and tool_use block
            assert_eq!(blocks.len(), 2);

            // First is text
            if let ContentBlock::Text(text_block) = &blocks[0] {
                assert_eq!(text_block.text, "I'll check the weather.");
            } else {
                panic!("Expected Text as first content block");
            }

            // Second is tool_use
            if let ContentBlock::ToolUse(tool_use) = &blocks[1] {
                assert_eq!(tool_use.id, "call_abc123");
                assert_eq!(tool_use.name, "get_weather");
            } else {
                panic!("Expected ToolUse content block");
            }
        } else {
            panic!("Expected Array content");
        }
    }

    #[test]
    fn converts_tool_message() {
        let msg = Message::tool("call_abc123", "get_weather", r#"{"temperature": 20}"#);
        let param = to_claudius_message(&msg);

        // Tools are sent as user role in Anthropic API
        assert!(matches!(param.role, MessageRole::User));
        if let MessageParamContent::Array(blocks) = &param.content {
            assert_eq!(blocks.len(), 1);
            if let ContentBlock::ToolResult(result) = &blocks[0] {
                assert_eq!(result.tool_use_id, "call_abc123");
            } else {
                panic!("Expected ToolResult content block");
            }
        } else {
            panic!("Expected Array content");
        }
    }

    #[test]
    fn converts_system_message() {
        let msg = Message::system("You are a helpful assistant.");
        let param = to_claudius_message(&msg);

        // System messages become user messages in Anthropic API
        assert!(matches!(param.role, MessageRole::User));
        if let MessageParamContent::String(text) = &param.content {
            assert_eq!(text, "You are a helpful assistant.");
        } else {
            panic!("Expected String content");
        }
    }

    #[test]
    fn converts_multiple_messages() {
        let messages = vec![
            Message::system("You are a helpful assistant."),
            Message::user("Hello!"),
            Message::assistant("Hi there!"),
        ];

        let params = to_claudius_messages(&messages);

        assert_eq!(params.len(), 3);
        // System becomes User in Anthropic API
        assert!(matches!(params[0].role, MessageRole::User));
        assert!(matches!(params[1].role, MessageRole::User));
        assert!(matches!(params[2].role, MessageRole::Assistant));
    }

    #[test]
    fn groups_parallel_tool_results_into_one_user_message() {
        let messages = vec![
            Message::tool_with_correlation(
                "invoke-1",
                ToolCallCorrelation::new("invoke-1")
                    .with_provider_tool_call_id("toolu_1")
                    .with_protocol(ToolProtocol::AnthropicClientTools),
                "get_weather",
                "sunny",
            ),
            Message::tool_with_correlation(
                "invoke-2",
                ToolCallCorrelation::new("invoke-2")
                    .with_provider_tool_call_id("toolu_2")
                    .with_protocol(ToolProtocol::AnthropicClientTools),
                "get_time",
                "noon",
            ),
        ];

        let params = to_claudius_messages(&messages);

        assert_eq!(params.len(), 1);
        assert!(matches!(params[0].role, MessageRole::User));
        if let MessageParamContent::Array(blocks) = &params[0].content {
            assert_eq!(blocks.len(), 2);
            if let ContentBlock::ToolResult(first) = &blocks[0] {
                assert_eq!(first.tool_use_id, "toolu_1");
            } else {
                panic!("Expected ToolResult content block");
            }
            if let ContentBlock::ToolResult(second) = &blocks[1] {
                assert_eq!(second.tool_use_id, "toolu_2");
            } else {
                panic!("Expected ToolResult content block");
            }
        } else {
            panic!("Expected Array content");
        }
    }
}
