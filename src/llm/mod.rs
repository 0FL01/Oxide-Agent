//! LLM providers and client
//!
//! Provides a unified interface to various LLM providers (Groq, Mistral, Gemini, OpenRouter).

mod common;
mod http_utils;
mod openai_compat;
/// Implementations of specific LLM providers
pub mod providers;

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
    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_call_id: Some(tool_call_id.to_string()),
            name: Some(name.to_string()),
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
}

/// Function details within a tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    /// Name of the function being called
    pub name: String,
    /// Arguments for the function call (JSON string)
    pub arguments: String,
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
}

/// Interface for all LLM providers
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
}

/// Unified client for interacting with multiple LLM providers
pub struct LlmClient {
    groq: Option<providers::GroqProvider>,
    mistral: Option<providers::MistralProvider>,
    zai: Option<providers::ZaiProvider>,
    gemini: Option<providers::GeminiProvider>,
    openrouter: Option<providers::OpenRouterProvider>,
}

impl LlmClient {
    /// Create a new LLM client with providers configured from settings
    #[must_use]
    pub fn new(settings: &crate::config::Settings) -> Self {
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
                .map(|k| providers::ZaiProvider::new(k.clone())),
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
        }
    }

    /// Returns the provider for the given name
    ///
    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if the provider is not configured.
    fn get_provider(&self, provider_name: &str) -> Result<&dyn LlmProvider, LlmError> {
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
        use crate::config::MODELS;

        let model_info = MODELS
            .iter()
            .find(|(name, _)| *name == model_name)
            .map(|(_, info)| info)
            .ok_or_else(|| LlmError::Unknown(format!("Model {model_name} not found")))?;

        let provider = self.get_provider(model_info.provider)?;

        // Special case for OpenRouter with retry/fallback as in Python
        if model_name == "OR Gemini 3 Flash" && model_info.provider == "openrouter" {
            debug!("Using OpenRouter fallback logic for Gemini 3 Flash");
            return self
                .openrouter_chat_with_fallback(system_prompt, history, user_message)
                .await;
        }

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
                model_info.id,
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
    /// Currently only supported by Mistral provider (Devstral model)
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found, if tool calling is not supported for the provider,
    /// or any error from the provider.
    #[instrument(skip(self, system_prompt, messages, tools))]
    pub async fn chat_with_tools(
        &self,
        system_prompt: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        model_name: &str,
    ) -> Result<ChatResponse, LlmError> {
        use crate::config::MODELS;

        let model_info = MODELS
            .iter()
            .find(|(name, _)| *name == model_name)
            .map(|(_, info)| info)
            .ok_or_else(|| LlmError::Unknown(format!("Model {model_name} not found")))?;

        // Only Mistral provider supports tool calling currently
        if model_info.provider != "mistral" {
            let provider = model_info.provider;
            return Err(LlmError::Unknown(format!(
                "Tool calling not supported for provider: {provider}"
            )));
        }

        let mistral = self
            .mistral
            .as_ref()
            .ok_or_else(|| LlmError::MissingConfig("mistral".to_string()))?;

        debug!(
            model = model_name,
            tools_count = tools.len(),
            messages_count = messages.len(),
            "Sending tool-enabled request to LLM"
        );

        let start = std::time::Instant::now();
        let result = mistral
            .chat_with_tools(
                system_prompt,
                messages,
                tools,
                model_info.id,
                model_info.max_tokens,
            )
            .await;
        let duration = start.elapsed();

        if let Ok(resp) = &result {
            debug!(
                model = model_name,
                duration_ms = duration.as_millis(),
                tool_calls_count = resp.tool_calls.len(),
                finish_reason = %resp.finish_reason,
                "Received tool response from LLM"
            );
        } else if let Err(e) = &result {
            warn!(
                model = model_name,
                duration_ms = duration.as_millis(),
                error = %e,
                "Tool-enabled LLM request failed"
            );
        }

        result
    }

    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if `OpenRouter` is not configured, or any error from the provider.
    async fn openrouter_chat_with_fallback(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
    ) -> Result<String, LlmError> {
        let provider = self
            .openrouter
            .as_ref()
            .ok_or_else(|| LlmError::MissingConfig("openrouter".to_string()))?;

        // Primary model: google/gemini-3-flash-preview
        let primary_model = "google/gemini-3-flash-preview";
        let fallback_model = "google/gemini-2.5-flash";

        for attempt in 1..=3 {
            let start = std::time::Instant::now();
            info!("OpenRouter: Attempting with {primary_model}, attempt {attempt}/3");
            match provider
                .chat_completion(system_prompt, history, user_message, primary_model, 64000)
                .await
            {
                Ok(res) => {
                    info!(
                        model = primary_model,
                        duration_ms = start.elapsed().as_millis(),
                        "OpenRouter: Success on attempt {attempt}"
                    );
                    return Ok(res);
                }
                Err(e) => {
                    warn!(
                        model = primary_model,
                        duration_ms = start.elapsed().as_millis(),
                        error = %e,
                        "OpenRouter: Error on attempt {attempt}"
                    );
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }

        info!(
            "OpenRouter: All attempts with {primary_model} failed, switching to {fallback_model}"
        );

        for attempt in 1..=5 {
            info!("OpenRouter: Attempting with {fallback_model}, attempt {attempt}/5");
            match provider
                .chat_completion(system_prompt, history, user_message, fallback_model, 64000)
                .await
            {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("OpenRouter: Error with {fallback_model} on attempt {attempt}: {e}");
                    // Check if it's a retryable error
                    let err_str = e.to_string().to_lowercase();
                    if (err_str.contains("503")
                        || err_str.contains("429")
                        || err_str.contains("500")
                        || err_str.contains("overloaded")
                        || err_str.contains("unavailable")
                        || err_str.contains("timeout"))
                        && attempt < 5
                    {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }

        Err(LlmError::ApiError(
            "All fallback attempts failed".to_string(),
        ))
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
        // As per Python logic, transcribe usually uses Gemini or OpenRouter
        // We'll use the provider associated with the model, or fallback if it's the "retry_with_model_fallback" case
        // In Python it's a decorator, here we just implement it.

        if model_name == "Gemini 2.5 Flash Lite" {
            return self
                .gemini_transcribe_with_fallback(audio_bytes, mime_type)
                .await;
        }

        let model_info = Self::get_model_info(model_name)?;
        let provider = self.get_provider(model_info.provider)?;
        provider
            .transcribe_audio(audio_bytes, mime_type, model_info.id)
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
                // Fallback to OpenRouter with Gemini 3 Flash for transcription
                info!("ZAI does not support audio, falling back to OpenRouter with Gemini 3 Flash");
                let openrouter = self
                    .openrouter
                    .as_ref()
                    .ok_or_else(|| LlmError::MissingConfig("openrouter".to_string()))?;
                let fallback_model = "google/gemini-3-flash-preview";
                openrouter
                    .transcribe_audio(audio_bytes, mime_type, fallback_model)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    /// # Errors
    ///
    /// Returns `LlmError::MissingConfig` if Gemini is not configured, or any error from the provider.
    async fn gemini_transcribe_with_fallback(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
    ) -> Result<String, LlmError> {
        let provider = self
            .gemini
            .as_ref()
            .ok_or_else(|| LlmError::MissingConfig("gemini".to_string()))?;

        let primary_model = "gemini-flash-latest";
        let fallback_model = "gemini-2.5-flash";

        for attempt in 1..=3 {
            match provider
                .transcribe_audio(audio_bytes.clone(), mime_type, primary_model)
                .await
            {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("Gemini transcription error (primary {primary_model}): {e}");
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }

        for attempt in 1..=5 {
            match provider
                .transcribe_audio(audio_bytes.clone(), mime_type, fallback_model)
                .await
            {
                Ok(res) => return Ok(res),
                Err(e) => {
                    warn!("Gemini transcription error (fallback {fallback_model}): {e}");
                    if attempt < 5 {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        Err(LlmError::ApiError(
            "All transcription fallback attempts failed".to_string(),
        ))
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
        let model_info = Self::get_model_info(model_name)?;
        let provider = self.get_provider(model_info.provider)?;
        provider
            .analyze_image(image_bytes, text_prompt, system_prompt, model_info.id)
            .await
    }

    /// Returns the model info for the given name
    ///
    /// # Errors
    ///
    /// Returns `LlmError::Unknown` if the model is not found.
    fn get_model_info(model_name: &str) -> Result<&'static crate::config::ModelInfo, LlmError> {
        crate::config::MODELS
            .iter()
            .find(|(name, _)| *name == model_name)
            .map(|(_, info)| info)
            .ok_or_else(|| LlmError::Unknown(format!("Model {model_name} not found")))
    }
}
