use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info, instrument, trace, warn};

use super::providers;
use super::{
    capabilities, support, ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message,
    ProviderCapabilities, ToolDefinition,
};

/// Unified client for interacting with multiple LLM providers
pub struct LlmClient {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    #[cfg(feature = "llm-opencode-go")]
    opencode_go_model_catalog:
        Option<Arc<providers::opencode_go::discovery::OpenCodeGoModelCatalog>>,
    #[cfg(feature = "llm-opencode-go")]
    opencode_zen_model_catalog:
        Option<Arc<providers::opencode_go::discovery::OpenCodeGoModelCatalog>>,
    /// Available models configured from settings
    pub models: Vec<(String, crate::config::ModelInfo)>,
    /// Optional explicit media model name for multimodal requests.
    pub media_model_name: Option<String>,
    /// Optional media model ID for audio/image/video requests.
    pub media_model_id: Option<String>,
    /// Optional media model provider for audio/image/video requests.
    pub media_model_provider: Option<String>,
}

/// Provider-discovered model metadata exposed without leaking provider-specific internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLlmModel {
    /// Provider identifier, such as `opencode-go`.
    pub provider_id: String,
    /// Raw provider model ID without a provider prefix.
    pub model_id: String,
    /// Application-qualified model ID, such as `opencode-go/kimi-k2.6`.
    pub qualified_id: String,
    /// User-facing display name.
    pub display_name: String,
    /// Provider protocol label for routing diagnostics.
    pub protocol: String,
    /// Discovery source label: `network`, `cache`, or `fallback`.
    pub source: String,
    /// Timestamp associated with the discovered model list.
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(feature = "llm-opencode-go")]
impl From<providers::opencode_go::discovery::DiscoveredOpenCodeGoModel> for DiscoveredLlmModel {
    fn from(model: providers::opencode_go::discovery::DiscoveredOpenCodeGoModel) -> Self {
        Self {
            provider_id: model.provider_id,
            model_id: model.model_id,
            qualified_id: model.qualified_id,
            display_name: model.display_name,
            protocol: model.protocol.as_str().to_string(),
            source: match model.source {
                providers::opencode_go::discovery::DiscoverySource::Network => "network",
                providers::opencode_go::discovery::DiscoverySource::Cache => "cache",
                providers::opencode_go::discovery::DiscoverySource::Fallback => "fallback",
            }
            .to_string(),
            fetched_at: model.fetched_at,
        }
    }
}

/// Internal plain-text completion use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InternalTextPurpose {
    CompactionSummary,
    LoopDetection,
    WikiMemoryWriter,
    InputIntentClassification,
}

impl LlmClient {
    fn provider_key(name: &str) -> String {
        providers::provider_key(name)
    }

    fn resolve_media_route_for_modality(
        &self,
        modality: capabilities::MediaModality,
    ) -> Result<(String, crate::config::ModelInfo), LlmError> {
        let Some(model_name) = self
            .media_model_name
            .as_deref()
            .filter(|name| !name.is_empty())
        else {
            return Err(LlmError::MissingConfig(format!(
                "MEDIA_MODEL is not configured for {}",
                modality.label()
            )));
        };

        let model_info = self.get_model_info(model_name)?;

        if !self.is_provider_available(&model_info.provider) {
            return Err(LlmError::MissingConfig(format!(
                "MEDIA_MODEL provider '{}' is not available; check the API key and compiled profile",
                model_info.provider
            )));
        }

        if capabilities::provider_media_capabilities_for_model(&model_info).supports(modality) {
            return Ok((model_name.to_string(), model_info));
        }

        Err(LlmError::MissingConfig(format!(
            "MEDIA_MODEL {}/{} is not allowed for {} by media policy",
            model_info.provider,
            model_info.id,
            modality.label()
        )))
    }

    /// Resolve the model route for audio transcription.
    ///
    /// Requires explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER`.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports audio transcription.
    pub fn resolve_media_model_for_audio_stt(&self) -> Result<crate::config::ModelInfo, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::AudioTranscription)
            .map(|(_, info)| info)
    }

    /// Resolve the model route for image understanding.
    ///
    /// Requires explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER`.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports image understanding.
    pub fn resolve_media_model_for_image(&self) -> Result<crate::config::ModelInfo, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::ImageUnderstanding)
            .map(|(_, info)| info)
    }

    /// Resolve the model route for video understanding.
    ///
    /// Requires explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER`.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports video understanding.
    pub fn resolve_media_model_for_video(&self) -> Result<crate::config::ModelInfo, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::VideoUnderstanding)
            .map(|(_, info)| info)
    }

    /// Resolve the configured model name for audio transcription.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports audio transcription.
    pub fn resolve_media_model_name_for_audio_stt(&self) -> Result<String, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::AudioTranscription)
            .map(|(name, _)| name)
    }

    /// Resolve the configured model name for image understanding.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports image understanding.
    pub fn resolve_media_model_name_for_image(&self) -> Result<String, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::ImageUnderstanding)
            .map(|(name, _)| name)
    }

    /// Resolve the configured model name for video understanding.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` when no route supports video understanding.
    pub fn resolve_media_model_name_for_video(&self) -> Result<String, LlmError> {
        self.resolve_media_route_for_modality(capabilities::MediaModality::VideoUnderstanding)
            .map(|(name, _)| name)
    }

    /// Returns true when at least one configured route supports audio transcription.
    #[must_use]
    pub fn is_audio_transcription_available(&self) -> bool {
        self.resolve_media_model_for_audio_stt().is_ok()
    }

    /// Returns true when at least one configured route supports image understanding.
    #[must_use]
    pub fn is_image_understanding_available(&self) -> bool {
        self.resolve_media_model_for_image().is_ok()
    }

    /// Returns true when at least one configured route supports video understanding.
    #[must_use]
    pub fn is_video_understanding_available(&self) -> bool {
        self.resolve_media_model_for_video().is_ok()
    }

    /// Create a new LLM client with providers configured from settings
    #[must_use]
    pub fn new(settings: &crate::config::AgentSettings) -> Self {
        let (media_model_id, media_model_provider) = match settings.get_media_model() {
            (id, provider) if !id.is_empty() && !provider.is_empty() => (Some(id), Some(provider)),
            _ => (None, None),
        };
        let media_model_name = media_model_id.clone();

        let providers = providers::build_configured_providers(settings);
        #[cfg(feature = "llm-opencode-go")]
        let opencode_go_model_catalog = providers::opencode_go::module::build_model_catalog(
            settings,
            support::http::create_http_client(),
        );
        #[cfg(feature = "llm-opencode-go")]
        let opencode_zen_model_catalog = providers::opencode_go::module::build_zen_model_catalog(
            settings,
            support::http::create_http_client(),
        );

        Self {
            providers,
            #[cfg(feature = "llm-opencode-go")]
            opencode_go_model_catalog,
            #[cfg(feature = "llm-opencode-go")]
            opencode_zen_model_catalog,
            models: settings.get_available_models(),
            media_model_name,
            media_model_id,
            media_model_provider,
        }
    }

    /// Register a custom/mock LLM provider
    pub fn register_provider(&mut self, name: String, provider: Arc<dyn LlmProvider>) {
        self.providers.insert(Self::provider_key(&name), provider);
    }

    /// Returns true if at least one multimodal provider is configured.
    #[must_use]
    pub fn is_multimodal_available(&self) -> bool {
        self.is_audio_transcription_available()
            || self.is_image_understanding_available()
            || self.is_video_understanding_available()
    }

    /// Returns true if requested provider is configured.
    #[must_use]
    pub fn is_provider_available(&self, name: &str) -> bool {
        self.providers.contains_key(&Self::provider_key(name))
    }

    /// Returns configured provider keys for diagnostics.
    #[must_use]
    pub fn configured_provider_names(&self) -> Vec<String> {
        let mut provider_names: Vec<String> = self.providers.keys().cloned().collect();
        provider_names.sort();
        provider_names
    }

    /// Returns OpenCode Go discovered models when the provider is compiled and configured.
    pub async fn opencode_go_models(&self) -> Option<Vec<DiscoveredLlmModel>> {
        #[cfg(feature = "llm-opencode-go")]
        {
            let catalog = self.opencode_go_model_catalog.as_ref()?;
            return Some(
                catalog
                    .models()
                    .await
                    .into_iter()
                    .map(DiscoveredLlmModel::from)
                    .collect(),
            );
        }
        #[cfg(not(feature = "llm-opencode-go"))]
        {
            None
        }
    }

    /// Refreshes OpenCode Go discovered models when the provider is compiled and configured.
    pub async fn refresh_opencode_go_models(&self) -> Option<Vec<DiscoveredLlmModel>> {
        #[cfg(feature = "llm-opencode-go")]
        {
            let catalog = self.opencode_go_model_catalog.as_ref()?;
            return Some(
                catalog
                    .refresh()
                    .await
                    .into_iter()
                    .map(DiscoveredLlmModel::from)
                    .collect(),
            );
        }
        #[cfg(not(feature = "llm-opencode-go"))]
        {
            None
        }
    }

    /// Returns free OpenCode Zen discovered models when the provider is compiled and configured.
    pub async fn opencode_zen_models(&self) -> Option<Vec<DiscoveredLlmModel>> {
        #[cfg(feature = "llm-opencode-go")]
        {
            let catalog = self.opencode_zen_model_catalog.as_ref()?;
            return Some(
                catalog
                    .models()
                    .await
                    .into_iter()
                    .map(DiscoveredLlmModel::from)
                    .collect(),
            );
        }
        #[cfg(not(feature = "llm-opencode-go"))]
        {
            None
        }
    }

    /// Refreshes free OpenCode Zen discovered models when the provider is compiled and configured.
    pub async fn refresh_opencode_zen_models(&self) -> Option<Vec<DiscoveredLlmModel>> {
        #[cfg(feature = "llm-opencode-go")]
        {
            let catalog = self.opencode_zen_model_catalog.as_ref()?;
            return Some(
                catalog
                    .refresh()
                    .await
                    .into_iter()
                    .map(DiscoveredLlmModel::from)
                    .collect(),
            );
        }
        #[cfg(not(feature = "llm-opencode-go"))]
        {
            None
        }
    }

    /// Returns the provider for the given name
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if the provider is not configured.
    fn get_provider(&self, provider_name: &str) -> Result<&dyn LlmProvider, LlmError> {
        self.providers
            .get(&Self::provider_key(provider_name))
            .map(Arc::as_ref)
            .ok_or_else(|| LlmError::MissingConfig(provider_name.to_string()))
    }

    /// Perform an internal plain-text completion request by configured model name.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found, or any error from the provider.
    #[instrument(skip(self, system_prompt, user_message))]
    pub(crate) async fn complete_internal_text_for_model_name(
        &self,
        purpose: InternalTextPurpose,
        system_prompt: &str,
        user_message: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;

        self.complete_internal_text(purpose, system_prompt, user_message, &model_info)
            .await
    }

    /// Perform an internal plain-text completion request for an explicit model route.
    ///
    /// # Errors
    ///
    /// Returns any provider error for the requested route.
    #[instrument(skip(self, system_prompt, user_message, model_info))]
    pub(crate) async fn complete_internal_text(
        &self,
        purpose: InternalTextPurpose,
        system_prompt: &str,
        user_message: &str,
        model_info: &crate::config::ModelInfo,
    ) -> Result<String, LlmError> {
        let provider = self.get_provider(&model_info.provider)?;
        let history = [];
        let (system_prompt, history) =
            support::history::fold_system_messages_into_prompt(system_prompt, &history);

        debug!(
            purpose = ?purpose,
            model = model_info.id,
            provider = model_info.provider,
            "Sending internal text request to LLM"
        );
        trace!(
            purpose = ?purpose,
            system_prompt = system_prompt,
            history = ?history,
            user_message = user_message,
            "Full internal text LLM request"
        );

        let start = std::time::Instant::now();
        let result = provider
            .complete_internal_text(
                &system_prompt,
                &history,
                user_message,
                &model_info.id,
                model_info.max_output_tokens,
            )
            .await;
        let duration = start.elapsed();

        if let Ok(resp) = &result {
            debug!(
                purpose = ?purpose,
                model = model_info.id,
                duration_ms = duration.as_millis(),
                "Received success response from internal text LLM request"
            );
            trace!(response = ?resp, "Full LLM Response");
        } else if let Err(e) = &result {
            warn!(
                purpose = ?purpose,
                model = model_info.id,
                duration_ms = duration.as_millis(),
                error = %e,
                "Received error response from internal text LLM request"
            );
        }

        result
    }

    /// Perform a single chat completion request with tool calling (no retry).
    ///
    /// This is the base method used by the agent runner, which owns retry handling and UI events.
    #[instrument(skip(self, system_prompt, messages, tools))]
    pub async fn chat_with_tools_single_attempt(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_name: &str,
        temperature: Option<f32>,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let model_info = self.get_model_info(model_name)?;

        self.chat_with_tools_single_attempt_for_model_info(
            system_prompt,
            messages,
            tools,
            &model_info,
            temperature,
            json_mode,
        )
        .await
    }

    /// Perform a single tool-enabled chat attempt for an explicit model route.
    #[instrument(skip(self, system_prompt, messages, tools, model_info, temperature))]
    pub async fn chat_with_tools_single_attempt_for_model_info(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_info: &crate::config::ModelInfo,
        temperature: Option<f32>,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let provider = self.get_provider(&model_info.provider)?;
        let capabilities = Self::provider_capabilities_for_model(model_info);
        let (system_prompt, messages) =
            support::history::fold_system_messages_into_prompt(system_prompt, messages);

        if !capabilities.can_run_chat_with_tools_request(!tools.is_empty(), json_mode) {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        support::history::validate_tool_history(&messages, capabilities)?;

        debug!(
            model = model_info.id,
            provider = model_info.provider,
            tools_count = tools.len(),
            messages_count = messages.len(),
            json_mode = json_mode,
            "Sending tool-enabled request to LLM (single attempt)"
        );

        let request = ChatWithToolsRequest {
            system_prompt: &system_prompt,
            messages: &messages,
            tools,
            model_id: &model_info.id,
            max_tokens: model_info.max_output_tokens,
            temperature,
            json_mode,
        };
        provider.chat_with_tools(request).await
    }

    /// Returns the provider name for a given model name.
    pub fn get_provider_name(&self, model_name: &str) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        Ok(model_info.provider)
    }

    /// Returns request-side capabilities for the named provider.
    #[must_use]
    pub fn provider_capabilities(provider_name: &str) -> ProviderCapabilities {
        capabilities::provider_capabilities(provider_name)
    }

    #[must_use]
    /// Returns capabilities for a specific configured model route.
    pub fn provider_capabilities_for_model(
        model_info: &crate::config::ModelInfo,
    ) -> ProviderCapabilities {
        capabilities::provider_capabilities_for_model(model_info)
    }

    #[must_use]
    /// Returns whether structured output should be used for a specific model route.
    pub fn supports_structured_output_for_model(model_info: &crate::config::ModelInfo) -> bool {
        capabilities::supports_structured_output_for_model(model_info)
    }

    /// Chat completion with tool calling support (for agent mode)
    ///
    /// This method includes retry logic with exponential backoff for transient errors
    /// (5xx status codes and network errors). Up to 5 attempts will be made with
    /// increasing delays: 1s, 2s, 4s, 8s, 16s.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found, if tool calling is not supported for the provider,
    /// or any error from the provider after all retry attempts are exhausted.
    #[instrument(skip(self, system_prompt, messages, tools))]
    pub async fn chat_with_tools(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_name: &str,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let capabilities = Self::provider_capabilities_for_model(&model_info);
        let (system_prompt, messages) =
            support::history::fold_system_messages_into_prompt(system_prompt, messages);

        if !capabilities.can_run_chat_with_tools_request(!tools.is_empty(), json_mode) {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        support::history::validate_tool_history(&messages, capabilities)?;

        let provider = self.get_provider(&model_info.provider)?;

        debug!(
            model = model_name,
            provider = model_info.provider,
            tools_count = tools.len(),
            messages_count = messages.len(),
            json_mode = json_mode,
            "Sending tool-enabled request to LLM"
        );

        for attempt in 1..=Self::MAX_RETRIES {
            let start = std::time::Instant::now();
            let request = ChatWithToolsRequest {
                system_prompt: &system_prompt,
                messages: &messages,
                tools,
                model_id: &model_info.id,
                max_tokens: model_info.max_output_tokens,
                temperature: None,
                json_mode,
            };
            let result = provider.chat_with_tools(request).await;
            let duration = start.elapsed();

            match result {
                Ok(resp) => {
                    if attempt > 1 {
                        info!(
                            model = model_name,
                            attempt = attempt,
                            duration_ms = duration.as_millis(),
                            "LLM retry succeeded"
                        );
                    }
                    debug!(
                        model = model_name,
                        duration_ms = duration.as_millis(),
                        tool_calls_count = resp.tool_calls.len(),
                        finish_reason = %resp.finish_reason,
                        has_reasoning = resp.reasoning_content.is_some(),
                        "Received tool response from LLM"
                    );
                    return Ok(resp);
                }
                Err(e) => {
                    warn!(
                        model = model_name,
                        attempt = attempt,
                        max_attempts = Self::MAX_RETRIES,
                        duration_ms = duration.as_millis(),
                        error = %e,
                        "Tool-enabled LLM request failed"
                    );

                    if attempt < Self::MAX_RETRIES {
                        if let Some(backoff) = Self::get_retry_delay(&e, attempt) {
                            info!(
                                model = model_name,
                                backoff_ms = backoff.as_millis(),
                                attempt = attempt,
                                max_attempts = Self::MAX_RETRIES,
                                error_type = ?e,
                                "Retrying LLM request"
                            );
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }

                    return Err(e);
                }
            }
        }

        Err(LlmError::ApiError(
            "All retry attempts exhausted".to_string(),
        ))
    }

    /// Maximum number of retry attempts for LLM calls.
    pub const MAX_RETRIES: usize = support::backoff::MAX_RETRIES;

    /// Calculates the delay before the next retry attempt based on the error type.
    /// Returns `None` if the error is not retryable.
    pub fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<std::time::Duration> {
        support::backoff::get_retry_delay(error, attempt)
    }

    /// Returns true if the error is retryable.
    pub fn is_retryable_error(error: &LlmError) -> bool {
        support::backoff::is_retryable_error(error)
    }

    /// Returns true if the error is a rate limit (429 or RateLimit variant).
    pub fn is_rate_limit_error(error: &LlmError) -> bool {
        support::backoff::is_rate_limit_error(error)
    }

    /// Returns the wait time in seconds from a rate limit error, if available.
    pub fn get_rate_limit_wait_secs(error: &LlmError) -> Option<u64> {
        support::backoff::get_rate_limit_wait_secs(error)
    }

    /// Transcribe audio to text
    ///
    /// # Errors
    ///
    /// Returns any error from the provider.
    pub async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(&model_info.provider)?;
        provider
            .transcribe_audio(audio_bytes, mime_type, &model_info.id)
            .await
    }

    /// Transcribe audio to text with a task-specific prompt.
    ///
    /// # Errors
    ///
    /// Returns any error from the provider.
    pub async fn transcribe_audio_with_prompt(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(&model_info.provider)?;
        provider
            .transcribe_audio_with_prompt(audio_bytes, mime_type, text_prompt, &model_info.id)
            .await
    }

    /// Transcribe audio with automatic fallback for text-only providers and retry logic.
    ///
    /// If the provider returns `ZAI_FALLBACK_TO_MEDIA` error, uses `media_model_provider` instead.
    /// Retries up to 5 times with exponential backoff for retryable errors.
    ///
    /// # Errors
    ///
    /// Returns any error from the provider after all retry attempts are exhausted.
    pub async fn transcribe_audio_with_fallback(
        &self,
        provider_name: &str,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let primary_result = self
            .retry_with_backoff(
                || async {
                    let provider = self.get_provider(provider_name)?;
                    provider
                        .transcribe_audio(audio_bytes.clone(), mime_type, model_id)
                        .await
                },
                &format!("Transcription with {}", provider_name),
                3000,
            )
            .await;

        match primary_result {
            Ok(text) => Ok(text),
            Err(LlmError::Unknown(msg)) if msg == "ZAI_FALLBACK_TO_MEDIA" => {
                let media_provider = self
                    .media_model_provider
                    .as_deref()
                    .ok_or_else(|| LlmError::MissingConfig("media_model_provider".to_string()))?;
                let media_model_id = self
                    .media_model_id
                    .as_deref()
                    .ok_or_else(|| LlmError::MissingConfig("media_model_id".to_string()))?;

                info!("ZAI does not support audio, falling back to media model {media_model_id}");

                self.retry_with_backoff(
                    || async {
                        let provider = self.get_provider(media_provider)?;
                        provider
                            .transcribe_audio(audio_bytes.clone(), mime_type, media_model_id)
                            .await
                    },
                    &format!("Transcription fallback with {}", media_provider),
                    3000,
                )
                .await
            }
            Err(e) => Err(e),
        }
    }

    /// Analyze an image with a text prompt
    ///
    /// # Errors
    ///
    /// Returns any error from the provider.
    pub async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(&model_info.provider)?;
        provider
            .analyze_image(image_bytes, text_prompt, system_prompt, &model_info.id)
            .await
    }

    /// Analyze a video with a text prompt
    ///
    /// # Errors
    ///
    /// Returns any error from the provider.
    pub async fn analyze_video(
        &self,
        video_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        system_prompt: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;
        let provider = self.get_provider(&model_info.provider)?;
        provider
            .analyze_video(
                video_bytes,
                mime_type,
                text_prompt,
                system_prompt,
                &model_info.id,
            )
            .await
    }

    /// Returns the model info for the given name
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found.
    pub fn get_model_info(&self, model_name: &str) -> Result<crate::config::ModelInfo, LlmError> {
        self.models
            .iter()
            .find(|(name, _)| name == model_name)
            .map(|(_, info)| info.clone())
            .ok_or_else(|| LlmError::Unknown(format!("Model {model_name} not found")))
    }

    /// Execute an async operation with retry logic and exponential backoff.
    async fn retry_with_backoff<T, F, Fut>(
        &self,
        operation: F,
        context: &str,
        initial_backoff_ms: u64,
    ) -> Result<T, LlmError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, LlmError>>,
    {
        for attempt in 1..=Self::MAX_RETRIES {
            match operation().await {
                Ok(result) => {
                    if attempt > 1 {
                        info!("{} succeeded after {} attempts", context, attempt);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if attempt < Self::MAX_RETRIES {
                        if let Some(backoff) = support::backoff::get_retry_delay_with_initial(
                            &e,
                            attempt,
                            initial_backoff_ms,
                        ) {
                            warn!(
                                "{} failed (attempt {}/{}): {}, retrying after {:?}",
                                context,
                                attempt,
                                Self::MAX_RETRIES,
                                e,
                                backoff
                            );
                            tokio::time::sleep(backoff).await;
                            continue;
                        }
                    }
                    warn!("{} failed after {} attempts: {}", context, attempt, e);
                    return Err(e);
                }
            }
        }

        Err(LlmError::ApiError(
            "All retry attempts exhausted".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{InternalTextPurpose, LlmClient};
    use crate::config::{AgentSettings, ModuleRuntimeConfig};
    use crate::llm::MockLlmProvider;
    #[cfg(feature = "llm-openrouter")]
    use crate::llm::{ChatResponse, Message};
    use std::sync::Arc;

    fn with_provider_key(
        mut settings: AgentSettings,
        module_id: &str,
        api_key: &str,
    ) -> AgentSettings {
        settings.modules.insert(
            module_id.to_string(),
            ModuleRuntimeConfig::default().with_string_value("api_key", api_key),
        );
        settings
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn media_resolver_prefers_explicit_media_route_for_video() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-openrouter".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                media_model_id: Some("google/gemini-3-flash-preview".to_string()),
                media_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let llm = LlmClient::new(&settings);
        let route = llm
            .resolve_media_model_for_video()
            .expect("media route should resolve");

        assert_eq!(route.id, "google/gemini-3-flash-preview");
        assert_eq!(route.provider, "openrouter");
    }

    #[cfg(all(feature = "llm-openrouter", feature = "llm-mistral"))]
    #[test]
    fn media_resolver_rejects_media_route_when_modality_is_not_supported() {
        let settings = with_provider_key(
            with_provider_key(
                AgentSettings {
                    agent_model_id: Some("agent-openrouter".to_string()),
                    agent_model_provider: Some("openrouter".to_string()),
                    media_model_id: Some("media-mistral".to_string()),
                    media_model_provider: Some("mistral".to_string()),
                    ..AgentSettings::default()
                },
                "llm-provider/openrouter",
                "test-openrouter-key",
            ),
            "llm-provider/mistral",
            "test-mistral-key",
        );

        let llm = LlmClient::new(&settings);
        let error = llm
            .resolve_media_model_for_image()
            .expect_err("media route should not fall back to agent route");

        assert!(matches!(
            error,
            crate::llm::LlmError::MissingConfig(message)
                if message.contains("image understanding")
        ));
    }

    #[cfg(feature = "llm-mistral")]
    #[test]
    fn media_resolver_allows_mistral_for_audio_stt_only() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-mistral".to_string()),
                agent_model_provider: Some("mistral".to_string()),
                media_model_id: Some("media-mistral".to_string()),
                media_model_provider: Some("mistral".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/mistral",
            "test-mistral-key",
        );

        let llm = LlmClient::new(&settings);
        let audio_route = llm
            .resolve_media_model_for_audio_stt()
            .expect("mistral should support stt route");

        assert_eq!(audio_route.id, "media-mistral");
        assert_eq!(audio_route.provider, "mistral");
        assert!(llm.resolve_media_model_for_video().is_err());
    }

    #[test]
    fn media_resolver_rejects_unconfigured_provider_routes() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-openrouter".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                media_model_id: Some("media-mistral".to_string()),
                media_model_provider: Some("mistral".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let llm = LlmClient::new(&settings);
        let error = llm
            .resolve_media_model_for_image()
            .expect_err("unavailable media provider must not fall back");

        assert!(matches!(
            error,
            crate::llm::LlmError::MissingConfig(message)
                if message.contains("not available")
        ));
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn media_model_name_resolvers_return_selected_route_names() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-openrouter".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                media_model_id: Some("google/gemini-3-flash-preview".to_string()),
                media_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let llm = LlmClient::new(&settings);
        assert_eq!(
            llm.resolve_media_model_name_for_audio_stt()
                .expect("audio stt route"),
            "google/gemini-3-flash-preview"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_image()
                .expect("image route"),
            "google/gemini-3-flash-preview"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_video()
                .expect("video route"),
            "google/gemini-3-flash-preview"
        );
    }

    #[cfg(all(feature = "llm-openrouter", feature = "llm-mistral"))]
    #[test]
    fn media_name_resolver_does_not_fallback_to_agent_for_non_stt_modalities() {
        let settings = with_provider_key(
            with_provider_key(
                AgentSettings {
                    agent_model_id: Some("agent-openrouter".to_string()),
                    agent_model_provider: Some("openrouter".to_string()),
                    media_model_id: Some("media-mistral".to_string()),
                    media_model_provider: Some("mistral".to_string()),
                    ..AgentSettings::default()
                },
                "llm-provider/openrouter",
                "test-openrouter-key",
            ),
            "llm-provider/mistral",
            "test-mistral-key",
        );

        let llm = LlmClient::new(&settings);
        assert_eq!(
            llm.resolve_media_model_name_for_audio_stt()
                .expect("audio stt route"),
            "media-mistral"
        );
        assert!(llm.resolve_media_model_name_for_image().is_err());
        assert!(llm.resolve_media_model_name_for_video().is_err());
    }

    #[cfg(feature = "llm-mistral")]
    #[test]
    fn multimodal_availability_is_modality_specific() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-mistral".to_string()),
                agent_model_provider: Some("mistral".to_string()),
                media_model_id: Some("media-mistral".to_string()),
                media_model_provider: Some("mistral".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/mistral",
            "test-mistral-key",
        );

        let llm = LlmClient::new(&settings);
        assert!(llm.is_multimodal_available());
        assert!(llm.is_audio_transcription_available());
        assert!(!llm.is_image_understanding_available());
        assert!(!llm.is_video_understanding_available());
    }

    #[test]
    fn multimodal_is_unavailable_when_no_supported_media_routes_exist() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("agent-openrouter".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let llm = LlmClient::new(&settings);
        assert!(!llm.is_multimodal_available());
        assert!(!llm.is_audio_transcription_available());
        assert!(!llm.is_image_understanding_available());
        assert!(!llm.is_video_understanding_available());

        let error = llm
            .resolve_media_model_for_video()
            .expect_err("video route missing");
        assert!(matches!(
            error,
            crate::llm::LlmError::MissingConfig(message)
                if message.contains("video understanding")
                    && message.contains("MEDIA_MODEL")
        ));
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn llm_client_registers_opencode_go_when_key_present() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("deepseek-v4-flash".to_string()),
                agent_model_provider: Some("opencode-go".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/opencode-go",
            "test-opencode-key",
        );

        let llm = LlmClient::new(&settings);

        assert!(llm.is_provider_available("opencode-go"));
        assert!(llm.is_provider_available("opencode_go"));
        assert!(llm
            .configured_provider_names()
            .contains(&"opencode-go".to_string()));
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn llm_client_registers_opencode_zen_when_key_present() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("opencode-zen/deepseek-v4-flash-free".to_string()),
                agent_model_provider: Some("opencode-zen".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/opencode-zen",
            "test-opencode-key",
        );

        let llm = LlmClient::new(&settings);

        assert!(llm.is_provider_available("opencode-zen"));
        assert!(llm.is_provider_available("opencode_zen"));
        assert!(llm
            .configured_provider_names()
            .contains(&"opencode-zen".to_string()));
    }

    #[cfg(feature = "llm-openrouter")]
    #[tokio::test]
    async fn main_agent_tool_request_uses_configured_temperature() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("deepseek/deepseek-v4-flash".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let mut llm = LlmClient::new(&settings);
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|request| {
            assert_eq!(request.temperature, Some(0.17));
            Ok(ChatResponse {
                content: Some("ok".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let model = crate::config::ModelInfo {
            id: "deepseek/deepseek-v4-flash".to_string(),
            provider: "openrouter".to_string(),
            max_output_tokens: 1024,
            context_window_tokens: 8192,
            weight: 1,
        };

        let response = llm
            .chat_with_tools_single_attempt_for_model_info(
                "You are helpful.",
                &[],
                &[],
                &model,
                Some(0.17),
                false,
            )
            .await
            .expect("request should succeed");

        assert_eq!(response.content.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn internal_text_completion_uses_explicit_route() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("deepseek/deepseek-v4-flash".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let mut llm = LlmClient::new(&settings);
        let mut provider = MockLlmProvider::new();
        provider.expect_complete_internal_text().return_once(
            |system_prompt, history, user_message, model_id, max_tokens| {
                assert_eq!(system_prompt, "You are helpful.");
                assert!(history.is_empty());
                assert_eq!(user_message, "new request");
                assert_eq!(model_id, "deepseek/deepseek-v4-flash");
                assert_eq!(max_tokens, 1024);
                Ok("ok".to_string())
            },
        );
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let model = crate::config::ModelInfo {
            id: "deepseek/deepseek-v4-flash".to_string(),
            provider: "openrouter".to_string(),
            max_output_tokens: 1024,
            context_window_tokens: 8192,
            weight: 1,
        };

        let response = llm
            .complete_internal_text(
                InternalTextPurpose::InputIntentClassification,
                "You are helpful.",
                "new request",
                &model,
            )
            .await
            .expect("request should succeed");

        assert_eq!(response, "ok");
    }

    #[cfg(feature = "llm-openrouter")]
    #[tokio::test]
    async fn tool_requests_fold_system_history_into_prompt() {
        let settings = with_provider_key(
            AgentSettings {
                agent_model_id: Some("deepseek/deepseek-v4-flash".to_string()),
                agent_model_provider: Some("openrouter".to_string()),
                ..AgentSettings::default()
            },
            "llm-provider/openrouter",
            "test-openrouter-key",
        );

        let mut llm = LlmClient::new(&settings);
        let mut provider = MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(|request| {
            assert_eq!(
                request.system_prompt,
                "You are helpful.\n\n[TOPIC_AGENTS_MD]\nAlways start with TL;DR.\n\n[SYSTEM: retry with strict JSON]"
            );
            assert_eq!(request.messages.len(), 2);
            assert_eq!(request.messages[0].role, "user");
            assert_eq!(request.messages[1].role, "assistant");
            Ok(ChatResponse {
                content: Some("ok".to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        llm.register_provider("openrouter".to_string(), Arc::new(provider));

        let history = vec![
            Message::system("[TOPIC_AGENTS_MD]\nAlways start with TL;DR."),
            Message::user("older request"),
            Message::system("[SYSTEM: retry with strict JSON]"),
            Message::assistant("older answer"),
        ];

        let response = llm
            .chat_with_tools(
                "You are helpful.",
                &history,
                &[],
                "deepseek/deepseek-v4-flash",
                false,
            )
            .await
            .expect("request should succeed");

        assert_eq!(response.content.as_deref(), Some("ok"));
    }
}
