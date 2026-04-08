use crate::config::{
    GEMINI_AUDIO_TRANSCRIBE_PROMPT, GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE, GEMINI_CHAT_TEMPERATURE,
    GEMINI_IMAGE_TEMPERATURE,
};
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use gemini_rust::{ContentBuilder, FunctionCallingMode, ThinkingConfig, ThinkingLevel, Tool};

use super::GeminiProvider;

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
        let response = Self::apply_model_defaults(client.generate_content(), model_id)
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
        let response = Self::apply_model_defaults(client.generate_content(), model_id)
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
        let image_mime_type = Self::infer_image_mime_type(&image_bytes);
        let client = self.sdk_client(model_id)?;
        let response = Self::apply_model_defaults(client.generate_content(), model_id)
            .with_system_prompt(system_prompt)
            .with_user_message(text_prompt)
            .with_inline_data(BASE64.encode(&image_bytes), image_mime_type)
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
        let response = Self::apply_model_defaults(client.generate_content(), model_id)
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
            temperature,
            json_mode,
        } = request;

        let client = self.sdk_client(model_id)?;
        let mut request_builder = Self::apply_model_defaults(client.generate_content(), model_id)
            .with_system_prompt(system_prompt)
            .with_messages(Self::history_to_sdk_messages(messages))
            .with_temperature(temperature.unwrap_or(GEMINI_CHAT_TEMPERATURE))
            .with_max_output_tokens(Self::max_output_tokens(max_tokens))
            .with_safety_settings(Self::safety_settings());

        if tools.is_empty() {
            if json_mode {
                request_builder = request_builder.with_response_mime_type("application/json");
            }
        } else {
            request_builder = request_builder
                .with_tool(Tool::with_functions(Self::function_declarations(tools)))
                .with_function_calling_mode(FunctionCallingMode::Any)
                .with_allowed_function_names(tools.iter().map(|tool| tool.name.clone()));
        }

        let response = request_builder
            .execute()
            .await
            .map_err(Self::map_sdk_error)?;

        Self::parse_chat_response(&response)
    }
}

impl GeminiProvider {
    pub(super) fn thinking_config_for_model(model_id: &str) -> Option<ThinkingConfig> {
        matches!(
            Self::normalized_model_id(model_id),
            "gemma-4-31b-it" | "gemini-3.1-flash-lite-preview"
        )
        .then(|| ThinkingConfig::new().with_thinking_level(ThinkingLevel::High))
    }

    fn apply_model_defaults(request_builder: ContentBuilder, model_id: &str) -> ContentBuilder {
        match Self::thinking_config_for_model(model_id) {
            Some(thinking_config) => request_builder.with_thinking_config(thinking_config),
            None => request_builder,
        }
    }

    fn normalized_model_id(model_id: &str) -> &str {
        model_id.strip_prefix("models/").unwrap_or(model_id)
    }
}
