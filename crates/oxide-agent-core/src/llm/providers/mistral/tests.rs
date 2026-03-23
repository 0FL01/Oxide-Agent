//! Unit tests for Mistral provider

#[cfg(test)]
mod tests {
    use crate::config::{
        MISTRAL_CHAT_TEMPERATURE, MISTRAL_REASONING_TEMPERATURE, MISTRAL_TOOL_TEMPERATURE,
    };
    use crate::llm::providers::mistral::{
        chat::{build_chat_completion_body, build_tool_chat_body, is_reasoning_model},
        messages::prepare_structured_messages,
        parsing::parse_chat_response,
    };
    use crate::llm::{Message, ToolCall, ToolCallFunction, ToolDefinition};
    use serde_json::json;

    #[test]
    fn reasoning_model_chat_body_uses_reasoning_defaults() {
        let body = build_chat_completion_body("system", &[], "hello", "mistral-small-2603", 4096);

        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(body["temperature"], json!(MISTRAL_REASONING_TEMPERATURE));
    }

    #[test]
    fn regular_model_tool_body_keeps_existing_temperature() {
        let body = build_tool_chat_body("system", &[], &[], "mistral-large-latest", 4096);

        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["temperature"], json!(MISTRAL_TOOL_TEMPERATURE));
    }

    #[test]
    fn regular_model_chat_body_keeps_existing_temperature() {
        let body = build_chat_completion_body("system", &[], "hello", "mistral-large-latest", 4096);

        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["temperature"], json!(MISTRAL_CHAT_TEMPERATURE));
    }

    #[test]
    fn parses_reasoning_chunks_into_content_and_reasoning() {
        let response = json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": [
                                {
                                    "type": "text",
                                    "text": "step one"
                                },
                                {
                                    "type": "text",
                                    "text": "step two"
                                }
                            ]
                        },
                        {
                            "type": "text",
                            "text": "final answer"
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        });

        let parsed = parse_chat_response(response).expect("response parses");

        assert_eq!(parsed.content.as_deref(), Some("final answer"));
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("step one\n\nstep two")
        );
        assert_eq!(parsed.usage.expect("usage").total_tokens, 30);
    }

    #[test]
    fn prepare_structured_messages_formats_tool_message() {
        let history = vec![Message::tool(
            "call_abc123",
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages = prepare_structured_messages("You are helpful.", &history);

        let tool_msg = &messages[1];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["content"], json!("{\"temperature\": 20}"));
        assert_eq!(tool_msg["tool_call_id"], json!("call_abc123"));
        assert_eq!(tool_msg["name"], json!("get_weather"));
    }

    #[test]
    fn parses_tool_calls_from_response() {
        let response = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc123",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"location\":\"Moscow\"}"
                            }
                        },
                        {
                            "id": "call_def456",
                            "type": "function",
                            "function": {
                                "name": "get_time",
                                "arguments": "{}"
                            }
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 30,
                "total_tokens": 80
            }
        });

        let parsed = parse_chat_response(response).expect("response parses");

        assert!(parsed.content.is_none());
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.tool_calls.len(), 2);

        assert_eq!(parsed.tool_calls[0].id, "call_abc123");
        assert_eq!(parsed.tool_calls[0].function.name, "get_weather");
        assert_eq!(
            parsed.tool_calls[0].function.arguments,
            "{\"location\":\"Moscow\"}"
        );

        assert_eq!(parsed.tool_calls[1].id, "call_def456");
        assert_eq!(parsed.tool_calls[1].function.name, "get_time");
        assert_eq!(parsed.tool_calls[1].function.arguments, "{}");
    }

    #[test]
    fn parses_tool_calls_with_interleaved_content() {
        let response = json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "I'll check the weather for you.",
                    "tool_calls": [
                        {
                            "id": "call_xyz789",
                            "type": "function",
                            "function": {
                                "name": "get_weather",
                                "arguments": "{\"city\":\"London\"}"
                            }
                        }
                    ]
                }
            }],
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 15,
                "total_tokens": 35
            }
        });

        let parsed = parse_chat_response(response).expect("response parses");

        assert_eq!(
            parsed.content.as_deref(),
            Some("I'll check the weather for you.")
        );
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].id, "call_xyz789");
    }

    #[test]
    fn builds_tool_chat_body_with_tools_array() {
        let tools = vec![
            ToolDefinition {
                name: "get_weather".to_string(),
                description: "Get weather for a city".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" }
                    },
                    "required": ["city"]
                }),
            },
            ToolDefinition {
                name: "get_time".to_string(),
                description: "Get current time".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
        ];

        let body = build_tool_chat_body(
            "You are a helpful assistant.",
            &[],
            &tools,
            "mistral-large-latest",
            4096,
        );

        // Verify tools array is present
        let tools_array = body.get("tools").expect("tools array should be present");
        let tools_vec = tools_array.as_array().expect("tools should be an array");
        assert_eq!(tools_vec.len(), 2);

        // Verify first tool structure
        let first_tool = &tools_vec[0];
        assert_eq!(first_tool["type"], json!("function"));
        assert_eq!(first_tool["function"]["name"], json!("get_weather"));
        assert_eq!(
            first_tool["function"]["description"],
            json!("Get weather for a city")
        );

        // Verify tool_choice and parallel_tool_calls are set
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));

        // Verify response_format is NOT present
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn builds_tool_chat_body_without_tools() {
        let body = build_tool_chat_body(
            "You are a helpful assistant.",
            &[],
            &[],
            "mistral-large-latest",
            4096,
        );

        // Verify tools array is NOT present when empty
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn prepare_structured_messages_preserves_assistant_tool_calls() {
        let history = vec![Message::assistant_with_tools(
            "I'll get the weather.",
            vec![ToolCall {
                id: "call_xyz".to_string(),
                function: ToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"Paris\"}".to_string(),
                },
                is_recovered: false,
            }],
        )];
        let messages = prepare_structured_messages("You are helpful.", &history);

        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], json!("assistant"));
        assert_eq!(assistant_msg["content"], json!("I'll get the weather."));
        assert!(assistant_msg.get("tool_calls").is_some());

        let tool_calls = assistant_msg["tool_calls"]
            .as_array()
            .expect("tool_calls should be present in assistant message");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], json!("call_xyz"));
        assert_eq!(tool_calls[0]["function"]["name"], json!("get_weather"));
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            json!("{\"city\":\"Paris\"}")
        );
    }

    #[test]
    fn test_is_reasoning_model() {
        assert!(is_reasoning_model("mistral-small-2603"));
        assert!(is_reasoning_model("Mistral-Small-2603"));
        assert!(!is_reasoning_model("mistral-large-latest"));
        assert!(!is_reasoning_model("mistral-small-2409"));
    }
}
