use crate::config::{
    GEMINI_AUDIO_TRANSCRIBE_PROMPT, GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE, GEMINI_CHAT_TEMPERATURE,
    GEMINI_IMAGE_TEMPERATURE,
};
use crate::llm::support::http::create_http_client_builder;
use crate::llm::{LlmError, LlmProvider, Message};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use gemini_rust::{
    generation::{BlockReason, FinishReason, GenerationResponse},
    safety::{HarmBlockThreshold, HarmCategory, SafetySetting},
    ClientError as GeminiClientError, Gemini, GeminiBuilder, Message as GeminiMessage, Model,
};
use reqwest::StatusCode;

/// LLM provider implementation for Google Gemini.
pub struct GeminiProvider {
    api_key: String,
}

impl GeminiProvider {
    /// Create a new Gemini provider instance.
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    fn sdk_client(&self, model_id: &str) -> Result<Gemini, LlmError> {
        GeminiBuilder::new(self.api_key.clone())
            .with_model(Self::sdk_model(model_id))
            .with_http_client(create_http_client_builder())
            .build()
            .map_err(Self::map_sdk_error)
    }

    fn sdk_model(model_id: &str) -> Model {
        let normalized = if model_id.starts_with("models/") {
            model_id.to_string()
        } else {
            format!("models/{model_id}")
        };

        Model::Custom(normalized)
    }

    fn map_sdk_error(error: GeminiClientError) -> LlmError {
        match error {
            GeminiClientError::BadResponse { code, description } => {
                let message = description.unwrap_or_else(|| "Gemini request failed".to_string());
                if code == StatusCode::TOO_MANY_REQUESTS.as_u16() {
                    LlmError::RateLimit {
                        wait_secs: None,
                        message,
                    }
                } else {
                    LlmError::ApiError(format!("Gemini API error [{code}]: {message}"))
                }
            }
            GeminiClientError::PerformRequest { source, .. }
            | GeminiClientError::PerformRequestNew { source } => {
                LlmError::NetworkError(source.to_string())
            }
            GeminiClientError::Io { source } => LlmError::NetworkError(source.to_string()),
            GeminiClientError::Deserialize { source } => LlmError::JsonError(source.to_string()),
            GeminiClientError::DecodeResponse { source } => LlmError::JsonError(source.to_string()),
            GeminiClientError::InvalidApiKey { source } => {
                LlmError::ApiError(format!("Invalid Gemini API key: {source}"))
            }
            GeminiClientError::ConstructUrl { source, suffix } => LlmError::ApiError(format!(
                "Failed to construct Gemini URL for {suffix}: {source}"
            )),
            GeminiClientError::MissingResponseHeader { header } => {
                LlmError::ApiError(format!("Gemini response missing header: {header}"))
            }
            GeminiClientError::BadPart { source } => LlmError::NetworkError(source.to_string()),
            GeminiClientError::UrlParse { source } => {
                LlmError::ApiError(format!("Failed to parse Gemini URL: {source}"))
            }
            GeminiClientError::OperationTimeout { name } => {
                LlmError::NetworkError(format!("Gemini operation timed out: {name}"))
            }
            GeminiClientError::OperationFailed {
                name,
                code,
                message,
            } => LlmError::ApiError(format!(
                "Gemini operation failed ({name}, code {code}): {message}"
            )),
            GeminiClientError::InvalidResourceName { name } => {
                LlmError::ApiError(format!("Invalid Gemini resource name: {name}"))
            }
        }
    }

    fn safety_settings() -> Vec<SafetySetting> {
        vec![
            SafetySetting {
                category: HarmCategory::Harassment,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::HateSpeech,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::SexuallyExplicit,
                threshold: HarmBlockThreshold::BlockNone,
            },
            SafetySetting {
                category: HarmCategory::DangerousContent,
                threshold: HarmBlockThreshold::BlockNone,
            },
        ]
    }

    fn extract_text_response(response: &GenerationResponse) -> Result<String, LlmError> {
        let text = response
            .all_text()
            .into_iter()
            .filter_map(|(text, is_thought)| (!is_thought && !text.is_empty()).then_some(text))
            .collect::<Vec<_>>()
            .join("\n");

        if !text.is_empty() {
            return Ok(text);
        }

        if let Some(prompt_feedback) = &response.prompt_feedback {
            if let Some(block_reason) = &prompt_feedback.block_reason {
                return Err(LlmError::ApiError(format!(
                    "Gemini blocked prompt: {}",
                    Self::block_reason_name(block_reason)
                )));
            }
        }

        if let Some(finish_reason) = response
            .candidates
            .iter()
            .find_map(|candidate| candidate.finish_reason.as_ref())
        {
            return Err(LlmError::ApiError(format!(
                "Gemini returned no text output ({})",
                Self::finish_reason_name(finish_reason)
            )));
        }

        Err(LlmError::ApiError("Empty response".to_string()))
    }

    fn finish_reason_name(reason: &FinishReason) -> &'static str {
        match reason {
            FinishReason::Stop => "STOP",
            FinishReason::FinishReasonUnspecified => "FINISH_REASON_UNSPECIFIED",
            FinishReason::MaxTokens => "MAX_TOKENS",
            FinishReason::Safety => "SAFETY",
            FinishReason::Recitation => "RECITATION",
            FinishReason::Language => "LANGUAGE",
            FinishReason::Other => "OTHER",
            FinishReason::Blocklist => "BLOCKLIST",
            FinishReason::ProhibitedContent => "PROHIBITED_CONTENT",
            FinishReason::Spii => "SPII",
            FinishReason::MalformedFunctionCall => "MALFORMED_FUNCTION_CALL",
            FinishReason::ImageSafety => "IMAGE_SAFETY",
            FinishReason::UnexpectedToolCall => "UNEXPECTED_TOOL_CALL",
            FinishReason::TooManyToolCalls => "TOO_MANY_TOOL_CALLS",
        }
    }

    fn block_reason_name(reason: &BlockReason) -> &'static str {
        match reason {
            BlockReason::BlockReasonUnspecified => "BLOCK_REASON_UNSPECIFIED",
            BlockReason::Safety => "SAFETY",
            BlockReason::Other => "OTHER",
            BlockReason::Blocklist => "BLOCKLIST",
            BlockReason::ProhibitedContent => "PROHIBITED_CONTENT",
            BlockReason::ImageSafety => "IMAGE_SAFETY",
        }
    }

    fn history_to_sdk_messages(history: &[Message]) -> Vec<GeminiMessage> {
        history
            .iter()
            .filter(|msg| msg.role != "system")
            .map(|msg| {
                if msg.role == "user" {
                    GeminiMessage::user(msg.content.clone())
                } else {
                    GeminiMessage::model(msg.content.clone())
                }
            })
            .collect()
    }

    fn max_output_tokens(max_tokens: u32) -> i32 {
        i32::try_from(max_tokens).unwrap_or(i32::MAX)
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let client = self.sdk_client(model_id)?;
        let response = client
            .generate_content()
            .with_system_prompt(system_prompt)
            .with_messages(Self::history_to_sdk_messages(history))
            .with_user_message(user_message)
            .with_temperature(GEMINI_CHAT_TEMPERATURE)
            .with_max_output_tokens(Self::max_output_tokens(max_tokens))
            .with_safety_settings(Self::safety_settings())
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::extract_text_response(&response)
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        self.transcribe_audio_with_prompt(
            audio_bytes,
            mime_type,
            GEMINI_AUDIO_TRANSCRIBE_PROMPT,
            model_id,
        )
        .await
    }

    async fn transcribe_audio_with_prompt(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let client = self.sdk_client(model_id)?;
        let response = client
            .generate_content()
            .with_user_message(text_prompt)
            .with_inline_data(BASE64.encode(&audio_bytes), mime_type)
            .with_temperature(GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE)
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::extract_text_response(&response)
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let client = self.sdk_client(model_id)?;
        let response = client
            .generate_content()
            .with_system_prompt(system_prompt)
            .with_user_message(text_prompt)
            .with_inline_data(BASE64.encode(&image_bytes), "image/jpeg")
            .with_temperature(GEMINI_IMAGE_TEMPERATURE)
            .with_max_output_tokens(4000)
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::extract_text_response(&response)
    }

    async fn analyze_video(
        &self,
        video_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let client = self.sdk_client(model_id)?;
        let response = client
            .generate_content()
            .with_system_prompt(system_prompt)
            .with_user_message(text_prompt)
            .with_inline_data(BASE64.encode(&video_bytes), mime_type)
            .with_temperature(GEMINI_IMAGE_TEMPERATURE)
            .with_max_output_tokens(4000)
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::extract_text_response(&response)
    }
}

#[cfg(test)]
mod tests {
    use super::GeminiProvider;
    use crate::llm::LlmError;
    use gemini_rust::{
        generation::FinishReason, BlockReason, Candidate, ClientError, Content, GenerationResponse,
        PromptFeedback,
    };

    #[test]
    fn normalizes_sdk_model_ids() {
        assert_eq!(
            GeminiProvider::sdk_model("gemini-2.5-flash").as_str(),
            "models/gemini-2.5-flash"
        );
        assert_eq!(
            GeminiProvider::sdk_model("models/gemini-3-flash-preview").as_str(),
            "models/gemini-3-flash-preview"
        );
    }

    #[test]
    fn maps_sdk_rate_limits() {
        let mapped = GeminiProvider::map_sdk_error(ClientError::BadResponse {
            code: 429,
            description: Some("slow down".to_string()),
        });

        assert!(matches!(
            mapped,
            LlmError::RateLimit { wait_secs: None, message } if message == "slow down"
        ));
    }

    #[test]
    fn surfaces_blocked_prompt_when_no_text() {
        let response = GenerationResponse {
            candidates: vec![Candidate {
                content: Content::default(),
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::Safety),
                index: Some(0),
            }],
            prompt_feedback: Some(PromptFeedback {
                safety_ratings: Vec::new(),
                block_reason: Some(BlockReason::Safety),
            }),
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let err = GeminiProvider::extract_text_response(&response).unwrap_err();
        assert!(
            matches!(err, LlmError::ApiError(message) if message.contains("Gemini blocked prompt: SAFETY"))
        );
    }
}
