//! LLM providers and client
//!
//! Provides a unified interface to various LLM providers (Groq, Mistral, Gemini, OpenRouter).

mod capabilities;
mod common;
pub mod embeddings;
mod error;
pub mod http_utils;
mod openai_compat;
/// Implementations of specific LLM providers
pub mod providers;
mod retry;
mod types;
mod validation;

use std::collections::HashMap;
use std::sync::Arc;

use tracing::{debug, info, instrument, trace, warn};

pub use capabilities::{ProviderCapabilities, ToolHistoryMode};
pub use error::LlmError;
pub use types::{
    ChatResponse, ChatWithToolsRequest, InvocationId, Message, ProviderItemId, ProviderToolCallId,
    TokenUsage, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition, ToolProtocol,
    ToolTransport,
};

/// Interface for all LLM providers
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Generate a chat completion
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError>;

    /// Transcribe audio content
    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;

    /// Analyze an image
    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError>;

    /// Chat completion with tool calling support (optional, not all providers support it)
    ///
    /// Default implementation returns an error indicating tool calling is not supported.
    /// Providers that support tool calling (e.g., Mistral, ZAI) should override this method.
    async fn chat_with_tools<'a>(
        &self,
        _request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        Err(LlmError::Unknown(
            "Tool calling not supported by this provider".to_string(),
        ))
    }
}

/// Unified client for interacting with multiple LLM providers
pub struct LlmClient {
    groq: Option<providers::GroqProvider>,
    mistral: Option<providers::MistralProvider>,
    minimax: Option<providers::MiniMaxProvider>,
    zai: Option<providers::ZaiProvider>,
    gemini: Option<providers::GeminiProvider>,
    nvidia: Option<providers::NvidiaProvider>,
    openrouter: Option<providers::OpenRouterProvider>,
    embedding: Option<(embeddings::EmbeddingProvider, String)>,
    custom_providers: HashMap<String, Arc<dyn LlmProvider>>,
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
    /// Optional media model ID for audio/image fallbacks
    pub media_model_id: Option<String>,
    /// Optional media model provider for audio/image fallbacks
    pub media_model_provider: Option<String>,
    /// Shared HTTP client with connection pool for all providers
    /// Used to create providers with shared connection pool
    #[allow(dead_code)]
    http_client: reqwest::Client,
}

impl LlmClient {
    fn create_embedding_provider(
        settings: &crate::config::AgentSettings,
    ) -> Option<(embeddings::EmbeddingProvider, String)> {
        let provider_name = settings.embedding_provider.as_ref()?;
        let model_id = settings.embedding_model_id.clone()?;

        let api_key = match provider_name.to_lowercase().as_str() {
            "mistral" => settings.mistral_api_key.clone()?,
            "openrouter" => settings.openrouter_api_key.clone()?,
            _ => return None,
        };

        let api_base = embeddings::get_api_base(provider_name)?;

        Some((
            embeddings::EmbeddingProvider::new(api_key, api_base.to_string()),
            model_id,
        ))
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

        // Create shared HTTP client with connection pooling
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(10)
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            groq: settings
                .groq_api_key
                .as_ref()
                .map(|k| providers::GroqProvider::new(k.clone())),
            mistral: settings.mistral_api_key.as_ref().map(|k| {
                providers::MistralProvider::new_with_client(k.clone(), http_client.clone())
            }),
            minimax: settings
                .minimax_api_key
                .as_ref()
                .map(|k| providers::MiniMaxProvider::new(k.clone())),
            zai: settings.zai_api_key.as_ref().map(|k| {
                providers::ZaiProvider::new_with_client(
                    k.clone(),
                    settings.zai_api_base.clone(),
                    http_client.clone(),
                )
            }),
            gemini: settings
                .gemini_api_key
                .as_ref()
                .map(|k| providers::GeminiProvider::new(k.clone())),
            nvidia: settings.nvidia_api_key.as_ref().map(|k| {
                providers::NvidiaProvider::new_with_client(
                    k.clone(),
                    settings.nvidia_api_base.clone(),
                    http_client.clone(),
                )
            }),
            openrouter: settings.openrouter_api_key.as_ref().map(|k| {
                providers::OpenRouterProvider::new_with_client(
                    k.clone(),
                    settings.openrouter_site_url.clone(),
                    settings.openrouter_site_name.clone(),
                    http_client.clone(),
                )
            }),
            embedding: Self::create_embedding_provider(settings),
            models: settings.get_available_models(),
            narrator_model: settings.get_configured_narrator_model().0,
            narrator_provider: settings.get_configured_narrator_model().1,
            chat_model_name,
            media_model_name,
            media_model_id,
            media_model_provider,
            custom_providers: HashMap::new(),
            http_client,
        }
    }

    /// Register a custom/mock LLM provider
    pub fn register_provider(&mut self, name: String, provider: Arc<dyn LlmProvider>) {
        self.custom_providers.insert(name, provider);
    }

    /// Returns true if at least one multimodal provider is configured.
    #[must_use]
    pub fn is_multimodal_available(&self) -> bool {
        self.gemini.is_some() || self.openrouter.is_some()
    }

    /// Returns true if embedding provider is configured.
    #[must_use]
    pub fn is_embedding_available(&self) -> bool {
        self.embedding.is_some()
    }

    /// Returns true if requested provider is configured.
    #[must_use]
    pub fn is_provider_available(&self, name: &str) -> bool {
        if self.custom_providers.contains_key(name) {
            return true;
        }
        if name.eq_ignore_ascii_case("groq") {
            return self.groq.is_some();
        }
        if name.eq_ignore_ascii_case("mistral") {
            return self.mistral.is_some();
        }
        if name.eq_ignore_ascii_case("minimax") {
            return self.minimax.is_some();
        }
        if name.eq_ignore_ascii_case("zai") {
            return self.zai.is_some();
        }
        if name.eq_ignore_ascii_case("gemini") {
            return self.gemini.is_some();
        }
        if name.eq_ignore_ascii_case("nvidia") {
            return self.nvidia.is_some();
        }
        if name.eq_ignore_ascii_case("openrouter") {
            return self.openrouter.is_some();
        }
        false
    }

    /// Returns the provider for the given name
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if the provider is not configured.
    fn get_provider(&self, provider_name: &str) -> Result<&dyn LlmProvider, LlmError> {
        if let Some(provider) = self.custom_providers.get(provider_name) {
            return Ok(provider.as_ref());
        }
        match provider_name {
            "groq" => self.groq.as_ref().map(|p| p as &dyn LlmProvider),
            "mistral" => self.mistral.as_ref().map(|p| p as &dyn LlmProvider),
            "minimax" => self.minimax.as_ref().map(|p| p as &dyn LlmProvider),
            "zai" => self.zai.as_ref().map(|p| p as &dyn LlmProvider),
            "gemini" => self.gemini.as_ref().map(|p| p as &dyn LlmProvider),
            "nvidia" => self.nvidia.as_ref().map(|p| p as &dyn LlmProvider),
            "openrouter" => self.openrouter.as_ref().map(|p| p as &dyn LlmProvider),
            _ => None,
        }
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
        // Get provider and call its chat_with_tools method (via trait)
        let provider = self.get_provider(&model_info.provider)?;
        let capabilities = Self::provider_capabilities_for_model(model_info);

        if !capabilities.can_run_agent_tools() {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        validation::validate_tool_history(messages, capabilities)?;

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
        // Retry configuration (hardcoded with reasonable defaults)
        const MAX_RETRIES: usize = 5;

        let model_info = self.get_model_info(model_name)?;
        let capabilities = Self::provider_capabilities_for_model(&model_info);

        if !capabilities.can_run_agent_tools() {
            return Err(LlmError::ApiError(format!(
                "Tool-enabled agent calls are not supported for {} model `{}`",
                model_info.provider, model_info.id
            )));
        }

        validation::validate_tool_history(messages, capabilities)?;

        // Get provider and call its chat_with_tools method (via trait)
        let provider = self.get_provider(&model_info.provider)?;

        debug!(
            model = model_name,
            provider = model_info.provider,
            tools_count = tools.len(),
            messages_count = messages.len(),
            json_mode = json_mode,
            "Sending tool-enabled request to LLM"
        );

        for attempt in 1..=MAX_RETRIES {
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
                        max_attempts = MAX_RETRIES,
                        duration_ms = duration.as_millis(),
                        error = %e,
                        "Tool-enabled LLM request failed"
                    );

                    // Check if error is retryable and we have attempts left
                    if attempt < MAX_RETRIES {
                        if let Some(backoff) = Self::get_retry_delay(&e, attempt) {
                            info!(
                                model = model_name,
                                backoff_ms = backoff.as_millis(),
                                attempt = attempt,
                                max_attempts = MAX_RETRIES,
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

        // This should be unreachable, but just in case
        Err(LlmError::ApiError(
            "All retry attempts exhausted".to_string(),
        ))
    }

    /// Maximum number of retry attempts for LLM calls.
    pub const MAX_RETRIES: usize = retry::MAX_RETRIES;

    /// Calculates the delay before the next retry attempt based on the error type.
    /// Returns `None` if the error is not retryable.
    pub fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<std::time::Duration> {
        retry::get_retry_delay(error, attempt)
    }

    /// Returns true if the error is retryable.
    pub fn is_retryable_error(error: &LlmError) -> bool {
        retry::is_retryable_error(error)
    }

    /// Returns true if the error is a rate limit (429 or RateLimit variant).
    pub fn is_rate_limit_error(error: &LlmError) -> bool {
        retry::is_rate_limit_error(error)
    }

    /// Returns the wait time in seconds from a rate limit error, if available.
    pub fn get_rate_limit_wait_secs(error: &LlmError) -> Option<u64> {
        retry::get_rate_limit_wait_secs(error)
    }

    /// Generate an embedding vector using configured provider.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if embedding provider is not configured, or any provider error.
    pub async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, LlmError> {
        let (provider, model) = self.embedding.as_ref().ok_or_else(|| {
            LlmError::MissingConfig("embedding provider not configured".to_string())
        })?;

        provider.generate(text, model).await
    }

    /// Probe embedding dimension by making a test request.
    ///
    /// Returns `None` if embedding provider is not configured or the probe fails.
    pub async fn probe_embedding_dimension(&self) -> Option<usize> {
        let (provider, model) = self.embedding.as_ref()?;
        provider.probe_dimension(model).await
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
        // Try primary provider with retry (first retry after 3s)
        let primary_result = self
            .retry_with_backoff(
                || async {
                    let provider = self.get_provider(provider_name)?;
                    provider
                        .transcribe_audio(audio_bytes.clone(), mime_type, model_id)
                        .await
                },
                &format!("Transcription with {}", provider_name),
                3000, // Initial backoff: 3s, then 6s, 12s, 24s
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

                // Try fallback provider with retry (first retry after 3s)
                self.retry_with_backoff(
                    || async {
                        let provider = self.get_provider(media_provider)?;
                        provider
                            .transcribe_audio(audio_bytes.clone(), mime_type, media_model_id)
                            .await
                    },
                    &format!("Transcription fallback with {}", media_provider),
                    3000, // Initial backoff: 3s, then 6s, 12s, 24s
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
    ///
    /// Retries up to 5 times with exponential backoff for retryable errors
    /// (5xx status codes, network errors, rate limits).
    ///
    /// # Type Parameters
    ///
    /// * `T` - The success type returned by the operation
    /// * `F` - The operation function type
    /// * `Fut` - The future type returned by the operation
    ///
    /// # Arguments
    ///
    /// * `operation` - Async closure that returns `Result<T, LlmError>`
    /// * `context` - Description of the operation for logging
    /// * `initial_backoff_ms` - Initial backoff in milliseconds (doubles each retry)
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
        const MAX_RETRIES: usize = retry::MAX_RETRIES;

        for attempt in 1..=MAX_RETRIES {
            match operation().await {
                Ok(result) => {
                    if attempt > 1 {
                        info!("{} succeeded after {} attempts", context, attempt);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if attempt < MAX_RETRIES {
                        if let Some(backoff) =
                            retry::get_retry_delay_with_initial(&e, attempt, initial_backoff_ms)
                        {
                            warn!(
                                "{} failed (attempt {}/{}): {}, retrying after {:?}",
                                context, attempt, MAX_RETRIES, e, backoff
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

        // This should be unreachable, but just in case
        Err(LlmError::ApiError(
            "All retry attempts exhausted".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        InvocationId, Message, ProviderItemId, ProviderToolCallId, ToolCall, ToolCallCorrelation,
        ToolCallFunction, ToolProtocol, ToolTransport,
    };
    use serde_json::json;

    fn tool_call(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            ToolCallFunction {
                name: name.to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
    }

    #[test]
    fn tool_call_correlation_defaults_to_invocation_id_for_legacy_wire_usage() {
        let correlation = ToolCallCorrelation::from_legacy_tool_call_id("call-123");

        assert_eq!(correlation.invocation_id, InvocationId::from("call-123"));
        assert_eq!(correlation.wire_tool_call_id(), "call-123");
        assert_eq!(correlation.legacy_tool_call_id(), "call-123");
        assert!(correlation.provider_tool_call_id.is_none());
        assert!(correlation.provider_item_id.is_none());
        assert_eq!(correlation.protocol, ToolProtocol::ChatLike);
        assert_eq!(correlation.transport, ToolTransport::ClientRoundTrip);
    }

    #[test]
    fn tool_call_correlation_prefers_provider_ids_when_present() {
        let correlation = ToolCallCorrelation::new("invoke-1")
            .with_provider_tool_call_id("provider-call-9")
            .with_provider_item_id("item-4")
            .with_protocol(ToolProtocol::ResponsesLike)
            .with_transport(ToolTransport::ServerExecuted);

        assert_eq!(correlation.wire_tool_call_id(), "provider-call-9");
        assert_eq!(correlation.legacy_tool_call_id(), "invoke-1");
        assert_eq!(
            correlation.provider_tool_call_id,
            Some(ProviderToolCallId::from("provider-call-9"))
        );
        assert_eq!(
            correlation.provider_item_id,
            Some(ProviderItemId::from("item-4"))
        );
        assert_eq!(correlation.protocol, ToolProtocol::ResponsesLike);
        assert_eq!(correlation.transport, ToolTransport::ServerExecuted);
    }

    #[test]
    fn tool_call_uses_explicit_correlation_for_runtime_and_wire_ids() {
        let tool_call = ToolCall::new(
            "legacy-provider-id",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-1")
                .with_provider_tool_call_id("provider-call-1")
                .with_protocol(ToolProtocol::AnthropicClientTools),
        );

        assert_eq!(tool_call.invocation_id().as_str(), "invoke-1");
        assert_eq!(tool_call.wire_tool_call_id(), "provider-call-1");
        assert_eq!(
            tool_call.correlation().protocol,
            ToolProtocol::AnthropicClientTools
        );
    }

    #[test]
    fn tool_message_serialization_includes_legacy_and_canonical_correlation_fields() {
        let message = Message::tool("call-1", "search", "result");
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["tool_call_id"], json!("call-1"));
        assert_eq!(
            value["tool_call_correlation"]["invocation_id"],
            json!("call-1")
        );
    }

    #[test]
    fn legacy_tool_message_resolves_correlation_from_tool_call_id() {
        let legacy = json!({
            "role": "tool",
            "content": "result",
            "tool_call_id": "call-legacy",
            "name": "search"
        });
        let message: Message = serde_json::from_value(legacy).expect("message deserializes");

        assert_eq!(message.tool_call_correlation, None);
        assert_eq!(
            message.resolved_tool_call_correlation(),
            Some(ToolCallCorrelation::from_legacy_tool_call_id("call-legacy"))
        );
    }

    #[test]
    fn assistant_tool_batch_serialization_includes_correlation_vector() {
        let message =
            Message::assistant_with_tools("calling tools", vec![tool_call("call-1", "search")]);
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["tool_calls"][0]["id"], json!("call-1"));
        assert_eq!(
            value["tool_call_correlations"][0]["invocation_id"],
            json!("call-1")
        );
    }

    #[test]
    fn assistant_tool_batch_uses_explicit_tool_call_correlation_metadata() {
        let correlated_tool_call = ToolCall::new(
            "provider-id",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-2")
                .with_provider_tool_call_id("provider-call-2")
                .with_protocol(ToolProtocol::ChatLike),
        );
        let message = Message::assistant_with_tools("calling tools", vec![correlated_tool_call]);

        assert_eq!(
            message.resolved_tool_call_correlations(),
            Some(vec![ToolCallCorrelation::new("invoke-2")
                .with_provider_tool_call_id("provider-call-2")
                .with_protocol(ToolProtocol::ChatLike)])
        );
    }

    #[test]
    fn legacy_assistant_tool_batch_resolves_correlations_from_tool_call_ids() {
        let legacy = json!({
            "role": "assistant",
            "content": "calling tools",
            "tool_calls": [{
                "id": "call-legacy",
                "function": {
                    "name": "search",
                    "arguments": "{}"
                },
                "is_recovered": false
            }]
        });
        let message: Message = serde_json::from_value(legacy).expect("message deserializes");

        assert_eq!(message.tool_call_correlations, None);
        assert_eq!(
            message.resolved_tool_call_correlations(),
            Some(vec![ToolCallCorrelation::from_legacy_tool_call_id(
                "call-legacy"
            )])
        );
    }
}
