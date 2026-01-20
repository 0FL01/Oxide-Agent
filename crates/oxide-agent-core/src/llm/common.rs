//! Common utilities for LLM providers
//!
//! Shared helper functions for building messages, handling errors,
//! and parsing responses across all LLM providers.

use super::{LlmError, Message};
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
    CreateChatCompletionResponse,
};

/// Build a list of chat messages for OpenAI-compatible APIs
///
/// # Errors
///
/// Returns `LlmError::Unknown` if message building fails.
pub fn build_openai_messages(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
) -> Result<Vec<ChatCompletionRequestMessage>, LlmError> {
    let mut messages = vec![ChatCompletionRequestSystemMessageArgs::default()
        .content(system_prompt)
        .build()
        .map_err(|e| LlmError::Unknown(e.to_string()))?
        .into()];

    for msg in history {
        let m = match msg.role.as_str() {
            "user" => ChatCompletionRequestUserMessageArgs::default()
                .content(msg.content.clone())
                .build()
                .map_err(|e| LlmError::Unknown(e.to_string()))?
                .into(),
            _ => ChatCompletionRequestAssistantMessageArgs::default()
                .content(msg.content.clone())
                .build()
                .map_err(|e| LlmError::Unknown(e.to_string()))?
                .into(),
        };
        messages.push(m);
    }

    messages.push(
        ChatCompletionRequestUserMessageArgs::default()
            .content(user_message)
            .build()
            .map_err(|e| LlmError::Unknown(e.to_string()))?
            .into(),
    );

    Ok(messages)
}

/// Extract text content from an OpenAI-compatible chat completion response
///
/// # Errors
///
/// Returns `LlmError::ApiError` if the response is empty.
pub fn extract_openai_response(
    response: &CreateChatCompletionResponse,
) -> Result<String, LlmError> {
    response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
}
