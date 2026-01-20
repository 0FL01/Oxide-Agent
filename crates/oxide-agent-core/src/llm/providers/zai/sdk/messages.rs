use crate::llm::{Message, ToolCall, ToolDefinition};
use serde_json::Value;
use zai_rs::model::chat_message_types::{
    FunctionParams, TextMessage, ToolCall as ZaiToolCall, VisionMessage, VisionRichContent,
};
use zai_rs::model::tools::{Function, Tools};

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
            "tool" => msg
                .tool_call_id
                .as_ref()
                .map(|id| TextMessage::tool_with_id(msg.content.clone(), id.clone()))
                .unwrap_or_else(|| TextMessage::tool(msg.content.clone())),
            "user" => TextMessage::user(msg.content.clone()),
            _ => TextMessage::user(msg.content.clone()),
        };
        messages.push(sdk_msg);
    }

    if let Some(user) = user_message {
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

    if let Some(user) = user_message {
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
        .map(|call| {
            let params =
                FunctionParams::new(call.function.name.clone(), call.function.arguments.clone());
            ZaiToolCall::new_function(call.id.clone(), params)
        })
        .collect()
}
