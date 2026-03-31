use super::{ChatResponse, ChatWithToolsRequest, LlmError, Message};

/// Interface for all LLM providers
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a chat completion
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError>;

    /// Transcribe audio content
    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;

    /// Transcribe audio content with an optional task-specific prompt.
    ///
    /// Default implementation falls back to plain transcription and ignores the prompt.
    async fn transcribe_audio_with_prompt(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let _ = text_prompt;
        self.transcribe_audio(audio_bytes, mime_type, model_id)
            .await
    }

    /// Analyze an image
    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;

    /// Analyze a video clip
    ///
    /// Default implementation returns an error indicating video analysis is not supported.
    async fn analyze_video(
        &self,
        _video_bytes: Vec<u8>,
        _mime_type: &str,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Video analysis not supported by this provider".to_string(),
        ))
    }

    /// Chat completion with tool calling support (optional, not all providers support it)
    ///
    /// Default implementation returns an error indicating tool calling is not supported.
    /// Providers that support tool calling (e.g., Mistral, ZAI) should override this method.
    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Unknown(
            "Tool calling not supported by this provider".to_string(),
        ))
    }
}
