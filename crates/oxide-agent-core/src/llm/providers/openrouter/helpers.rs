#[cfg(test)]
use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;
#[cfg(test)]
use crate::llm::providers::chat_completions::request::{self as chat_request, ChatRequestOptions};
use crate::llm::providers::chat_completions::response as chat_response;
use crate::llm::{LlmError, ToolCall};
#[cfg(test)]
use crate::llm::{Message, ToolDefinition};

#[cfg(test)]
pub(super) fn prepare_structured_messages(
    system_prompt: &str,
    history: &[Message],
) -> Vec<serde_json::Value> {
    chat_request::prepare_messages(
        system_prompt,
        history,
        ChatRequestOptions::new(ChatCompletionsProfile::openrouter())
            .with_native_image_parts(false),
    )
}

#[cfg(test)]
pub(super) fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
    chat_request::prepare_tools_json(tools)
}

pub(super) fn parse_tool_calls(value: &serde_json::Value) -> Result<Vec<ToolCall>, LlmError> {
    chat_response::parse_tool_calls(value, ChatCompletionsProfile::openrouter())
}

#[cfg(test)]
mod tests {
    use super::{parse_tool_calls, prepare_structured_messages, prepare_tools_json};
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition};
    use serde_json::json;

    #[test]
    fn prepare_structured_messages_preserves_tool_ids_for_assistant_and_tool_messages() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![
                    ToolCall::new(
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
                    ),
                ],
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
    fn prepare_tools_json_uses_chat_completions_function_schema() {
        let tools = prepare_tools_json(&[ToolDefinition {
            name: "search".to_string(),
            description: "Search the web".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "query": { "type": "string" } }
            }),
        }]);

        assert_eq!(tools[0]["type"], json!("function"));
        assert_eq!(tools[0]["function"]["name"], json!("search"));
        assert_eq!(tools[0]["function"]["parameters"]["type"], json!("object"));
        assert!(tools[0].get("name").is_none());
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

    #[test]
    fn parse_tool_calls_handles_empty_id() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "",
                "type": "function",
                "function": {
                    "name": "search",
                    "arguments": "{\"query\":\"oxide\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        // Empty ID should be treated as uncorrelated
        assert_eq!(
            tool_calls[0].wire_tool_call_id(),
            tool_calls[0].invocation_id().as_str()
        );
    }
}
