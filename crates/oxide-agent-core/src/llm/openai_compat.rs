//! OpenAI-compatible provider utilities
//!
//! Shared implementation for providers using the async-openai client
//! (Groq, Mistral, Zai).

use super::common::{build_openai_messages, extract_openai_response};
use super::{LlmError, Message};
use async_openai::{config::OpenAIConfig, types::chat::CreateChatCompletionRequestArgs, Client};

/// Perform a chat completion using an OpenAI-compatible API
///
/// This is a shared implementation for Groq, Mistral, and Zai providers
/// which all use the same async-openai client with different base URLs.
pub async fn chat_completion(
    client: &Client<OpenAIConfig>,
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
) -> Result<String, LlmError> {
    let messages = build_openai_messages(system_prompt, history, user_message)?;

    let request = CreateChatCompletionRequestArgs::default()
        .model(model_id)
        .messages(messages)
        .max_tokens(max_tokens)
        .temperature(temperature)
        .build()
        .map_err(|e| LlmError::Unknown(e.to_string()))?;

    let response = client
        .chat()
        .create(request)
        .await
        .map_err(|e| LlmError::ApiError(e.to_string()))?;

    extract_openai_response(&response)
}
