use crate::llm::providers::protocol_profiles::{
    CHAT_LIKE_TOOL_ADAPTER, CHAT_LIKE_TOOL_RESULT_ENCODER,
};
use crate::llm::providers::tool_result_encoder::{ProviderToolResultEncoder, ToolResultEncoder};
use crate::llm::{LlmError, Message, ToolCall, ToolDefinition};
use serde_json::json;

const OPENROUTER_TOOL_RESULT_ENCODER: ProviderToolResultEncoder = CHAT_LIKE_TOOL_RESULT_ENCODER;

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
                                "id": CHAT_LIKE_TOOL_ADAPTER.assistant_tool_call_id(tc),
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
                if let Some(result) = OPENROUTER_TOOL_RESULT_ENCODER
                    .encode(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": result.tool_call_id,
                        "content": result.content
                    }));
                }
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

pub(super) fn parse_tool_calls(value: &serde_json::Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from OpenRouter".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for call in array {
        let Some(function) = call.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let arguments = function
            .get("arguments")
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| serde_json::to_string(value).ok())
            })
            .unwrap_or_default();
        let wire_id = call.get("id").and_then(|value| value.as_str());
        tool_calls.push(match wire_id {
            Some(wire_id) => CHAT_LIKE_TOOL_ADAPTER.inbound_provider_tool_call(
                wire_id,
                None,
                name.to_string(),
                arguments,
            ),
            None => {
                CHAT_LIKE_TOOL_ADAPTER.inbound_uncorrelated_tool_call(name.to_string(), arguments)
            }
        });
    }

    Ok(tool_calls)
}

#[cfg(test)]
mod tests {
    use super::{parse_tool_calls, prepare_structured_messages};
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction};
    use serde_json::json;

    #[test]
    fn prepare_structured_messages_preserves_tool_ids_for_assistant_and_tool_messages() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![ToolCall::new(
                    "invoke-openrouter-1",
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: r#"{"query":"oxide"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-openrouter-1")
                        .with_provider_tool_call_id("call-openrouter-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-openrouter-1",
                ToolCallCorrelation::new("invoke-openrouter-1")
                    .with_provider_tool_call_id("call-openrouter-1"),
                "search",
                "result",
            ),
        ];

        let messages = prepare_structured_messages("system", &history);

        assert_eq!(
            messages[1]["tool_calls"][0]["id"],
            json!("call-openrouter-1")
        );
        assert_eq!(messages[2]["tool_call_id"], json!("call-openrouter-1"));
    }

    #[test]
    fn parse_tool_calls_attaches_openrouter_wire_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-openrouter-2",
                "type": "function",
                "function": {
                    "name": "search",
                    "arguments": "{\"query\":\"oxide\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_ne!(tool_calls[0].invocation_id().as_str(), "call-openrouter-2");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call-openrouter-2");
    }
}
