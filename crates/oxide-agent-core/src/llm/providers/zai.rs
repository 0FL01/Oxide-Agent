mod sdk;

pub use sdk::parse_zai_flush_time;

use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;

/// LLM provider implementation for Zai (Zhipu AI)
pub struct ZaiProvider {
    api_key: String,
    api_base: String,
}

impl ZaiProvider {
    /// Create a new Zai provider instance
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self { api_key, api_base }
    }

    /// Create a new Zai provider with a shared HTTP client
    ///
    /// Note: Zai uses the zai_rs SDK which manages its own HTTP connections,
    /// so the provided http_client is not currently used. This method exists
    /// for API consistency with other providers.
    #[must_use]
    pub fn new_with_client(
        api_key: String,
        api_base: String,
        _http_client: reqwest::Client,
    ) -> Self {
        // Note: zai_rs SDK doesn't support external HTTP client
        Self::new(api_key, api_base)
    }
}

#[async_trait]
impl LlmProvider for ZaiProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        self.chat_completion_sdk(system_prompt, history, user_message, model_id, max_tokens)
            .await
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("ZAI_FALLBACK_TO_GEMINI".to_string()))
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        self.analyze_image_sdk(image_bytes, text_prompt, system_prompt, model_id)
            .await
    }

    /// Chat completion with tool calling support for agent mode.
    /// Supports streaming tool calls.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
    /// or `LlmError::JsonError` if parsing fails.
    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages: history,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode: _json_mode,
        } = request;
        self.chat_with_tools_sdk(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            temperature,
        )
        .await
    }
}
