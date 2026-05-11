use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall};
use gemini_rust::{
    generation::{BlockReason, FinishReason, GenerationResponse},
    Part,
};

use super::GeminiProvider;

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

impl GeminiProvider {
    pub(super) fn extract_text_response(response: &GenerationResponse) -> Result<String, LlmError> {
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

    pub(super) fn finish_reason_name(reason: &FinishReason) -> &'static str {
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

    pub(super) fn block_reason_name(reason: &BlockReason) -> &'static str {
        match reason {
            BlockReason::BlockReasonUnspecified => "BLOCK_REASON_UNSPECIFIED",
            BlockReason::Safety => "SAFETY",
            BlockReason::Other => "OTHER",
            BlockReason::Blocklist => "BLOCKLIST",
            BlockReason::ProhibitedContent => "PROHIBITED_CONTENT",
            BlockReason::ImageSafety => "IMAGE_SAFETY",
        }
    }

    pub(super) fn finish_reason(response: &GenerationResponse) -> String {
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

    pub(super) fn usage(response: &GenerationResponse) -> Option<TokenUsage> {
        let usage = response.usage_metadata.as_ref()?;

        Some(TokenUsage {
            prompt_tokens: Self::token_count(usage.prompt_token_count)?,
            completion_tokens: Self::token_count(usage.candidates_token_count)?,
            total_tokens: Self::token_count(usage.total_token_count)?,
        })
    }

    pub(super) fn parse_chat_response(
        response: &GenerationResponse,
    ) -> Result<ChatResponse, LlmError> {
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
