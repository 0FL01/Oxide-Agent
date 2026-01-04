//! Input preprocessor for multimodal content
//!
//! Handles voice and image preprocessing using Gemini Flash
//! before passing to the agent for execution.

use crate::llm::LlmClient;
use anyhow::Result;
use std::sync::Arc;
use tracing::info;

/// Preprocessor for converting multimodal inputs to text
pub struct Preprocessor {
    llm_client: Arc<LlmClient>,
}

impl Preprocessor {
    /// Create a new preprocessor with the given LLM client
    #[must_use]
    pub const fn new(llm_client: Arc<LlmClient>) -> Self {
        Self { llm_client }
    }

    /// Transcribe voice audio to text using Gemini Flash
    ///
    /// Uses the existing transcription infrastructure with `OpenRouter` Gemini
    ///
    /// # Errors
    ///
    /// Returns an error if the transcription fails.
    pub async fn transcribe_voice(&self, audio_bytes: Vec<u8>, mime_type: &str) -> Result<String> {
        info!(
            "Transcribing voice message: {} bytes, mime: {mime_type}",
            audio_bytes.len()
        );

        // Use Gemini Flash for transcription (via OpenRouter)
        let transcription = self
            .llm_client
            .transcribe_audio(audio_bytes, mime_type, "OR Gemini 3 Flash")
            .await
            .map_err(|e| anyhow::anyhow!("Transcription failed: {e}"))?;

        info!("Transcription result: {} chars", transcription.len());
        Ok(transcription)
    }

    /// Describe an image for the agent context
    ///
    /// Generates a detailed description that the agent can use
    ///
    /// # Errors
    ///
    /// Returns an error if the image analysis fails.
    pub async fn describe_image(
        &self,
        image_bytes: Vec<u8>,
        user_context: Option<&str>,
    ) -> Result<String> {
        info!("Describing image: {} bytes", image_bytes.len());

        let prompt = user_context.map_or_else(
            || {
                "Опиши это изображение детально для AI-агента. \
                     Укажи все важные детали, текст, объекты и их расположение."
                    .to_string()
            },
            |ctx| {
                format!(
                    "Опиши это изображение детально для AI-агента, который будет выполнять задачу. \
                 Контекст пользователя: {ctx}"
                )
            },
        );

        let system_prompt = "Ты - визуальный анализатор для AI-агента. \
                            Твоя задача - создать подробное текстовое описание изображения, \
                            которое позволит агенту понять его содержание без доступа к самому изображению.";

        let description = self
            .llm_client
            .analyze_image(
                image_bytes,
                &prompt,
                system_prompt,
                "OR Gemini 3 Flash", // Use Gemini for multimodal
            )
            .await
            .map_err(|e| anyhow::anyhow!("Image analysis failed: {e}"))?;

        info!("Image description: {} chars", description.len());
        Ok(description)
    }

    /// Preprocess any input type and return text suitable for the agent
    ///
    /// # Errors
    ///
    /// Returns an error if transcription or image analysis fails.
    pub async fn preprocess_input(&self, input: AgentInput) -> Result<String> {
        match input {
            AgentInput::Text(text) => Ok(text),
            AgentInput::Voice { bytes, mime_type } => {
                self.transcribe_voice(bytes, &mime_type).await
            }
            AgentInput::Image { bytes, context } => {
                self.describe_image(bytes, context.as_deref()).await
            }
            AgentInput::ImageWithText { image_bytes, text } => {
                let description = self.describe_image(image_bytes, Some(&text)).await?;
                Ok(format!(
                    "Пользователь отправил изображение с текстом: \"{text}\"\n\nОписание изображения:\n{description}"
                ))
            }
        }
    }
}

/// Types of input the agent can receive
pub enum AgentInput {
    /// Plain text message
    Text(String),
    /// Voice message to be transcribed
    Voice { bytes: Vec<u8>, mime_type: String },
    /// Image to be described
    Image {
        bytes: Vec<u8>,
        context: Option<String>,
    },
    /// Image with accompanying text
    ImageWithText { image_bytes: Vec<u8>, text: String },
}
