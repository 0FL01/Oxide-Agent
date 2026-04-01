use crate::config::{
    GEMINI_AUDIO_TRANSCRIBE_PROMPT, GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE, GEMINI_CHAT_TEMPERATURE,
    GEMINI_IMAGE_TEMPERATURE,
};
use crate::llm::support::http::create_http_client_builder;
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use gemini_rust::{
    generation::{BlockReason, FinishReason, GenerationResponse},
    safety::{HarmBlockThreshold, HarmCategory, SafetySetting},
    ClientError as GeminiClientError, Gemini, GeminiBuilder, Message as GeminiMessage, Model, Part,
};
use reqwest::StatusCode;

#[derive(Default)]
struct ResponsePartsSummary {
    text_parts: Vec<String>,
    thought_count: usize,
    function_call_count: usize,
    function_response_count: usize,
    inline_data_count: usize,
    file_data_count: usize,
    executable_code_count: usize,
    code_execution_result_count: usize,
    finish_reasons: Vec<&'static str>,
}

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
        let mut summary = ResponsePartsSummary::default();

        for candidate in &response.candidates {
            if let Some(finish_reason) = candidate.finish_reason.as_ref() {
                summary
                    .finish_reasons
                    .push(Self::finish_reason_name(finish_reason));
            }

            if let Some(parts) = candidate.content.parts.as_ref() {
                for part in parts {
                    match part {
                        Part::Text { text, thought, .. } => {
                            if thought.unwrap_or(false) {
                                summary.thought_count += 1;
                            } else if !text.is_empty() {
                                summary.text_parts.push(text.clone());
                            }
                        }
                        Part::FunctionCall { .. } => summary.function_call_count += 1,
                        Part::FunctionResponse { .. } => summary.function_response_count += 1,
                        Part::InlineData { .. } => summary.inline_data_count += 1,
                        Part::FileData { .. } => summary.file_data_count += 1,
                        Part::ExecutableCode { .. } => summary.executable_code_count += 1,
                        Part::CodeExecutionResult { .. } => {
                            summary.code_execution_result_count += 1;
                        }
                    }
                }
            }
        }

        let text = summary.text_parts.join("\n");

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

        let response_details = Self::response_details(&summary);
        if let Some(finish_reason) = summary.finish_reasons.first() {
            return Err(LlmError::ApiError(format!(
                "Gemini returned no text output ({finish_reason}; {response_details})"
            )));
        }

        if !response_details.is_empty() {
            return Err(LlmError::ApiError(format!(
                "Gemini returned no text output ({response_details})"
            )));
        }

        Err(LlmError::ApiError("Empty response".to_string()))
    }

    fn response_details(summary: &ResponsePartsSummary) -> String {
        let mut details = Vec::new();

        if summary.thought_count > 0 {
            details.push(format!("thoughts={}", summary.thought_count));
        }
        if summary.function_call_count > 0 {
            details.push(format!("function_calls={}", summary.function_call_count));
        }
        if summary.function_response_count > 0 {
            details.push(format!(
                "function_responses={}",
                summary.function_response_count
            ));
        }
        if summary.inline_data_count > 0 {
            details.push(format!("inline_data={}", summary.inline_data_count));
        }
        if summary.file_data_count > 0 {
            details.push(format!("file_data={}", summary.file_data_count));
        }
        if summary.executable_code_count > 0 {
            details.push(format!("executable_code={}", summary.executable_code_count));
        }
        if summary.code_execution_result_count > 0 {
            details.push(format!(
                "code_execution_results={}",
                summary.code_execution_result_count
            ));
        }

        details.join(", ")
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

    fn finish_reason(response: &GenerationResponse) -> String {
        response
            .candidates
            .iter()
            .find_map(|candidate| candidate.finish_reason.as_ref())
            .map(Self::finish_reason_name)
            .map(|reason| reason.to_ascii_lowercase())
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn token_count(count: Option<i32>) -> Option<u32> {
        count.and_then(|value| u32::try_from(value).ok())
    }

    fn usage(response: &GenerationResponse) -> Option<TokenUsage> {
        let usage = response.usage_metadata.as_ref()?;

        Some(TokenUsage {
            prompt_tokens: Self::token_count(usage.prompt_token_count)?,
            completion_tokens: Self::token_count(usage.candidates_token_count)?,
            total_tokens: Self::token_count(usage.total_token_count)?,
        })
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

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            json_mode,
        } = request;

        if !tools.is_empty() {
            return Err(LlmError::Unknown(
                "Gemini tool calling is not implemented yet".to_string(),
            ));
        }

        if !json_mode {
            return Err(LlmError::Unknown(
                "Gemini structured chat requests require json_mode".to_string(),
            ));
        }

        let client = self.sdk_client(model_id)?;
        let response = client
            .generate_content()
            .with_system_prompt(system_prompt)
            .with_messages(Self::history_to_sdk_messages(messages))
            .with_temperature(GEMINI_CHAT_TEMPERATURE)
            .with_max_output_tokens(Self::max_output_tokens(max_tokens))
            .with_safety_settings(Self::safety_settings())
            .with_response_mime_type("application/json")
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Ok(ChatResponse {
            content: Some(Self::extract_text_response(&response)?),
            tool_calls: Vec::new(),
            finish_reason: Self::finish_reason(&response),
            reasoning_content: None,
            usage: Self::usage(&response),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::GeminiProvider;
    use crate::llm::LlmError;
    use crate::llm::TokenUsage;
    use gemini_rust::{
        generation::{FinishReason, UsageMetadata},
        BlockReason, Candidate, ClientError, Content, FunctionCall, GenerationResponse, Part,
        PromptFeedback,
    };
    use serde_json::json;

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

    #[test]
    fn extracts_only_non_thought_text_from_mixed_parts() {
        let response = GenerationResponse {
            candidates: vec![Candidate {
                content: Content {
                    parts: Some(vec![
                        Part::Text {
                            text: "visible answer".to_string(),
                            thought: None,
                            thought_signature: None,
                        },
                        Part::Text {
                            text: "hidden reasoning".to_string(),
                            thought: Some(true),
                            thought_signature: None,
                        },
                        Part::FunctionCall {
                            function_call: FunctionCall::with_id(
                                "lookup_weather",
                                json!({"city": "Paris"}),
                                "call_123",
                            ),
                            thought_signature: None,
                        },
                    ]),
                    role: None,
                },
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                index: Some(0),
            }],
            prompt_feedback: None,
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let text = GeminiProvider::extract_text_response(&response).unwrap();
        assert_eq!(text, "visible answer");
    }

    #[test]
    fn joins_text_across_candidates_and_parts() {
        let response = GenerationResponse {
            candidates: vec![
                Candidate {
                    content: Content {
                        parts: Some(vec![
                            Part::Text {
                                text: "first".to_string(),
                                thought: None,
                                thought_signature: None,
                            },
                            Part::Text {
                                text: "second".to_string(),
                                thought: None,
                                thought_signature: None,
                            },
                        ]),
                        role: None,
                    },
                    safety_ratings: None,
                    citation_metadata: None,
                    grounding_metadata: None,
                    finish_reason: Some(FinishReason::Stop),
                    index: Some(0),
                },
                Candidate {
                    content: Content {
                        parts: Some(vec![Part::Text {
                            text: "third".to_string(),
                            thought: None,
                            thought_signature: None,
                        }]),
                        role: None,
                    },
                    safety_ratings: None,
                    citation_metadata: None,
                    grounding_metadata: None,
                    finish_reason: Some(FinishReason::Stop),
                    index: Some(1),
                },
            ],
            prompt_feedback: None,
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let text = GeminiProvider::extract_text_response(&response).unwrap();
        assert_eq!(text, "first\nsecond\nthird");
    }

    #[test]
    fn surfaces_non_text_response_details() {
        let response = GenerationResponse {
            candidates: vec![Candidate {
                content: Content {
                    parts: Some(vec![
                        Part::Text {
                            text: "reasoning only".to_string(),
                            thought: Some(true),
                            thought_signature: None,
                        },
                        Part::FunctionCall {
                            function_call: FunctionCall::with_id(
                                "lookup_weather",
                                json!({"city": "Paris"}),
                                "call_123",
                            ),
                            thought_signature: None,
                        },
                    ]),
                    role: None,
                },
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::Stop),
                index: Some(0),
            }],
            prompt_feedback: None,
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let err = GeminiProvider::extract_text_response(&response).unwrap_err();
        assert!(matches!(
            err,
            LlmError::ApiError(message)
                if message.contains("STOP")
                    && message.contains("thoughts=1")
                    && message.contains("function_calls=1")
        ));
    }

    #[test]
    fn finish_reason_is_lowercased_for_chat_responses() {
        let response = GenerationResponse {
            candidates: vec![Candidate {
                content: Content::default(),
                safety_ratings: None,
                citation_metadata: None,
                grounding_metadata: None,
                finish_reason: Some(FinishReason::MaxTokens),
                index: Some(0),
            }],
            prompt_feedback: None,
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        assert_eq!(GeminiProvider::finish_reason(&response), "max_tokens");
    }

    #[test]
    fn maps_usage_metadata_to_token_usage() {
        let response = GenerationResponse {
            candidates: Vec::new(),
            prompt_feedback: None,
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: Some(12),
                candidates_token_count: Some(34),
                total_token_count: Some(46),
                thoughts_token_count: Some(3),
                prompt_tokens_details: None,
                cached_content_token_count: None,
                cache_tokens_details: None,
            }),
            model_version: None,
            response_id: None,
        };

        assert_eq!(
            GeminiProvider::usage(&response),
            Some(TokenUsage {
                prompt_tokens: 12,
                completion_tokens: 34,
                total_tokens: 46,
            })
        );
    }
}
