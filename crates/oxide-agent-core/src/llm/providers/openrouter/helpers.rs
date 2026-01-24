use crate::llm::{Message, ToolDefinition};
use serde_json::json;

pub(super) fn prepare_structured_messages(
    system_prompt: &str,
    history: &[Message],
) -> Vec<serde_json::Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt
    })];

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                messages.push(json!({
                    "role": "system",
                    "content": msg.content
                }));
            }
            "assistant" => {
                let mut m = json!({
                    "role": "assistant",
                    "content": msg.content
                });

                if let Some(tool_calls) = &msg.tool_calls {
                    let api_tool_calls: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments
                                }
                            })
                        })
                        .collect();

                    if !api_tool_calls.is_empty() {
                        m["tool_calls"] = json!(api_tool_calls);
                    }
                }

                messages.push(m);
            }
            "tool" => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": msg.tool_call_id,
                    "content": msg.content
                }));
            }
            _ => {
                messages.push(json!({
                    "role": "user",
                    "content": msg.content
                }));
            }
        }
    }
    messages
}

pub(super) fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }
            })
        })
        .collect()
}
