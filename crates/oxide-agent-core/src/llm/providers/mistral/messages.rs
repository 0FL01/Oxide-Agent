//! Message preparation utilities for Mistral API

use crate::llm::providers::mistral::id_mapper::ToolCallIdMapper;
use crate::llm::providers::tool_call_adapter::ProviderToolCallAdapter;
use crate::llm::providers::tool_result_encoder::{ProviderToolResultEncoder, ToolResultEncoder};
use crate::llm::{Message, ToolProtocol, ToolTransport};
use serde_json::{json, Value};

const MISTRAL_TOOL_ADAPTER: ProviderToolCallAdapter =
    ProviderToolCallAdapter::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);
const MISTRAL_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    ProviderToolResultEncoder::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

/// Prepare structured messages for tool calling
///
/// Mistral requires system role before any tool/user/assistant after tool.
/// Tool call IDs are transformed to Mistral-compatible format (9 alphanumeric chars).
pub fn prepare_structured_messages(
    system_prompt: &str,
    history: &[Message],
    id_mapper: &mut ToolCallIdMapper,
) -> Vec<Value> {
    // Collect all system messages from history to prepend them
    let mut history_systems = Vec::new();
    let mut other_messages = Vec::new();

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                history_systems.push(json!({
                    "role": "system",
                    "content": msg.content
                }));
            }
            "assistant" => {
                let content = msg.content.clone();
                let tool_calls = msg.tool_calls.as_ref();

                let mut msg_obj = json!({
                    "role": "assistant",
                    "content": content
                });

                if let Some(calls) = tool_calls {
                    if !calls.is_empty() {
                        let mistral_tool_calls: Vec<Value> = calls
                            .iter()
                            .map(|tc| {
                                // Transform ID to Mistral-compatible format
                                let mistral_id = id_mapper
                                    .to_mistral(&MISTRAL_TOOL_ADAPTER.assistant_tool_call_id(tc));
                                json!({
                                    "id": mistral_id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.function.name,
                                        "arguments": tc.function.arguments
                                    }
                                })
                            })
                            .collect();
                        msg_obj["tool_calls"] = json!(mistral_tool_calls);
                    }
                }
                other_messages.push(msg_obj);
            }
            "tool" => {
                if let Some(result) = MISTRAL_TOOL_RESULT_ENCODER
                    .encode(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    let mut tool_msg = json!({
                        "role": "tool",
                        "content": result.content
                    });
                    // Transform ID to Mistral-compatible format
                    let mistral_id = id_mapper.to_mistral(&result.tool_call_id);
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
                    "content": msg.content
                }));
            }
        }
    }

    // Build final message list: all systems first, then main system, then others
    let mut messages = Vec::new();
    messages.extend(history_systems);
    messages.push(json!({
        "role": "system",
        "content": system_prompt
    }));
    messages.extend(other_messages);

    messages
}

/// Legacy function without ID mapping (for backward compatibility where mapping isn't needed)
///
/// ⚠️ Warning: This should only be used when tool calling is not involved,
/// as Mistral requires 9-character alphanumeric tool call IDs.
pub fn prepare_structured_messages_legacy(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let mut dummy_mapper = ToolCallIdMapper::new();
    prepare_structured_messages(system_prompt, history, &mut dummy_mapper)
}

/// Prepare simple chat messages (no tool calling)
pub fn prepare_chat_messages(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];

    for msg in history {
        match msg.role.as_str() {
            "system" => messages.push(json!({
                "role": "system",
                "content": msg.content
            })),
            "assistant" => messages.push(json!({
                "role": "assistant",
                "content": msg.content
            })),
            "tool" => messages.push(json!({
                "role": "user",
                "content": format!("[Tool Output] {}", msg.content)
            })),
            _ => messages.push(json!({
                "role": "user",
                "content": msg.content
            })),
        }
    }

    messages.push(json!({
        "role": "user",
        "content": user_message
    }));

    messages
}
