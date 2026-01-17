mod sdk;

use crate::llm::{ChatResponse, LlmError, LlmProvider, Message, ToolDefinition};
use async_trait::async_trait;
use tracing::debug;

/// LLM provider implementation for Zai (`ZeroAI`)
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
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        debug!(
            "ZAI: Starting chat completion request (model: {model_id}, max_tokens: {max_tokens}, history_size: {})",
            history.len()
        );

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
    async fn chat_with_tools(
        &self,
        system_prompt: &str,
        history: &[Message],
        tools: &[ToolDefinition],
        model_id: &str,
        max_tokens: u32,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        debug!(
            "ZAI: *** CHAT_WITH_TOOLS ENTRY *** model={model_id} tools_count={} history_size={} json_mode={}",
            tools.len(),
            history.len(),
            json_mode
        );

        debug!(
            "ZAI: Starting tool-enabled chat completion (model: {model_id}, tools: {}, history: {})",
            tools.len(),
            history.len()
        );

        self.chat_with_tools_sdk(system_prompt, history, tools, model_id, max_tokens)
            .await
    }
}
