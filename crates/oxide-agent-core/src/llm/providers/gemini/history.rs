use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::Message;
use gemini_rust::{Content, Message as GeminiMessage, Part, Role};

use super::GeminiProvider;

impl GeminiProvider {
    pub(super) fn history_to_sdk_messages(history: &[Message]) -> Vec<GeminiMessage> {
        history
            .iter()
            .filter_map(Self::history_message_to_sdk_message)
            .collect()
    }

    fn history_message_to_sdk_message(message: &Message) -> Option<GeminiMessage> {
        match message.role.as_str() {
            "system" => None,
            "assistant" => Self::assistant_history_message(message),
            "tool" => Self::tool_history_message(message),
            "user" => Some(GeminiMessage::user(message.content.clone())),
            _ => Some(GeminiMessage::model(message.content.clone())),
        }
    }

    fn assistant_history_message(message: &Message) -> Option<GeminiMessage> {
        let Some(tool_calls) = &message.tool_calls else {
            return Some(GeminiMessage::model(message.content.clone()));
        };

        let mut parts = Vec::new();
        let text = message.content.trim();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                thought: None,
                thought_signature: None,
            });
        }

        for tool_call in tool_calls {
            let Some(encoded_tool_call) = CHAT_LIKE_TOOL_PROFILE
                .encode_tool_call(tool_call)
                .and_then(|call| call.into_chat_like())
            else {
                continue;
            };

            parts.push(Part::FunctionCall {
                function_call: Self::sdk_function_call(
                    encoded_tool_call.name,
                    &encoded_tool_call.arguments,
                    Some(encoded_tool_call.id),
                ),
                thought_signature: None,
            });
        }

        if parts.is_empty() {
            return None;
        }

        Some(GeminiMessage {
            content: Content {
                parts: Some(parts),
                role: Some(Role::Model),
            },
            role: Role::Model,
        })
    }

    fn tool_history_message(message: &Message) -> Option<GeminiMessage> {
        let encoded_tool_result = CHAT_LIKE_TOOL_PROFILE
            .encode_tool_result(message)
            .and_then(|result| result.into_chat_like())?;
        let name = encoded_tool_result.name?;

        Some(GeminiMessage {
            content: Content::function_response(Self::sdk_function_response(
                name,
                &encoded_tool_result.content,
                Some(encoded_tool_result.tool_call_id),
            ))
            .with_role(Role::User),
            role: Role::User,
        })
    }
}
