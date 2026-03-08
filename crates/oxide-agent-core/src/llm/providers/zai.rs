mod sdk;

use crate::llm::{
    ChatCompletionRequest, ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider,
};
use async_trait::async_trait;
use tracing::debug;

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
}

#[async_trait]
impl LlmProvider for ZaiProvider {
    async fn chat_completion(&self, request: ChatCompletionRequest) -> Result<String, LlmError> {
        debug!(
            "ZAI: Starting chat completion request (model: {}, max_tokens: {}, history_size: {})",
            request.model_id,
            request.max_tokens,
            request.history.len()
        );

        self.chat_completion_sdk(
            &request.system_prompt,
            &request.history,
            &request.user_message,
            &request.model_id,
            request.max_tokens,
        )
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
    async fn chat_with_tools(
        &self,
        request: ChatWithToolsRequest,
    ) -> Result<ChatResponse, LlmError> {
        debug!(
            "ZAI: *** CHAT_WITH_TOOLS ENTRY *** model={} tools_count={} history_size={} json_mode={}",
            request.model_id,
            request.tools.len(),
            request.messages.len(),
            request.json_mode
        );

        debug!(
            "ZAI: Starting tool-enabled chat completion (model: {}, tools: {}, history: {})",
            request.model_id,
            request.tools.len(),
            request.messages.len()
        );

        self.chat_with_tools_sdk(
            &request.system_prompt,
            &request.messages,
            &request.tools,
            &request.model_id,
            request.max_tokens,
        )
        .await
    }
}
