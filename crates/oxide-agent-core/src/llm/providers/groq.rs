use crate::config::GROQ_CHAT_TEMPERATURE;
use crate::llm::{openai_compat, LlmError, LlmProvider, Message};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;

/// LLM provider implementation for Groq
pub struct GroqProvider {
    client: Client<OpenAIConfig>,
}

impl GroqProvider {
    /// Create a new Groq provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base("https://api.groq.com/openai/v1");
        Self {
            client: Client::with_config(config),
        }
    }
}

#[async_trait]
impl LlmProvider for GroqProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            GROQ_CHAT_TEMPERATURE,
        )
        .await
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }
}
