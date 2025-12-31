pub mod providers;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("JSON error: {0}")]
    JsonError(String),
    #[error("Missing client/API key: {0}")]
    MissingConfig(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError>;

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;
}

pub struct LlmClient {
    groq: Option<providers::GroqProvider>,
    mistral: Option<providers::MistralProvider>,
    gemini: Option<providers::GeminiProvider>,
    openrouter: Option<providers::OpenRouterProvider>,
}

impl LlmClient {
    pub fn new(settings: &crate::config::Settings) -> Self {
        Self {
            groq: settings.groq_api_key.as_ref().map(|k| providers::GroqProvider::new(k.clone())),
            mistral: settings.mistral_api_key.as_ref().map(|k| providers::MistralProvider::new(k.clone())),
            gemini: settings.gemini_api_key.as_ref().map(|k| providers::GeminiProvider::new(k.clone())),
            openrouter: settings.openrouter_api_key.as_ref().map(|k| {
                providers::OpenRouterProvider::new(
                    k.clone(),
                    settings.openrouter_site_url.clone(),
                    settings.openrouter_site_name.clone(),
                )
            }),
        }
    }

    fn get_provider(&self, provider_name: &str) -> Result<&dyn LlmProvider, LlmError> {
        match provider_name {
            "groq" => self.groq.as_ref().map(|p| p as &dyn LlmProvider),
            "mistral" => self.mistral.as_ref().map(|p| p as &dyn LlmProvider),
            "gemini" => self.gemini.as_ref().map(|p| p as &dyn LlmProvider),
            "openrouter" => self.openrouter.as_ref().map(|p| p as &dyn LlmProvider),
            _ => None,
        }
        .ok_or_else(|| LlmError::MissingConfig(provider_name.to_string()))
    }

    pub async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        use crate::config::MODELS;

        let model_info = MODELS.iter()
            .find(|(name, _)| *name == model_name)
            .map(|(_, info)| info)
            .ok_or_else(|| LlmError::Unknown(format!("Model {} not found", model_name)))?;

        let provider = self.get_provider(model_info.provider)?;

        // Special case for OpenRouter with retry/fallback as in Python
        if model_name == "OR Gemini 3 Flash" && model_info.provider == "openrouter" {
             return self.openrouter_chat_with_fallback(system_prompt, history, user_message).await;
        }

        provider.chat_completion(
            system_prompt,
            history,
            user_message,
            model_info.id,
            model_info.max_tokens,
        ).await
    }

    async fn openrouter_chat_with_fallback(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
    ) -> Result<String, LlmError> {
        let provider = self.openrouter.as_ref()
            .ok_or_else(|| LlmError::MissingConfig("openrouter".to_string()))?;

        // Primary model: google/gemini-3-flash-preview
        let primary_model = "google/gemini-3-flash-preview";
        let fallback_model = "google/gemini-2.5-flash";

        for attempt in 1..=3 {
            info!("OpenRouter: Attempting with {}, attempt {}/3", primary_model, attempt);
            match provider.chat_completion(system_prompt, history, user_message, primary_model, 64000).await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("OpenRouter: Error with {} on attempt {}: {}", primary_model, attempt, e);
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }

        info!("OpenRouter: All attempts with {} failed, switching to {}", primary_model, fallback_model);

        for attempt in 1..=5 {
            info!("OpenRouter: Attempting with {}, attempt {}/5", fallback_model, attempt);
            match provider.chat_completion(system_prompt, history, user_message, fallback_model, 64000).await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("OpenRouter: Error with {} on attempt {}: {}", fallback_model, attempt, e);
                    // Check if it's a retryable error
                    let err_str = e.to_string().to_lowercase();
                    if (err_str.contains("503") || err_str.contains("429") || err_str.contains("500") ||
                       err_str.contains("overloaded") || err_str.contains("unavailable") || err_str.contains("timeout"))
                       && attempt < 5 {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(LlmError::ApiError("All fallback attempts failed".to_string()))
    }

    pub async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        // As per Python logic, transcribe usually uses Gemini or OpenRouter
        // We'll use the provider associated with the model, or fallback if it's the "retry_with_model_fallback" case
        // In Python it's a decorator, here we just implement it.
        
        if model_name == "Gemini 2.5 Flash Lite" {
             return self.gemini_transcribe_with_fallback(audio_bytes, mime_type).await;
        }

        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(model_info.provider)?;
        provider.transcribe_audio(audio_bytes, mime_type, model_info.id).await
    }

    async fn gemini_transcribe_with_fallback(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<String, LlmError> {
        let provider = self.gemini.as_ref()
            .ok_or_else(|| LlmError::MissingConfig("gemini".to_string()))?;

        let primary_model = "gemini-flash-latest";
        let fallback_model = "gemini-2.5-flash";

        for attempt in 1..=3 {
            match provider.transcribe_audio(audio_bytes.clone(), mime_type, primary_model).await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("Gemini transcription error (primary {}): {}", primary_model, e);
                    if attempt < 3 { tokio::time::sleep(std::time::Duration::from_secs(2)).await; }
                }
            }
        }

        for attempt in 1..=5 {
            match provider.transcribe_audio(audio_bytes.clone(), mime_type, fallback_model).await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("Gemini transcription error (fallback {}): {}", fallback_model, e);
                    if attempt < 5 { tokio::time::sleep(std::time::Duration::from_secs(3)).await; }
                    else { return Err(e); }
                }
            }
        }
        Err(LlmError::ApiError("All transcription fallback attempts failed".to_string()))
    }

    pub async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(model_info.provider)?;
        provider.analyze_image(image_bytes, text_prompt, system_prompt, model_info.id).await
    }

    fn get_model_info(&self, model_name: &str) -> Result<&'static crate::config::ModelInfo, LlmError> {
        crate::config::MODELS.iter()
            .find(|(name, _)| *name == model_name)
            .map(|(_, info)| info)
            .ok_or_else(|| LlmError::Unknown(format!("Model {} not found", model_name)))
    }
}
