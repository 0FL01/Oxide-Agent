use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info, instrument, trace, warn};

use super::{
    capabilities, embeddings, providers, support, ChatResponse, ChatWithToolsRequest, LlmError,
    LlmProvider, Message, ProviderCapabilities, ToolDefinition,
};

/// Unified client for interacting with multiple LLM providers
pub struct LlmClient {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    embedding: Option<(embeddings::EmbeddingProvider, String, u32)>,
    /// Available models configured from settings
    pub models: Vec<(String, crate::config::ModelInfo)>,
    /// Narrator model ID
    pub narrator_model: String,
    /// Narrator provider name
    pub narrator_provider: String,
    /// Default chat model name for user-facing requests
    pub chat_model_name: String,
    /// Optional media model name for multimodal requests
    pub media_model_name: Option<String>,
    /// Optional media model ID for audio/image/video fallbacks
    pub media_model_id: Option<String>,
    /// Optional media model provider for audio/image/video fallbacks
    pub media_model_provider: Option<String>,
}

impl LlmClient {
    fn create_embedding_provider(
        settings: &crate::config::AgentSettings,
    ) -> Option<(embeddings::EmbeddingProvider, String, u32)> {
        let provider_name = settings.embedding_provider.as_ref()?;
        let model_id = settings.embedding_model_id.clone()?;

        let api_key = match provider_name.to_lowercase().as_str() {
            "mistral" => settings.mistral_api_key.clone()?,
            "openrouter" => settings.openrouter_api_key.clone()?,
            "gemini" | "google" => settings.gemini_api_key.clone()?,
            _ => return None,
        };
        let provider = match provider_name.to_lowercase().as_str() {
            "gemini" | "google" => embeddings::EmbeddingProvider::new_gemini(api_key),
            _ => {
                let api_base = embeddings::get_api_base(provider_name)?;
                embeddings::EmbeddingProvider::new_openai_compatible(api_key, api_base.to_string())
            }
        };

        let dimensions = settings
            .embedding_dimensions
            .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSIONS);

        Some((provider, model_id, dimensions))
    }

    fn provider_key(name: &str) -> String {
        name.to_ascii_lowercase()
    }

    fn insert_provider(
        providers: &mut HashMap<String, Arc<dyn LlmProvider>>,
        name: &str,
        provider: Arc<dyn LlmProvider>,
    ) {
        providers.insert(Self::provider_key(name), provider);
    }

    fn resolve_media_route_for_modality(
        &self,
        modality: capabilities::MediaModality,
    ) -> Result<(String, crate::config::ModelInfo), LlmError> {
        let mut candidates = Vec::with_capacity(2);
        if let Some(name) = self.media_model_name.as_deref() {
            if !name.is_empty() {
                candidates.push(name);
            }
        }

        if !self.chat_model_name.is_empty()
            && !candidates
                .iter()
                .any(|candidate| *candidate == self.chat_model_name)
        {
            candidates.push(&self.chat_model_name);
        }

        for model_name in candidates {
            let Ok(model_info) = self.get_model_info(model_name) else {
                continue;
            };

            if !self.is_provider_available(&model_info.provider) {
                continue;
            }

            if capabilities::provider_media_capabilities_for_model(&model_info).supports(modality) {
                return Ok((model_name.to_string(), model_info));
            }
        }

        Err(LlmError::MissingConfig(format!(
            "No configured route supports {} (expected providers: gemini/openrouter, mistral for STT)",
            modality.label()
        )))
    }

    /// Resolve the model route for audio transcription.
    ///
    /// Prefers explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER` and falls back to chat model
    /// when it supports audio transcription.
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
    /// Prefers explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER` and falls back to chat model
    /// when it supports image understanding.
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
    /// Prefers explicit `MEDIA_MODEL_ID`/`MEDIA_MODEL_PROVIDER` and falls back to chat model
    /// when it supports video understanding.
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
        let chat_model_name = settings.get_default_chat_model_name();
        let (media_model_id, media_model_provider) = match settings.get_media_model() {
            (id, provider) if !id.is_empty() && !provider.is_empty() => (Some(id), Some(provider)),
            _ => (None, None),
        };
        let media_model_name = media_model_id.clone();

        let http_client = support::http::create_http_client();

        let mut providers = HashMap::new();

        if let Some(api_key) = settings.groq_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "groq",
                Arc::new(providers::GroqProvider::new(api_key.clone())),
            );
        }

        if let Some(api_key) = settings.mistral_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "mistral",
                Arc::new(providers::MistralProvider::new_with_client(
                    api_key.clone(),
                    http_client.clone(),
                )),
            );
        }

        if let Some(api_key) = settings.minimax_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "minimax",
                Arc::new(providers::MiniMaxProvider::new(api_key.clone())),
            );
        }

        if let Some(api_key) = settings.zai_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "zai",
                Arc::new(providers::ZaiProvider::new_with_client(
                    api_key.clone(),
                    settings.zai_api_base.clone(),
                    http_client.clone(),
                )),
            );
        }

        if let Some(api_key) = settings.gemini_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "gemini",
                Arc::new(providers::GeminiProvider::new(api_key.clone())),
            );
        }

        if let Some(api_key) = settings.nvidia_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "nvidia",
                Arc::new(providers::NvidiaProvider::new_with_client(
                    api_key.clone(),
                    settings.nvidia_api_base.clone(),
                    http_client.clone(),
                )),
            );
        }

        if let Some(api_key) = settings.openrouter_api_key.as_ref() {
            Self::insert_provider(
                &mut providers,
                "openrouter",
                Arc::new(providers::OpenRouterProvider::new_with_client(
                    api_key.clone(),
                    settings.openrouter_site_url.clone(),
                    settings.openrouter_site_name.clone(),
                    http_client,
                )),
            );
        }

        Self {
            providers,
            embedding: Self::create_embedding_provider(settings),
            models: settings.get_available_models(),
            narrator_model: settings.get_configured_narrator_model().0,
            narrator_provider: settings.get_configured_narrator_model().1,
            chat_model_name,
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

    /// Returns true if embedding provider is configured.
    #[must_use]
    pub fn is_embedding_available(&self) -> bool {
        self.embedding.is_some()
    }

    /// Returns true if requested provider is configured.
    #[must_use]
    pub fn is_provider_available(&self, name: &str) -> bool {
        self.providers.contains_key(&Self::provider_key(name))
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

    /// Perform a chat completion request
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found, or any error from the provider.
    #[instrument(skip(self, system_prompt, history))]
    pub async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_name: &str,
    ) -> Result<String, LlmError> {
        let model_info = self.get_model_info(model_name)?;

        self.chat_completion_for_model_info(system_prompt, history, user_message, &model_info)
            .await
    }

    /// Perform a chat completion request for an explicit model route.
    ///
    /// # Errors
    ///
    /// Returns any provider error for the requested route.
    #[instrument(skip(self, system_prompt, history, model_info))]
    pub async fn chat_completion_for_model_info(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_info: &crate::config::ModelInfo,
    ) -> Result<String, LlmError> {
        let provider = self.get_provider(&model_info.provider)?;

        debug!(
            model = model_info.id,
            provider = model_info.provider,
            "Sending request to LLM"
        );
        trace!(
            system_prompt = system_prompt,
            history = ?history,
            user_message = user_message,
            "Full LLM Request"
        );

        let start = std::time::Instant::now();
        let result = provider
            .chat_completion(
                system_prompt,
                history,
                user_message,
                &model_info.id,
                model_info.max_output_tokens,
            )
            .await;
        let duration = start.elapsed();

        if let Ok(resp) = &result {
            debug!(
                model = model_info.id,
                duration_ms = duration.as_millis(),
                "Received success response from LLM"
            );
            trace!(response = ?resp, "Full LLM Response");
        } else if let Err(e) = &result {
            warn!(
                model = model_info.id,
                duration_ms = duration.as_millis(),
                error = %e,
                "Received error response from LLM"
            );
        }

        result
    }

    /// Perform a single chat completion request with tool calling (no retry).
    ///
    /// This is the base method used by `chat_with_tools` which handles retries internally.
    /// For agent runner retry handling with UI events, use `chat_with_tools_once` instead.
    #[instrument(skip(self, system_prompt, messages, tools))]
    pub async fn chat_with_tools_single_attempt(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_name: &str,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let model_info = self.get_model_info(model_name)?;

        self.chat_with_tools_single_attempt_for_model_info(
            system_prompt,
            messages,
            tools,
            &model_info,
            json_mode,
        )
        .await
    }

    /// Perform a single tool-enabled chat attempt for an explicit model route.
    #[instrument(skip(self, system_prompt, messages, tools, model_info))]
    pub async fn chat_with_tools_single_attempt_for_model_info(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_info: &crate::config::ModelInfo,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        let provider = self.get_provider(&model_info.provider)?;
        let capabilities = Self::provider_capabilities_for_model(model_info);

        if !capabilities.can_run_chat_with_tools_request(!tools.is_empty(), json_mode) {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        support::history::validate_tool_history(messages, capabilities)?;

        debug!(
            model = model_info.id,
            provider = model_info.provider,
            tools_count = tools.len(),
            messages_count = messages.len(),
            json_mode = json_mode,
            "Sending tool-enabled request to LLM (single attempt)"
        );

        let request = ChatWithToolsRequest {
            system_prompt,
            messages,
            tools,
            model_id: &model_info.id,
            max_tokens: model_info.max_output_tokens,
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

        if !capabilities.can_run_chat_with_tools_request(!tools.is_empty(), json_mode) {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        support::history::validate_tool_history(messages, capabilities)?;

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
                system_prompt,
                messages,
                tools,
                model_id: &model_info.id,
                max_tokens: model_info.max_output_tokens,
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

    /// Generate an embedding vector using configured provider.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if embedding provider is not configured, or any provider error.
    pub async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, LlmError> {
        self.generate_embedding_for_task(text, None, None).await
    }

    /// Generate an embedding vector with provider-specific retrieval task metadata.
    pub async fn generate_embedding_for_task(
        &self,
        text: &str,
        task_type: Option<embeddings::EmbeddingTaskType>,
        title: Option<&str>,
    ) -> Result<Vec<f32>, LlmError> {
        let (provider, model, dimensions) = self.embedding.as_ref().ok_or_else(|| {
            LlmError::MissingConfig("embedding provider not configured".to_string())
        })?;

        provider
            .generate(text, model, task_type, title, Some(*dimensions))
            .await
    }

    /// Probe embedding dimension by making a test request.
    ///
    /// Returns `None` if embedding provider is not configured or the probe fails.
    pub async fn probe_embedding_dimension(&self) -> Option<usize> {
        let (provider, model, _) = self.embedding.as_ref()?;
        provider.probe_dimension(model).await
    }

    /// Return the configured embedding output dimensionality.
    ///
    /// Returns `None` if embedding provider is not configured.
    #[must_use]
    pub fn embedding_dimensions(&self) -> Option<u32> {
        self.embedding.as_ref().map(|(_, _, dim)| *dim)
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
    /// If the provider returns `ZAI_FALLBACK_TO_GEMINI` error, uses `media_model_provider` instead.
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
            Err(LlmError::Unknown(msg)) if msg == "ZAI_FALLBACK_TO_GEMINI" => {
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
    use super::LlmClient;
    use crate::config::AgentSettings;

    #[test]
    fn media_resolver_prefers_explicit_media_route_for_video() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-openrouter".to_string()),
            chat_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-gemini".to_string()),
            media_model_provider: Some("gemini".to_string()),
            openrouter_api_key: Some("test-openrouter-key".to_string()),
            gemini_api_key: Some("test-gemini-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        let route = llm
            .resolve_media_model_for_video()
            .expect("media route should resolve");

        assert_eq!(route.id, "media-gemini");
        assert_eq!(route.provider, "gemini");
    }

    #[test]
    fn media_resolver_falls_back_to_chat_route_when_media_is_stt_only() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-openrouter".to_string()),
            chat_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-mistral".to_string()),
            media_model_provider: Some("mistral".to_string()),
            openrouter_api_key: Some("test-openrouter-key".to_string()),
            mistral_api_key: Some("test-mistral-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        let image_route = llm
            .resolve_media_model_for_image()
            .expect("chat route should be used for image modality");

        assert_eq!(image_route.id, "chat-openrouter");
        assert_eq!(image_route.provider, "openrouter");
    }

    #[test]
    fn media_resolver_allows_mistral_for_audio_stt_only() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-mistral".to_string()),
            chat_model_provider: Some("mistral".to_string()),
            mistral_api_key: Some("test-mistral-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        let audio_route = llm
            .resolve_media_model_for_audio_stt()
            .expect("mistral should support stt route");

        assert_eq!(audio_route.id, "chat-mistral");
        assert_eq!(audio_route.provider, "mistral");
        assert!(llm.resolve_media_model_for_video().is_err());
    }

    #[test]
    fn media_resolver_skips_unconfigured_provider_routes() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-openrouter".to_string()),
            chat_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-gemini".to_string()),
            media_model_provider: Some("gemini".to_string()),
            openrouter_api_key: Some("test-openrouter-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        let route = llm
            .resolve_media_model_for_image()
            .expect("chat route should be used when media provider is unavailable");

        assert_eq!(route.id, "chat-openrouter");
        assert_eq!(route.provider, "openrouter");
    }

    #[test]
    fn media_model_name_resolvers_return_selected_route_names() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-openrouter".to_string()),
            chat_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-gemini".to_string()),
            media_model_provider: Some("gemini".to_string()),
            openrouter_api_key: Some("test-openrouter-key".to_string()),
            gemini_api_key: Some("test-gemini-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        assert_eq!(
            llm.resolve_media_model_name_for_audio_stt()
                .expect("audio stt route"),
            "media-gemini"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_image()
                .expect("image route"),
            "media-gemini"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_video()
                .expect("video route"),
            "media-gemini"
        );
    }

    #[test]
    fn media_name_resolver_falls_back_to_chat_for_non_stt_modalities() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-openrouter".to_string()),
            chat_model_provider: Some("openrouter".to_string()),
            media_model_id: Some("media-mistral".to_string()),
            media_model_provider: Some("mistral".to_string()),
            openrouter_api_key: Some("test-openrouter-key".to_string()),
            mistral_api_key: Some("test-mistral-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        assert_eq!(
            llm.resolve_media_model_name_for_audio_stt()
                .expect("audio stt route"),
            "media-mistral"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_image()
                .expect("image route"),
            "chat-openrouter"
        );
        assert_eq!(
            llm.resolve_media_model_name_for_video()
                .expect("video route"),
            "chat-openrouter"
        );
    }

    #[test]
    fn multimodal_availability_is_modality_specific() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-mistral".to_string()),
            chat_model_provider: Some("mistral".to_string()),
            mistral_api_key: Some("test-mistral-key".to_string()),
            ..AgentSettings::default()
        };

        let llm = LlmClient::new(&settings);
        assert!(llm.is_multimodal_available());
        assert!(llm.is_audio_transcription_available());
        assert!(!llm.is_image_understanding_available());
        assert!(!llm.is_video_understanding_available());
    }

    #[test]
    fn multimodal_is_unavailable_when_no_supported_media_routes_exist() {
        let settings = AgentSettings {
            chat_model_id: Some("chat-groq".to_string()),
            chat_model_provider: Some("groq".to_string()),
            groq_api_key: Some("test-groq-key".to_string()),
            ..AgentSettings::default()
        };

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
                    && message.contains("gemini/openrouter")
        ));
    }
}
