//! Anthropic-compatible Messages request body construction and conversion.

use crate::llm::providers::protocol_profiles::ANTHROPIC_CLIENT_TOOL_PROFILE;
use crate::llm::{Message, ToolDefinition};
use serde_json::{Value, json};

use super::MessagesProfile;

/// Build extra HTTP headers for an Anthropic Messages API request.
///
/// Returns `(anthropic-version, x-api-key)` header pairs.
#[allow(dead_code)]
pub(crate) fn anthropic_extra_headers(api_key: &str) -> Vec<(&'static str, &str)> {
    MessagesProfile::anthropic().extra_headers(api_key)
}

/// Build a non-streaming Anthropic Messages request body for text completion.
pub(crate) fn build_completion_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
    thinking: Option<Value>,
) -> Value {
    let mut messages = prepare_messages(history);
    messages.push(text_message("user", user_message));
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": false,
    });
    if let Some(system) = build_system_prompt(system_prompt, history) {
        body["system"] = json!(system);
    }
    if let Some(thinking) = thinking {
        body["thinking"] = thinking;
    }
    body
}

/// Build a non-streaming Anthropic Messages request body for chat with tools.
pub(crate) fn build_messages_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
    thinking: Option<Value>,
) -> Value {
    let messages = prepare_messages(history);
    let anthropic_tools = prepare_tools_json(tools);
    let has_tools = !anthropic_tools.is_empty();

    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": false,
    });

    if let Some(system) = build_system_prompt(system_prompt, history) {
        body["system"] = json!(system);
    }
    if has_tools {
        body["tools"] = json!(anthropic_tools);
        body["tool_choice"] = json!({ "type": "auto" });
    }
    if let Some(thinking) = thinking {
        body["thinking"] = thinking;
    }

    body
}

/// Build a top-level `system` prompt from the prompt string and any `system` role
/// messages in history.
pub(crate) fn build_system_prompt(system_prompt: &str, history: &[Message]) -> Option<String> {
    let mut parts = Vec::new();
    if !system_prompt.trim().is_empty() {
        parts.push(system_prompt.trim().to_string());
    }
    parts.extend(
        history
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.trim())
            .filter(|content| !content.is_empty())
            .map(ToString::to_string),
    );
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Convert an internal `Message` history into the Anthropic Messages `messages` array.
///
/// Skips `system` role messages, groups consecutive `tool` role messages into a
/// single `user` message with `tool_result` content blocks, and converts `assistant`
/// messages to use `text` + `tool_use` content blocks.
pub(crate) fn prepare_messages(history: &[Message]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len());
    let mut index = 0;
    while index < history.len() {
        let message = &history[index];
        if message.role == "system" {
            index += 1;
            continue;
        }
        if message.role == "tool" {
            let mut blocks = Vec::new();
            let mut cursor = index;
            while cursor < history.len() && history[cursor].role == "tool" {
                if let Some(block) = tool_result_block(&history[cursor]) {
                    blocks.push(block);
                }
                cursor += 1;
            }
            if !blocks.is_empty() {
                messages.push(json!({
                    "role": "user",
                    "content": blocks,
                }));
            }
            index = cursor;
            continue;
        }

        messages.push(match message.role.as_str() {
            "assistant" => assistant_message(message),
            _ => text_message("user", &message.content),
        });
        index += 1;
    }
    messages
}

/// Build a text content message.
pub(crate) fn text_message(role: &str, text: &str) -> Value {
    json!({
        "role": role,
        "content": [{
            "type": "text",
            "text": text,
        }],
    })
}

/// Build an assistant message with `text` + `tool_use` content blocks.
pub(crate) fn assistant_message(message: &Message) -> Value {
    let mut blocks = Vec::new();
    if !message.content.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }
    if let Some(tool_calls) = &message.tool_calls {
        blocks.extend(tool_calls.iter().filter_map(|tool_call| {
            ANTHROPIC_CLIENT_TOOL_PROFILE
                .encode_tool_call(tool_call)
                .and_then(|call| call.into_anthropic())
                .map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.input,
                    })
                })
        }));
    }
    if blocks.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": "",
        }));
    }
    json!({
        "role": "assistant",
        "content": blocks,
    })
}

/// Build a `tool_result` content block for a tool response message.
pub(crate) fn tool_result_block(message: &Message) -> Option<Value> {
    ANTHROPIC_CLIENT_TOOL_PROFILE
        .encode_tool_result(message)
        .and_then(|result| result.into_anthropic())
        .map(|result| {
            let mut block = json!({
                "type": "tool_result",
                "tool_use_id": result.tool_use_id,
                "content": result.content,
            });
            if let Some(is_error) = result.is_error {
                block["is_error"] = json!(is_error);
            }
            block
        })
}

/// Convert `ToolDefinition` slice to Anthropic tool schema format.
pub(crate) fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            })
        })
        .collect()
}

/// Map an Anthropic `stop_reason` to a normalized finish reason string.
pub(crate) fn map_stop_reason(stop_reason: &str) -> String {
    match stop_reason {
        "end_turn" => "stop".to_string(),
        "tool_use" => "tool_calls".to_string(),
        "stop_sequence" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCall, ToolCallCorrelation, ToolCallFunction};
    use serde_json::json;

    #[test]
    fn prepare_messages_groups_consecutive_tool_results() {
        let history = vec![
            Message::tool_with_correlation(
                "invoke-a",
                ToolCallCorrelation::new("invoke-a").with_provider_tool_call_id("toolu-a"),
                "read_file",
                "a",
            ),
            Message::tool_with_correlation(
                "invoke-b",
                ToolCallCorrelation::new("invoke-b").with_provider_tool_call_id("toolu-b"),
                "read_file",
                "b",
            ),
        ];

        let messages = prepare_messages(&history);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"].as_array().expect("blocks").len(), 2);
        assert_eq!(messages[0]["content"][0]["tool_use_id"], json!("toolu-a"));
        assert_eq!(messages[0]["content"][1]["tool_use_id"], json!("toolu-b"));
    }

    #[test]
    fn build_messages_body_uses_anthropic_wire_shape() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![
                    ToolCall::new(
                        "invoke-1",
                        ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                        },
                        false,
                    )
                    .with_correlation(
                        ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("toolu-1"),
                    ),
                ],
            ),
            Message::tool_with_correlation(
                "invoke-1",
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("toolu-1"),
                "read_file",
                "contents",
            ),
        ];
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
        }];

        let body =
            build_messages_body("system", &history, &tools, "minimax-m2.7", 32000, 0.2, None);

        assert_eq!(body["model"], json!("minimax-m2.7"));
        assert_eq!(body["system"], json!("system"));
        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
        assert_eq!(body["messages"][0]["content"][1]["id"], json!("toolu-1"));
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("tool_result")
        );
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        assert_eq!(
            body["tools"][0]["input_schema"]["properties"]["path"]["type"],
            json!("string")
        );
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
    }

    #[test]
    fn build_messages_body_folds_history_system_into_top_level_system() {
        let history = vec![
            Message {
                role: "system".to_string(),
                content: "History system".to_string(),
                ..Message::user("")
            },
            Message::user("hello"),
        ];

        let body = build_messages_body(
            "Main system",
            &history,
            &[],
            "minimax-m2.7",
            32000,
            0.2,
            None,
        );

        assert_eq!(body["system"], json!("Main system\n\nHistory system"));
        assert_eq!(body["messages"].as_array().expect("messages").len(), 1);
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"][0]["text"], json!("hello"));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn build_messages_body_includes_thinking_when_provided() {
        let body = build_completion_body(
            "system",
            &[],
            "hello",
            "deepseek-v4-flash",
            32000,
            1.0,
            Some(json!({ "type": "enabled" })),
        );
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
    }

    #[test]
    fn build_messages_body_omits_thinking_when_none() {
        let body = build_completion_body("system", &[], "hello", "gpt-4o", 32000, 1.0, None);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn map_stop_reason_converts_anthropic_reasons() {
        assert_eq!(map_stop_reason("end_turn"), "stop");
        assert_eq!(map_stop_reason("tool_use"), "tool_calls");
        assert_eq!(map_stop_reason("stop_sequence"), "stop");
        assert_eq!(map_stop_reason("max_tokens"), "length");
        assert_eq!(map_stop_reason("custom_reason"), "custom_reason");
    }

    #[test]
    fn prepare_tools_json_uses_input_schema() {
        let tools = prepare_tools_json(&[ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({ "type": "object" }),
        }]);

        assert_eq!(tools[0]["name"], json!("read_file"));
        assert_eq!(tools[0]["input_schema"]["type"], json!("object"));
        assert!(tools[0].get("function").is_none());
    }
}
