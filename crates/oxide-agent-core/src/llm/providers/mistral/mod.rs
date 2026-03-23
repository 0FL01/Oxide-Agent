//! Mistral AI LLM provider
//!
//! Supports chat completion, tool calling, and audio transcription.
//! 
//! # Structure
//! - `types`: Constants and type definitions
//! - `client`: HTTP client creation utilities
//! - `messages`: Message preparation for API requests
//! - `parsing`: Response parsing utilities
//! - `chat`: Chat completion and tool calling
//! - `transcription`: Audio transcription (Stage 2)
//! - `image`: Image analysis (placeholder)
//! - `tests`: Unit tests

use crate::config::MISTRAL_CHAT_TEMPERATURE;
use crate::llm::{
    openai_compat, ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message,
};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;
use reqwest::Client as HttpClient;

pub mod chat;
pub mod client;
pub mod image;
pub mod messages;
pub mod parsing;
pub mod tests;
pub mod transcription;
pub mod types;

/// LLM provider implementation for Mistral AI
pub struct MistralProvider {
    client: Client<OpenAIConfig>,
    http_client: HttpClient,
    api_key: String,
}

impl MistralProvider {
    /// Create a new Mistral provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        let http_client = client::create_http_client();
        let openai_client = client::create_openai_client(&api_key);
        Self {
            client: openai_client,
            http_client,
            api_key,
        }
    }

    /// Create a new Mistral provider with a shared HTTP client
    ///
    /// This allows connection reuse across multiple providers,
    /// significantly reducing latency for sequential requests.
    #[must_use]
    pub fn new_with_client(api_key: String, http_client: HttpClient) -> Self {
        let openai_client = client::create_openai_client(&api_key);
        Self {
            client: openai_client,
            http_client,
            api_key,
        }
    }
}

#[async_trait]
impl LlmProvider for MistralProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        if chat::is_reasoning_model(model_id) {
            let body = chat::build_chat_completion_body(
                system_prompt,
                history,
                user_message,
                model_id,
                max_tokens,
            );
            let response = chat::send_chat_request(
                &self.http_client,
                &self.api_key,
                body,
            )
            .await?;
            return response
                .content
                .ok_or_else(|| LlmError::ApiError("Empty response".to_string()));
        }

        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            MISTRAL_CHAT_TEMPERATURE,
        )
        .await
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        transcription::transcribe_audio(
            &self.http_client,
            &self.api_key,
            audio_bytes,
            mime_type,
            model_id,
        )
        .await
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        image::analyze_image(image_bytes, text_prompt, system_prompt, model_id).await
    }

    /// Chat completion with tool calling support for agent mode
    ///
    /// # Errors
    ///
    /// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
    /// or `LlmError::JsonError` if parsing fails.
    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        chat::chat_with_tools(&self.http_client,
            &self.api_key,
            request,
        )
        .await
    }
}

// Re-exports for backward compatibility
pub use self::types::*;
