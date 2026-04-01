use super::GeminiProvider;
use crate::llm::TokenUsage;
use crate::llm::{
    LlmError, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition,
};
use gemini_rust::{
    generation::{FinishReason, UsageMetadata},
    BlockReason, Candidate, ClientError, Content, FunctionCall, GenerationResponse, Part,
    PromptFeedback, Role,
};
use serde_json::json;

#[test]
fn normalizes_sdk_model_ids() {
    assert_eq!(
        GeminiProvider::sdk_model("gemini-2.5-flash").as_str(),
        "models/gemini-2.5-flash"
    );
    assert_eq!(
        GeminiProvider::sdk_model("models/gemini-3-flash-preview").as_str(),
        "models/gemini-3-flash-preview"
    );
}

#[test]
fn maps_sdk_rate_limits() {
    let mapped = GeminiProvider::map_sdk_error(ClientError::BadResponse {
        code: 429,
        description: Some("slow down".to_string()),
    });

    assert!(matches!(
        mapped,
        LlmError::RateLimit { wait_secs: None, message } if message == "slow down"
    ));
}

#[test]
fn maps_generic_sdk_api_errors() {
    let mapped = GeminiProvider::map_sdk_error(ClientError::BadResponse {
        code: 500,
        description: Some("internal failure".to_string()),
    });

    assert!(matches!(
        mapped,
        LlmError::ApiError(message)
            if message.contains("Gemini API error [500]")
                && message.contains("internal failure")
    ));
}

#[test]
fn safety_settings_disable_expected_categories() {
    let settings = GeminiProvider::safety_settings();

    assert_eq!(settings.len(), 4);
    assert!(settings.iter().any(|setting| {
        setting.category == gemini_rust::HarmCategory::Harassment
            && matches!(
                setting.threshold,
                gemini_rust::HarmBlockThreshold::BlockNone
            )
    }));
    assert!(settings.iter().any(|setting| {
        setting.category == gemini_rust::HarmCategory::HateSpeech
            && matches!(
                setting.threshold,
                gemini_rust::HarmBlockThreshold::BlockNone
            )
    }));
    assert!(settings.iter().any(|setting| {
        setting.category == gemini_rust::HarmCategory::SexuallyExplicit
            && matches!(
                setting.threshold,
                gemini_rust::HarmBlockThreshold::BlockNone
            )
    }));
    assert!(settings.iter().any(|setting| {
        setting.category == gemini_rust::HarmCategory::DangerousContent
            && matches!(
                setting.threshold,
                gemini_rust::HarmBlockThreshold::BlockNone
            )
    }));
}

#[test]
fn surfaces_blocked_prompt_when_no_text() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content::default(),
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Safety),
            index: Some(0),
        }],
        prompt_feedback: Some(PromptFeedback {
            safety_ratings: Vec::new(),
            block_reason: Some(BlockReason::Safety),
        }),
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let err = GeminiProvider::extract_text_response(&response).unwrap_err();
    assert!(
        matches!(err, LlmError::ApiError(message) if message.contains("Gemini blocked prompt: SAFETY"))
    );
}

#[test]
fn extracts_only_non_thought_text_from_mixed_parts() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![
                    Part::Text {
                        text: "visible answer".to_string(),
                        thought: None,
                        thought_signature: None,
                    },
                    Part::Text {
                        text: "hidden reasoning".to_string(),
                        thought: Some(true),
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Paris"}),
                            "call_123",
                        ),
                        thought_signature: None,
                    },
                ]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let text = GeminiProvider::extract_text_response(&response).unwrap();
    assert_eq!(text, "visible answer");
}

#[test]
fn joins_text_across_candidates_and_parts() {
    let response = GenerationResponse {
        candidates: vec![
            Candidate {
                content: Content {
                    parts: Some(vec![
                        Part::Text {
                            text: "first".to_string(),
                            thought: None,
                            thought_signature: None,
                        },
                        Part::Text {
                            text: "second".to_string(),
                            thought: None,
                            thought_signature: None,
                        },
                    ]),
                    role: None,
                },
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                index: Some(0),
            },
            Candidate {
                content: Content {
                    parts: Some(vec![Part::Text {
                        text: "third".to_string(),
                        thought: None,
                        thought_signature: None,
                    }]),
                    role: None,
                },
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                index: Some(1),
            },
        ],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let text = GeminiProvider::extract_text_response(&response).unwrap();
    assert_eq!(text, "first\nsecond\nthird");
}

#[test]
fn surfaces_non_text_response_details() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![
                    Part::Text {
                        text: "reasoning only".to_string(),
                        thought: Some(true),
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Paris"}),
                            "call_123",
                        ),
                        thought_signature: None,
                    },
                ]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let err = GeminiProvider::extract_text_response(&response).unwrap_err();
    assert!(matches!(
        err,
        LlmError::ApiError(message)
            if message.contains("STOP")
                && message.contains("thoughts=1")
                && message.contains("function_calls=1")
    ));
}

#[test]
fn finish_reason_is_lowercased_for_chat_responses() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content::default(),
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::MaxTokens),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    assert_eq!(GeminiProvider::finish_reason(&response), "max_tokens");
}

#[test]
fn maps_usage_metadata_to_token_usage() {
    let response = GenerationResponse {
        candidates: Vec::new(),
        prompt_feedback: None,
        usage_metadata: Some(UsageMetadata {
            prompt_token_count: Some(12),
            candidates_token_count: Some(34),
            total_token_count: Some(46),
            thoughts_token_count: Some(3),
            prompt_tokens_details: None,
            cached_content_token_count: None,
            cache_tokens_details: None,
        }),
        model_version: None,
        response_id: None,
    };

    assert_eq!(
        GeminiProvider::usage(&response),
        Some(TokenUsage {
            prompt_tokens: 12,
            completion_tokens: 34,
            total_tokens: 46,
        })
    );
}

#[test]
fn builds_function_declarations_from_tool_definitions() {
    let declarations = GeminiProvider::function_declarations(&[ToolDefinition {
        name: "lookup_weather".to_string(),
        description: "Look up weather by city".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "city": { "type": "string" }
            },
            "required": ["city"]
        }),
    }]);

    let serialized = serde_json::to_value(&declarations[0]).expect("serialize declaration");
    assert_eq!(serialized["name"], json!("lookup_weather"));
    assert_eq!(serialized["description"], json!("Look up weather by city"));
    assert_eq!(serialized["parameters"]["required"], json!(["city"]));
}

#[test]
fn parses_tool_calls_into_chat_response() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![
                    Part::Text {
                        text: "thinking".to_string(),
                        thought: Some(true),
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Paris"}),
                            "call_123",
                        ),
                        thought_signature: None,
                    },
                ]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let parsed = GeminiProvider::parse_chat_response(&response).expect("chat response parse");

    assert!(parsed.content.is_none());
    assert_eq!(parsed.reasoning_content.as_deref(), Some("thinking"));
    assert_eq!(parsed.finish_reason, "stop");
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].function.name, "lookup_weather");
    assert_eq!(
        parsed.tool_calls[0].function.arguments,
        r#"{"city":"Paris"}"#
    );
    assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_123");
}

#[test]
fn parse_chat_response_surfaces_blocked_prompt_before_other_content() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![Part::Text {
                    text: "hidden by prompt block".to_string(),
                    thought: None,
                    thought_signature: None,
                }]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Safety),
            index: Some(0),
        }],
        prompt_feedback: Some(PromptFeedback {
            safety_ratings: Vec::new(),
            block_reason: Some(BlockReason::Safety),
        }),
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let err = GeminiProvider::parse_chat_response(&response).unwrap_err();
    assert!(matches!(
        err,
        LlmError::ApiError(message) if message.contains("Gemini blocked prompt: SAFETY")
    ));
}

#[test]
fn preserves_visible_text_alongside_tool_calls_in_chat_response() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![
                    Part::Text {
                        text: "Calling weather tool".to_string(),
                        thought: None,
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Paris"}),
                            "call_123",
                        ),
                        thought_signature: None,
                    },
                ]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let parsed = GeminiProvider::parse_chat_response(&response).expect("chat response parse");

    assert_eq!(parsed.content.as_deref(), Some("Calling weather tool"));
    assert_eq!(parsed.tool_calls.len(), 1);
    assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_123");
}

#[test]
fn parses_multiple_same_name_tool_calls_with_distinct_provider_ids() {
    let response = GenerationResponse {
        candidates: vec![Candidate {
            content: Content {
                parts: Some(vec![
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Paris"}),
                            "call_paris",
                        ),
                        thought_signature: None,
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall::with_id(
                            "lookup_weather",
                            json!({"city": "Berlin"}),
                            "call_berlin",
                        ),
                        thought_signature: None,
                    },
                ]),
                role: None,
            },
            safety_ratings: None,
            citation_metadata: None,
            grounding_metadata: None,
            finish_reason: Some(FinishReason::Stop),
            index: Some(0),
        }],
        prompt_feedback: None,
        usage_metadata: None,
        model_version: None,
        response_id: None,
    };

    let parsed = GeminiProvider::parse_chat_response(&response).expect("chat response parse");

    assert_eq!(parsed.tool_calls.len(), 2);
    assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_paris");
    assert_eq!(
        parsed.tool_calls[0].function.arguments,
        r#"{"city":"Paris"}"#
    );
    assert_eq!(parsed.tool_calls[1].wire_tool_call_id(), "call_berlin");
    assert_eq!(
        parsed.tool_calls[1].function.arguments,
        r#"{"city":"Berlin"}"#
    );
}

#[test]
fn tool_calls_without_provider_ids_become_uncorrelated() {
    let tool_call = GeminiProvider::parse_tool_call(&FunctionCall::new(
        "lookup_weather",
        json!({"city": "Paris"}),
    ));

    assert_eq!(tool_call.function.arguments, r#"{"city":"Paris"}"#);
    assert_eq!(
        tool_call.wire_tool_call_id(),
        tool_call.invocation_id().as_str()
    );
}

#[test]
fn unwraps_double_encoded_tool_argument_json_strings() {
    let tool_call = GeminiProvider::parse_tool_call(&FunctionCall::new(
        "lookup_weather",
        json!("{\"city\":\"Paris\"}"),
    ));

    assert_eq!(tool_call.function.arguments, r#"{"city":"Paris"}"#);
}

#[test]
fn replays_assistant_tool_calls_with_provider_ids() {
    let history = vec![Message::assistant_with_tools(
        "Calling weather tool",
        vec![ToolCall::new(
            "invoke-1",
            ToolCallFunction {
                name: "lookup_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("call_123"),
        )],
    )];

    let messages = GeminiProvider::history_to_sdk_messages(&history);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::Model);

    let parts = messages[0].content.parts.as_ref().expect("assistant parts");
    assert!(matches!(&parts[0], Part::Text { text, .. } if text == "Calling weather tool"));
    assert!(matches!(
        &parts[1],
        Part::FunctionCall { function_call, .. }
            if function_call.name == "lookup_weather"
                && function_call.id.as_deref() == Some("call_123")
                && function_call.args == json!({"city": "Paris"})
    ));
}

#[test]
fn replays_tool_results_as_user_function_responses_with_same_provider_id() {
    let history = vec![Message::tool_with_correlation(
        "invoke-1",
        ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("call_123"),
        "lookup_weather",
        r#"{"temperature":22,"condition":"sunny"}"#,
    )];

    let messages = GeminiProvider::history_to_sdk_messages(&history);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::User);

    let parts = messages[0]
        .content
        .parts
        .as_ref()
        .expect("tool result parts");
    assert!(matches!(
        &parts[0],
        Part::FunctionResponse { function_response }
            if function_response.name == "lookup_weather"
                && function_response.id.as_deref() == Some("call_123")
                && function_response.response.as_ref() == Some(&json!({"temperature":22,"condition":"sunny"}))
    ));
}

#[test]
fn replays_legacy_tool_results_with_invocation_id_as_function_response_id() {
    let history = vec![Message::tool(
        "legacy-call-1",
        "lookup_weather",
        r#"{"temperature":22}"#,
    )];

    let messages = GeminiProvider::history_to_sdk_messages(&history);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::User);

    let parts = messages[0]
        .content
        .parts
        .as_ref()
        .expect("tool result parts");
    assert!(matches!(
        &parts[0],
        Part::FunctionResponse { function_response }
            if function_response.name == "lookup_weather"
                && function_response.id.as_deref() == Some("legacy-call-1")
                && function_response.response.as_ref() == Some(&json!({"temperature":22}))
    ));
}

#[test]
fn keeps_plain_assistant_history_as_model_text_message() {
    let history = vec![Message::assistant("plain assistant reply")];

    let messages = GeminiProvider::history_to_sdk_messages(&history);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::Model);
    assert!(matches!(
        messages[0].content.parts.as_ref().and_then(|parts| parts.first()),
        Some(Part::Text { text, .. }) if text == "plain assistant reply"
    ));
}

#[test]
fn wraps_plain_text_tool_results_into_json_object() {
    assert_eq!(
        GeminiProvider::tool_result_value("done"),
        json!({ "output": "done" })
    );
    assert_eq!(
        GeminiProvider::tool_result_value("[1,2,3]"),
        json!({ "output": [1, 2, 3] })
    );
}

#[test]
fn infers_image_mime_type_from_magic_bytes() {
    let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0x00];
    let jpeg = [0xFF, 0xD8, 0xFF, 0xDB];
    let gif = *b"GIF89a";
    let webp = [b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];
    let unknown = [0x00, 0x11, 0x22, 0x33];

    assert_eq!(GeminiProvider::infer_image_mime_type(&png), "image/png");
    assert_eq!(GeminiProvider::infer_image_mime_type(&jpeg), "image/jpeg");
    assert_eq!(GeminiProvider::infer_image_mime_type(&gif), "image/gif");
    assert_eq!(GeminiProvider::infer_image_mime_type(&webp), "image/webp");
    assert_eq!(
        GeminiProvider::infer_image_mime_type(&unknown),
        "image/jpeg"
    );
}
