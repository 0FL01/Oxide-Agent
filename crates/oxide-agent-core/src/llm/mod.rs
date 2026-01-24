//! LLM providers and client
//!
//! Provides a unified interface to various LLM providers (Groq, Mistral, Gemini, OpenRouter).

mod common;
pub mod embeddings;
mod http_utils;
mod openai_compat;
/// Implementations of specific LLM providers
pub mod providers;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, instrument, trace, warn};

/// Errors that can occur during LLM operations
#[derive(Debug, Error)]
pub enum LlmError {
    /// Error returned by the provider's API
    #[error("API error: {0}")]
    ApiError(String),
    /// Error during network communication
    #[error("Network error: {0}")]
    NetworkError(String),
    /// Error during JSON serialization or deserialization
    #[error("JSON error: {0}")]
    JsonError(String),
    /// Missing provider configuration or API key
    #[error("Missing client/API key: {0}")]
    MissingConfig(String),
    /// Rate limit exceeded (429), optionally with a wait time
    #[error("Rate limit exceeded: {message} (wait: {wait_secs:?}s)")]
    RateLimit {
        /// Retry-After duration in seconds, if provided by the server
        wait_secs: Option<u64>,
        /// Error message from the server
        message: String,
    },
    /// Any other unexpected error
    #[error("Unknown error: {0}")]
    Unknown(String),
}

/// A message in an LLM conversation
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    /// Role of the message sender (user, assistant, system, tool)
    pub role: String,
    /// Text content of the message
    pub content: String,
    /// Tool call ID (for tool responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Tool name (for tool responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls made by the assistant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl Message {
    /// Create a new user message
    #[must_use]
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create a new assistant message
    #[must_use]
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    /// Create a new assistant message with tool calls
    #[must_use]
    pub fn assistant_with_tools(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: Some(tool_calls),
        }
    }

    /// Create a new tool response message
    #[must_use]
    pub fn tool(tool_call_id: &str, _name: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_call_id: Some(tool_call_id.to_string()),
            name: None,
            tool_calls: None,
        }
    }

    /// Create a new system message
    #[must_use]
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }
}

/// Tool definition for LLM function calling
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    /// Name of the tool
    pub name: String,
    /// Description of what the tool does
    pub description: String,
    /// JSON schema for tool parameters
    pub parameters: serde_json::Value,
}

/// Tool call from LLM response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for the tool call
    pub id: String,
    /// Function to be called
    #[serde(rename = "function")]
    pub function: ToolCallFunction,
    /// Whether this tool call was recovered from a malformed LLM response
    #[serde(default)]
    pub is_recovered: bool,
}

/// Function details within a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    /// Name of the function being called
    pub name: String,
    /// Arguments for the function call (JSON string)
    pub arguments: String,
}

/// Token usage statistics from API response
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    /// Input tokens (system prompt + history + files)
    pub prompt_tokens: u32,
    /// Output tokens (model response + reasoning)
    pub completion_tokens: u32,
    /// Total tokens used
    pub total_tokens: u32,
}

/// Chat response that may include tool calls
#[derive(Debug, Clone)]
pub struct ChatResponse {
    /// Optional text content of the response
    pub content: Option<String>,
    /// List of tool calls requested by the model
    pub tool_calls: Vec<ToolCall>,
    /// Reason why the model stopped generating
    pub finish_reason: String,
    /// Optional reasoning/thinking process (for models that support it, e.g., GLM-4.7)
    pub reasoning_content: Option<String>,
    /// Token usage statistics (if provided by the API)
    pub usage: Option<TokenUsage>,
}

/// Interface for all LLM providers
#[cfg_attr(test, mockall::automock)]
#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)]
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
    async fn chat_with_tools(
        &self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model_id: &str,
        _max_tokens: u32,
        _json_mode: bool,
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
    zai: Option<providers::ZaiProvider>,
    gemini: Option<providers::GeminiProvider>,
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

        Self {
            groq: settings
                .groq_api_key
                .as_ref()
                .map(|k| providers::GroqProvider::new(k.clone())),
            mistral: settings
                .mistral_api_key
                .as_ref()
                .map(|k| providers::MistralProvider::new(k.clone())),
            zai: settings
                .zai_api_key
                .as_ref()
                .map(|k| providers::ZaiProvider::new(k.clone(), settings.zai_api_base.clone())),
            gemini: settings
                .gemini_api_key
                .as_ref()
                .map(|k| providers::GeminiProvider::new(k.clone())),
            openrouter: settings.openrouter_api_key.as_ref().map(|k| {
                providers::OpenRouterProvider::new(
                    k.clone(),
                    settings.openrouter_site_url.clone(),
                    settings.openrouter_site_name.clone(),
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
        if name.eq_ignore_ascii_case("groq") {
            return self.groq.is_some();
        }
        if name.eq_ignore_ascii_case("mistral") {
            return self.mistral.is_some();
        }
        if name.eq_ignore_ascii_case("zai") {
            return self.zai.is_some();
        }
        if name.eq_ignore_ascii_case("gemini") {
            return self.gemini.is_some();
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
            "zai" => self.zai.as_ref().map(|p| p as &dyn LlmProvider),
            "gemini" => self.gemini.as_ref().map(|p| p as &dyn LlmProvider),
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

        let provider = self.get_provider(&model_info.provider)?;

        debug!(
            model = model_name,
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
                model_info.max_tokens,
            )
            .await;
        let duration = start.elapsed();

        if let Ok(resp) = &result {
            debug!(
                model = model_name,
                duration_ms = duration.as_millis(),
                "Received success response from LLM"
            );
            trace!(response = ?resp, "Full LLM Response");
        } else if let Err(e) = &result {
            warn!(
                model = model_name,
                duration_ms = duration.as_millis(),
                error = %e,
                "Received error response from LLM"
            );
        }

        result
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
            let result = provider
                .chat_with_tools(
                    system_prompt,
                    messages,
                    tools,
                    &model_info.id,
                    model_info.max_tokens,
                    json_mode,
                )
                .await;
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

    /// Calculates the delay before the next retry attempt based on the error type.
    /// Returns `None` if the error is not retryable.
    fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<std::time::Duration> {
        const INITIAL_BACKOFF_MS: u64 = 1000;

        match error {
            LlmError::RateLimit { wait_secs, .. } => {
                // If the server provided a wait time, use it (plus a small buffer)
                if let Some(secs) = wait_secs {
                    return Some(std::time::Duration::from_secs(*secs + 1));
                }
                // Otherwise use a more aggressive backoff for rate limits: 10s, 20s, 40s...
                // attempt starts at 1
                let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_secs(backoff_secs))
            }
            LlmError::ApiError(msg) => {
                let msg_lower = msg.to_lowercase();
                if msg_lower.contains("429") {
                    // Treat as rate limit without explicit wait time
                    let backoff_secs = 10u64 * 2u64.pow((attempt - 1) as u32);
                    return Some(std::time::Duration::from_secs(backoff_secs));
                }

                if msg_lower.contains("500")
                    || msg_lower.contains("502")
                    || msg_lower.contains("503")
                    || msg_lower.contains("504")
                    || msg_lower.contains("timeout")
                    || msg_lower.contains("overloaded")
                {
                    let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                    return Some(std::time::Duration::from_millis(backoff_ms));
                }
                None
            }
            LlmError::NetworkError(_) => {
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            _ => None,
        }
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

    /// Transcribe audio with automatic fallback for text-only providers
    /// If the provider returns `ZAI_FALLBACK_TO_GEMINI` error, use `OpenRouter` instead
    ///
    /// # Errors
    ///
    /// Returns any error from the provider.
    pub async fn transcribe_audio_with_fallback(
        &self,
        provider_name: &str,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let provider = self.get_provider(provider_name)?;
        match provider
            .transcribe_audio(audio_bytes.clone(), mime_type, model_id)
            .await
        {
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
                let provider = self.get_provider(media_provider)?;
                provider
                    .transcribe_audio(audio_bytes, mime_type, media_model_id)
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
}
