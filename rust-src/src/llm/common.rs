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
use reqwest::StatusCode;

/// Build a list of chat messages for OpenAI-compatible APIs
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
pub fn extract_openai_response(
    response: &CreateChatCompletionResponse,
) -> Result<String, LlmError> {
    response
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
}

/// Create an LlmError from HTTP response status and body
#[allow(dead_code)]
pub fn handle_http_error(provider: &str, status: StatusCode, body: &str) -> LlmError {
    LlmError::ApiError(format!("{} API error: {} - {}", provider, status, body))
}

/// Extract text content from a JSON response using a path
///
/// # Arguments
/// * `response` - The JSON value to extract from
/// * `path` - Array of keys to traverse (e.g., `["choices", "0", "message", "content"]`)
///
/// # Example
/// ```ignore
/// let text = extract_json_content(&json, &["candidates", "0", "content", "parts", "0", "text"])?;
/// ```
#[allow(dead_code)]
pub fn extract_json_content(
    response: &serde_json::Value,
    path: &[&str],
) -> Result<String, LlmError> {
    let mut current = response;

    for key in path {
        current = if let Ok(index) = key.parse::<usize>() {
            &current[index]
        } else {
            &current[key]
        };
    }

    current
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| LlmError::ApiError(format!("Invalid response format: {:?}", response)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_json_content_simple() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Hello, world!"
                }
            }]
        });

        let result = extract_json_content(&response, &["choices", "0", "message", "content"]);
        assert_eq!(result.unwrap(), "Hello, world!");
    }

    #[test]
    fn test_extract_json_content_gemini_format() {
        let response = json!({
            "candidates": [{
                "content": {
                    "parts": [{
                        "text": "Gemini response"
                    }]
                }
            }]
        });

        let result = extract_json_content(
            &response,
            &["candidates", "0", "content", "parts", "0", "text"],
        );
        assert_eq!(result.unwrap(), "Gemini response");
    }

    #[test]
    fn test_extract_json_content_missing_path() {
        let response = json!({"foo": "bar"});
        let result = extract_json_content(&response, &["missing", "path"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_http_error() {
        let error = handle_http_error(
            "TestProvider",
            StatusCode::INTERNAL_SERVER_ERROR,
            "Server error",
        );
        assert!(error.to_string().contains("TestProvider"));
        assert!(error.to_string().contains("500"));
    }
}
