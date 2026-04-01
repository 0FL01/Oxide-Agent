use crate::config::{
    GEMINI_AUDIO_TRANSCRIBE_PROMPT, GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE, GEMINI_CHAT_TEMPERATURE,
    GEMINI_IMAGE_TEMPERATURE,
};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::create_http_client_builder;
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use gemini_rust::{
    generation::{BlockReason, FinishReason, GenerationResponse},
    safety::{HarmBlockThreshold, HarmCategory, SafetySetting},
    ClientError as GeminiClientError, Content, FunctionCall as GeminiFunctionCall,
    FunctionCallingMode, FunctionDeclaration, FunctionResponse as GeminiFunctionResponse, Gemini,
    GeminiBuilder, Message as GeminiMessage, Model, Part, Role, Tool,
};
use reqwest::StatusCode;
use serde_json::json;
use serde_json::Value;

#[derive(Default)]
struct ResponsePartsSummary {
    text_parts: Vec<String>,
    thought_parts: Vec<String>,
    tool_calls: Vec<ToolCall>,
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
        let summary = Self::summarize_response_parts(response);

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

    fn summarize_response_parts(response: &GenerationResponse) -> ResponsePartsSummary {
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
                                if !text.is_empty() {
                                    summary.thought_parts.push(text.clone());
                                }
                            } else if !text.is_empty() {
                                summary.text_parts.push(text.clone());
                            }
                        }
                        Part::FunctionCall { function_call, .. } => {
                            summary.function_call_count += 1;
                            summary
                                .tool_calls
                                .push(Self::parse_tool_call(function_call));
                        }
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

        summary
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
            .filter_map(Self::history_message_to_sdk_message)
            .collect()
    }

    fn history_message_to_sdk_message(message: &Message) -> Option<GeminiMessage> {
        match message.role.as_str() {
            "system" => None,
            "assistant" => Self::assistant_history_message(message),
            "tool" => Self::tool_history_message(message),
            "user" => Some(GeminiMessage::user(message.content.clone())),
            _ => Some(GeminiMessage::model(message.content.clone())),
        }
    }

    fn assistant_history_message(message: &Message) -> Option<GeminiMessage> {
        let Some(tool_calls) = &message.tool_calls else {
            return Some(GeminiMessage::model(message.content.clone()));
        };

        let mut parts = Vec::new();
        let text = message.content.trim();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
                thought: None,
                thought_signature: None,
            });
        }

        for tool_call in tool_calls {
            let Some(encoded_tool_call) = CHAT_LIKE_TOOL_PROFILE
                .encode_tool_call(tool_call)
                .and_then(|call| call.into_chat_like())
            else {
                continue;
            };

            parts.push(Part::FunctionCall {
                function_call: Self::sdk_function_call(
                    encoded_tool_call.name,
                    &encoded_tool_call.arguments,
                    Some(encoded_tool_call.id),
                ),
                thought_signature: None,
            });
        }

        if parts.is_empty() {
            return None;
        }

        Some(GeminiMessage {
            content: Content {
                parts: Some(parts),
                role: Some(Role::Model),
            },
            role: Role::Model,
        })
    }

    fn tool_history_message(message: &Message) -> Option<GeminiMessage> {
        let encoded_tool_result = CHAT_LIKE_TOOL_PROFILE
            .encode_tool_result(message)
            .and_then(|result| result.into_chat_like())?;
        let name = encoded_tool_result.name?;

        Some(GeminiMessage {
            content: Content::function_response(Self::sdk_function_response(
                name,
                &encoded_tool_result.content,
                Some(encoded_tool_result.tool_call_id),
            ))
            .with_role(Role::User),
            role: Role::User,
        })
    }

    fn function_declarations(tools: &[ToolDefinition]) -> Vec<FunctionDeclaration> {
        tools
            .iter()
            .map(|tool| {
                FunctionDeclaration::new(tool.name.clone(), tool.description.clone(), None)
                    .with_parameters_schema(tool.parameters.clone())
            })
            .collect()
    }

    fn normalize_tool_arguments(value: &Value) -> String {
        match value {
            Value::Null => "{}".to_string(),
            Value::String(raw) => Self::normalize_tool_arguments_str(raw),
            other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        }
    }

    fn normalize_tool_arguments_str(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return "{}".to_string();
        }

        let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
            return trimmed.to_string();
        };

        match parsed {
            Value::String(inner) => match serde_json::from_str::<Value>(&inner) {
                Ok(inner_parsed) => serde_json::to_string(&inner_parsed).unwrap_or(inner),
                Err(_) => inner,
            },
            other => serde_json::to_string(&other).unwrap_or_else(|_| trimmed.to_string()),
        }
    }

    fn sdk_function_call(
        name: impl Into<String>,
        arguments: &str,
        provider_id: Option<String>,
    ) -> GeminiFunctionCall {
        let name = name.into();
        let args = Self::tool_arguments_value(arguments);

        match provider_id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => {
                GeminiFunctionCall::with_id(name, args, provider_id)
            }
            _ => GeminiFunctionCall::new(name, args),
        }
    }

    fn tool_arguments_value(arguments: &str) -> Value {
        match serde_json::from_str::<Value>(&Self::normalize_tool_arguments_str(arguments)) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => json!({ "input": other }),
            Err(_) => json!({ "input": arguments }),
        }
    }

    fn sdk_function_response(
        name: impl Into<String>,
        content: &str,
        provider_id: Option<String>,
    ) -> GeminiFunctionResponse {
        let name = name.into();
        let response = Self::tool_result_value(content);

        match provider_id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => {
                GeminiFunctionResponse::with_id(name, response, provider_id)
            }
            _ => GeminiFunctionResponse::new(name, response),
        }
    }

    fn tool_result_value(content: &str) -> Value {
        match serde_json::from_str::<Value>(content) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(other) => json!({ "output": other }),
            Err(_) => json!({ "output": content }),
        }
    }

    fn parse_tool_call(function_call: &GeminiFunctionCall) -> ToolCall {
        let arguments = Self::normalize_tool_arguments(&function_call.args);
        match function_call.id.as_deref().map(str::trim) {
            Some(provider_id) if !provider_id.is_empty() => CHAT_LIKE_TOOL_PROFILE
                .inbound_provider_tool_call(
                    provider_id,
                    None,
                    function_call.name.clone(),
                    arguments,
                ),
            _ => CHAT_LIKE_TOOL_PROFILE
                .inbound_uncorrelated_tool_call(function_call.name.clone(), arguments),
        }
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

    fn parse_chat_response(response: &GenerationResponse) -> Result<ChatResponse, LlmError> {
        if let Some(prompt_feedback) = &response.prompt_feedback {
            if let Some(block_reason) = &prompt_feedback.block_reason {
                return Err(LlmError::ApiError(format!(
                    "Gemini blocked prompt: {}",
                    Self::block_reason_name(block_reason)
                )));
            }
        }

        let summary = Self::summarize_response_parts(response);
        let content = (!summary.text_parts.is_empty()).then(|| summary.text_parts.join("\n"));
        let reasoning_content =
            (!summary.thought_parts.is_empty()).then(|| summary.thought_parts.join("\n"));

        if content.is_none() && reasoning_content.is_none() && summary.tool_calls.is_empty() {
            let response_details = Self::response_details(&summary);
            if let Some(finish_reason) = summary.finish_reasons.first() {
                return Err(LlmError::ApiError(format!(
                    "Gemini returned empty chat response ({finish_reason}; {response_details})"
                )));
            }

            if !response_details.is_empty() {
                return Err(LlmError::ApiError(format!(
                    "Gemini returned empty chat response ({response_details})"
                )));
            }

            return Err(LlmError::ApiError("Empty response".to_string()));
        }

        Ok(ChatResponse {
            content,
            tool_calls: summary.tool_calls,
            finish_reason: Self::finish_reason(response),
            reasoning_content,
            usage: Self::usage(response),
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

        let client = self.sdk_client(model_id)?;
        let mut request_builder = client
            .generate_content()
            .with_system_prompt(system_prompt)
            .with_messages(Self::history_to_sdk_messages(messages))
            .with_temperature(GEMINI_CHAT_TEMPERATURE)
            .with_max_output_tokens(Self::max_output_tokens(max_tokens))
            .with_safety_settings(Self::safety_settings());

        if tools.is_empty() {
            if !json_mode {
                return Err(LlmError::Unknown(
                    "Gemini structured chat requests require json_mode".to_string(),
                ));
            }

            request_builder = request_builder.with_response_mime_type("application/json");
        } else {
            request_builder = request_builder
                .with_tool(Tool::with_functions(Self::function_declarations(tools)))
                .with_function_calling_mode(FunctionCallingMode::Auto)
                .with_allowed_function_names(tools.iter().map(|tool| tool.name.clone()));
        }

        let response = request_builder
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::parse_chat_response(&response)
    }
}

#[cfg(test)]
mod tests {
    use super::GeminiProvider;
    use crate::llm::TokenUsage;
    use crate::llm::{
        LlmError, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition,
    };
    use gemini_rust::{
        generation::{FinishReason, UsageMetadata},
        BlockReason, Candidate, ClientError, Content, FunctionCall, GenerationResponse, Part,
        PromptFeedback, Role,
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

    #[test]
    fn builds_function_declarations_from_tool_definitions() {
        let declarations = GeminiProvider::function_declarations(&[ToolDefinition {
            name: "lookup_weather".to_string(),
            description: "Look up weather by city".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }),
        }]);

        let serialized = serde_json::to_value(&declarations[0]).expect("serialize declaration");
        assert_eq!(serialized["name"], json!("lookup_weather"));
        assert_eq!(serialized["description"], json!("Look up weather by city"));
        assert_eq!(serialized["parameters"]["required"], json!(["city"]));
    }

    #[test]
    fn parses_tool_calls_into_chat_response() {
        let response = GenerationResponse {
            candidates: vec![Candidate {
                content: Content {
                    parts: Some(vec![
                        Part::Text {
                            text: "thinking".to_string(),
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

        let parsed = GeminiProvider::parse_chat_response(&response).expect("chat response parse");

        assert!(parsed.content.is_none());
        assert_eq!(parsed.reasoning_content.as_deref(), Some("thinking"));
        assert_eq!(parsed.finish_reason, "stop");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].function.name, "lookup_weather");
        assert_eq!(
            parsed.tool_calls[0].function.arguments,
            r#"{"city":"Paris"}"#
        );
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_123");
    }

    #[test]
    fn tool_calls_without_provider_ids_become_uncorrelated() {
        let tool_call = GeminiProvider::parse_tool_call(&FunctionCall::new(
            "lookup_weather",
            json!({"city": "Paris"}),
        ));

        assert_eq!(tool_call.function.arguments, r#"{"city":"Paris"}"#);
        assert_eq!(
            tool_call.wire_tool_call_id(),
            tool_call.invocation_id().as_str()
        );
    }

    #[test]
    fn replays_assistant_tool_calls_with_provider_ids() {
        let history = vec![Message::assistant_with_tools(
            "Calling weather tool",
            vec![ToolCall::new(
                "invoke-1",
                ToolCallFunction {
                    name: "lookup_weather".to_string(),
                    arguments: r#"{"city":"Paris"}"#.to_string(),
                },
                false,
            )
            .with_correlation(
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("call_123"),
            )],
        )];

        let messages = GeminiProvider::history_to_sdk_messages(&history);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::Model);

        let parts = messages[0].content.parts.as_ref().expect("assistant parts");
        assert!(matches!(&parts[0], Part::Text { text, .. } if text == "Calling weather tool"));
        assert!(matches!(
            &parts[1],
            Part::FunctionCall { function_call, .. }
                if function_call.name == "lookup_weather"
                    && function_call.id.as_deref() == Some("call_123")
                    && function_call.args == json!({"city": "Paris"})
        ));
    }

    #[test]
    fn replays_tool_results_as_user_function_responses_with_same_provider_id() {
        let history = vec![Message::tool_with_correlation(
            "invoke-1",
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("call_123"),
            "lookup_weather",
            r#"{"temperature":22,"condition":"sunny"}"#,
        )];

        let messages = GeminiProvider::history_to_sdk_messages(&history);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::User);

        let parts = messages[0]
            .content
            .parts
            .as_ref()
            .expect("tool result parts");
        assert!(matches!(
            &parts[0],
            Part::FunctionResponse { function_response }
                if function_response.name == "lookup_weather"
                    && function_response.id.as_deref() == Some("call_123")
                    && function_response.response.as_ref() == Some(&json!({"temperature":22,"condition":"sunny"}))
        ));
    }

    #[test]
    fn wraps_plain_text_tool_results_into_json_object() {
        assert_eq!(
            GeminiProvider::tool_result_value("done"),
            json!({ "output": "done" })
        );
        assert_eq!(
            GeminiProvider::tool_result_value("[1,2,3]"),
            json!({ "output": [1, 2, 3] })
        );
    }
}
