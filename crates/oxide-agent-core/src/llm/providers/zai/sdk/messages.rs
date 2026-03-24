use crate::llm::providers::protocol_profiles::{
    CHAT_LIKE_TOOL_CALL_ENCODER, CHAT_LIKE_TOOL_RESULT_ENCODER,
};
use crate::llm::providers::tool_call_encoder::{ProviderToolCallEncoder, ToolCallEncoder};
use crate::llm::providers::tool_result_encoder::{ProviderToolResultEncoder, ToolResultEncoder};
use crate::llm::{Message, ToolCall, ToolDefinition};
use serde_json::Value;
use zai_rs::model::chat_message_types::{
    FunctionParams, TextMessage, ToolCall as ZaiToolCall, VisionMessage, VisionRichContent,
};
use zai_rs::model::tools::{Function, Tools};

const ZAI_TOOL_CALL_ENCODER: ProviderToolCallEncoder = CHAT_LIKE_TOOL_CALL_ENCODER;
const ZAI_TOOL_RESULT_ENCODER: ProviderToolResultEncoder = CHAT_LIKE_TOOL_RESULT_ENCODER;

pub(super) fn convert_to_text_messages(
    system_prompt: &str,
    history: &[Message],
    user_message: Option<&str>,
) -> Vec<TextMessage> {
    let mut messages = Vec::new();
    messages.push(TextMessage::system(system_prompt));

    for msg in history {
        let sdk_msg = match msg.role.as_str() {
            "system" => TextMessage::system(msg.content.clone()),
            "assistant" => {
                let tool_calls = msg
                    .tool_calls
                    .as_deref()
                    .map(convert_assistant_tool_calls)
                    .unwrap_or_default();
                if tool_calls.is_empty() {
                    TextMessage::assistant(msg.content.clone())
                } else {
                    let content = if msg.content.trim().is_empty() {
                        None
                    } else {
                        Some(msg.content.clone())
                    };
                    TextMessage::assistant_with_tools(content, tool_calls)
                }
            }
            "tool" => ZAI_TOOL_RESULT_ENCODER
                .encode(msg)
                .and_then(|result| result.into_chat_like())
                .map(|result| TextMessage::tool_with_id(result.content, result.tool_call_id))
                .unwrap_or_else(|| TextMessage::tool(msg.content.clone())),
            "user" => TextMessage::user(msg.content.clone()),
            _ => TextMessage::user(msg.content.clone()),
        };
        messages.push(sdk_msg);
    }

    if let Some(user) = user_message.filter(|user| !user.trim().is_empty()) {
        messages.push(TextMessage::user(user));
    }

    messages
}

pub(super) fn convert_to_vision_messages(
    system_prompt: &str,
    history: &[Message],
    user_message: Option<&str>,
) -> Vec<VisionMessage> {
    let mut messages = Vec::new();
    messages.push(VisionMessage::system(system_prompt));

    for msg in history {
        let sdk_msg = match msg.role.as_str() {
            "system" => VisionMessage::system(msg.content.clone()),
            "assistant" => VisionMessage::assistant(msg.content.clone()),
            _ => VisionMessage::new_user().add_user(VisionRichContent::text(msg.content.clone())),
        };
        messages.push(sdk_msg);
    }

    if let Some(user) = user_message.filter(|user| !user.trim().is_empty()) {
        messages
            .push(VisionMessage::new_user().add_user(VisionRichContent::text(user.to_string())));
    }

    messages
}

pub(super) fn convert_tools(tools: &[ToolDefinition]) -> Vec<Tools> {
    tools
        .iter()
        .map(|tool| {
            let function = Function::new(
                tool.name.clone(),
                tool.description.clone(),
                tool.parameters.clone(),
            );
            Tools::Function { function }
        })
        .collect()
}

pub(super) fn extract_text_content(content: Option<Value>) -> Option<String> {
    let content = content?;
    match content {
        Value::String(text) => Some(text),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                match item {
                    Value::String(text) => parts.push(text),
                    Value::Object(map) => {
                        if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                            parts.push(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(""))
            }
        }
        Value::Object(map) => map
            .get("text")
            .and_then(|v| v.as_str())
            .map(|text| text.to_string()),
        _ => None,
    }
}

fn convert_assistant_tool_calls(tool_calls: &[ToolCall]) -> Vec<ZaiToolCall> {
    tool_calls
        .iter()
        .filter_map(|call| {
            ZAI_TOOL_CALL_ENCODER
                .encode(call)
                .and_then(|call| call.into_chat_like())
                .map(|call| {
                    let params = FunctionParams::new(call.name, call.arguments);
                    ZaiToolCall::new_function(call.id, params)
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::convert_to_text_messages;
    use crate::llm::Message;
    use serde_json::json;

    #[test]
    fn convert_to_text_messages_skips_empty_final_user_message() {
        let history = [Message::user("older request")];

        let messages = convert_to_text_messages("system prompt", &history, Some("   "));

        let serialized = serde_json::to_value(&messages).expect("serialize messages");
        assert_eq!(
            serialized,
            json!([
                {"role": "system", "content": "system prompt"},
                {"role": "user", "content": "older request"}
            ])
        );
    }

    #[test]
    fn convert_to_text_messages_appends_non_empty_final_user_message() {
        let history = [Message::assistant("older response")];

        let messages = convert_to_text_messages("system prompt", &history, Some("new request"));

        let serialized = serde_json::to_value(&messages).expect("serialize messages");
        assert_eq!(
            serialized,
            json!([
                {"role": "system", "content": "system prompt"},
                {"role": "assistant", "content": "older response"},
                {"role": "user", "content": "new request"}
            ])
        );
    }
}
