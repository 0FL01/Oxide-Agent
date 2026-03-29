//! LLM providers and client
//!
//! Provides a unified interface to various LLM providers (Groq, Mistral, Gemini, OpenRouter).

mod common;
pub mod embeddings;
pub mod http_utils;
mod openai_compat;
/// Implementations of specific LLM providers
pub mod providers;

use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
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
    /// Request history is internally inconsistent but can be repaired locally.
    #[error("Repairable history error: {0}")]
    RepairableHistory(String),
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
    /// Legacy tool call id echoed by chat-like providers and persisted for compatibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Canonical correlation metadata for a tool result message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_correlation: Option<ToolCallCorrelation>,
    /// Tool name (for tool responses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls made by the assistant
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Canonical correlation metadata for assistant tool call batches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_correlations: Option<Vec<ToolCallCorrelation>>,
}

impl Message {
    /// Create a new user message
    #[must_use]
    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: None,
            tool_call_correlations: None,
        }
    }

    /// Create a new assistant message
    #[must_use]
    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: None,
            tool_call_correlations: None,
        }
    }

    /// Create a new assistant message with tool calls
    #[must_use]
    pub fn assistant_with_tools(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        let tool_call_correlations = (!tool_calls.is_empty())
            .then(|| tool_calls.iter().map(ToolCall::correlation).collect());
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: Some(tool_calls),
            tool_call_correlations,
        }
    }

    /// Create a new tool response message
    #[must_use]
    pub fn tool(tool_call_id: &str, name: &str, content: &str) -> Self {
        Self::tool_with_correlation(
            tool_call_id,
            ToolCallCorrelation::from_legacy_tool_call_id(tool_call_id),
            name,
            content,
        )
    }

    /// Create a new tool response message with explicit canonical correlation metadata.
    #[must_use]
    pub fn tool_with_correlation(
        tool_call_id: &str,
        tool_call_correlation: ToolCallCorrelation,
        name: &str,
        content: &str,
    ) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_call_id: Some(tool_call_id.to_string()),
            tool_call_correlation: Some(tool_call_correlation),
            name: Some(name.to_string()),
            tool_calls: None,
            tool_call_correlations: None,
        }
    }

    /// Create a new system message
    #[must_use]
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: content.to_string(),
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: None,
            tool_call_correlations: None,
        }
    }

    /// Resolve the canonical correlation for a tool result message.
    #[must_use]
    pub fn resolved_tool_call_correlation(&self) -> Option<ToolCallCorrelation> {
        self.tool_call_correlation.clone().or_else(|| {
            self.tool_call_id
                .as_deref()
                .map(ToolCallCorrelation::from_legacy_tool_call_id)
        })
    }

    /// Resolve canonical correlations for an assistant tool call batch.
    #[must_use]
    pub fn resolved_tool_call_correlations(&self) -> Option<Vec<ToolCallCorrelation>> {
        let tool_calls = self.tool_calls.as_ref()?;
        let derived: Vec<ToolCallCorrelation> =
            tool_calls.iter().map(ToolCall::correlation).collect();

        match &self.tool_call_correlations {
            Some(correlations) if correlations.len() == derived.len() => Some(correlations.clone()),
            _ => Some(derived),
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
    /// Legacy/internal identifier for the tool call.
    pub id: String,
    /// Canonical correlation metadata for provider-specific tool transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_correlation: Option<ToolCallCorrelation>,
    /// Function to be called
    #[serde(rename = "function")]
    pub function: ToolCallFunction,
    /// Whether this tool call was recovered from a malformed LLM response
    #[serde(default)]
    pub is_recovered: bool,
}

impl ToolCall {
    /// Build a legacy tool call with an internal invocation id and no provider metadata.
    #[must_use]
    pub fn new(id: impl Into<String>, function: ToolCallFunction, is_recovered: bool) -> Self {
        Self {
            id: id.into(),
            tool_call_correlation: None,
            function,
            is_recovered,
        }
    }

    /// Attach explicit canonical correlation metadata to the tool call.
    #[must_use]
    pub fn with_correlation(mut self, tool_call_correlation: ToolCallCorrelation) -> Self {
        self.tool_call_correlation = Some(tool_call_correlation);
        self
    }

    /// Resolve the stable runtime invocation id for this tool call.
    #[must_use]
    pub fn invocation_id(&self) -> InvocationId {
        self.tool_call_correlation
            .as_ref()
            .map(|correlation| correlation.invocation_id.clone())
            .unwrap_or_else(|| InvocationId::from(self.id.clone()))
    }

    /// Resolve the canonical correlation for this tool call using the legacy id.
    #[must_use]
    pub fn correlation(&self) -> ToolCallCorrelation {
        self.tool_call_correlation
            .clone()
            .unwrap_or_else(|| ToolCallCorrelation::from_legacy_tool_call_id(self.id.clone()))
    }

    /// Resolve the provider-facing tool call id for outbound history.
    #[must_use]
    pub fn wire_tool_call_id(&self) -> &str {
        self.tool_call_correlation
            .as_ref()
            .map_or_else(|| self.id.as_str(), ToolCallCorrelation::wire_tool_call_id)
    }
}

/// Stable runtime identifier for one tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InvocationId(String);

impl InvocationId {
    /// Build a runtime invocation id from any owned string input.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the invocation id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the id and return the owned string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for InvocationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for InvocationId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for InvocationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<InvocationId> for String {
    fn from(value: InvocationId) -> Self {
        value.0
    }
}

impl fmt::Display for InvocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Opaque provider-owned identifier used to correlate a tool result back to a provider call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderToolCallId(String);

impl ProviderToolCallId {
    /// Build a provider tool-call id from any owned string input.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the provider id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the id and return the owned string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for ProviderToolCallId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ProviderToolCallId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for ProviderToolCallId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<ProviderToolCallId> for String {
    fn from(value: ProviderToolCallId) -> Self {
        value.0
    }
}

impl fmt::Display for ProviderToolCallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Optional provider-owned item identifier for APIs that distinguish item and call ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderItemId(String);

impl ProviderItemId {
    /// Build a provider item id from any owned string input.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the provider item id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the id and return the owned string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for ProviderItemId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ProviderItemId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl AsRef<str> for ProviderItemId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<ProviderItemId> for String {
    fn from(value: ProviderItemId) -> Self {
        value.0
    }
}

impl fmt::Display for ProviderItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Provider protocol family used to encode tool interactions on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolProtocol {
    /// OpenAI-style chat messages with `tool_calls[].id` and `role=tool` responses.
    #[default]
    ChatLike,
    /// OpenAI Responses-style items with distinct item and call identifiers.
    ResponsesLike,
    /// Anthropic-compatible client-side tools with `tool_use`/`tool_result` blocks.
    AnthropicClientTools,
}

/// Execution transport for a tool interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolTransport {
    /// The model emits a tool request and expects the client to send the result back.
    #[default]
    ClientRoundTrip,
    /// The provider executes the tool and returns control after server-side completion.
    ServerExecuted,
}

/// Shared domain correlation record for one tool invocation across runtime and provider layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallCorrelation {
    /// Stable internal identifier used by runtime, memory, retries, and tracing.
    pub invocation_id: InvocationId,
    /// Opaque provider correlation id echoed back in tool results when required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_tool_call_id: Option<ProviderToolCallId>,
    /// Optional provider item id for APIs that return both item and call identifiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<ProviderItemId>,
    /// Provider protocol family responsible for this correlation.
    #[serde(default)]
    pub protocol: ToolProtocol,
    /// Execution transport used for the tool flow.
    #[serde(default)]
    pub transport: ToolTransport,
}

impl ToolCallCorrelation {
    /// Build a correlation record seeded by the stable internal invocation id.
    #[must_use]
    pub fn new(invocation_id: impl Into<InvocationId>) -> Self {
        Self {
            invocation_id: invocation_id.into(),
            provider_tool_call_id: None,
            provider_item_id: None,
            protocol: ToolProtocol::ChatLike,
            transport: ToolTransport::ClientRoundTrip,
        }
    }

    /// Convert a legacy single-string tool call id into the new correlation record.
    #[must_use]
    pub fn from_legacy_tool_call_id(id: impl Into<InvocationId>) -> Self {
        Self::new(id)
    }

    /// Attach the opaque provider correlation id used for outbound tool results.
    #[must_use]
    pub fn with_provider_tool_call_id(
        mut self,
        provider_tool_call_id: impl Into<ProviderToolCallId>,
    ) -> Self {
        self.provider_tool_call_id = Some(provider_tool_call_id.into());
        self
    }

    /// Attach an optional provider item id for responses-like APIs.
    #[must_use]
    pub fn with_provider_item_id(mut self, provider_item_id: impl Into<ProviderItemId>) -> Self {
        self.provider_item_id = Some(provider_item_id.into());
        self
    }

    /// Override the provider protocol family for this correlation.
    #[must_use]
    pub fn with_protocol(mut self, protocol: ToolProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Override how the tool execution round-trip is transported.
    #[must_use]
    pub fn with_transport(mut self, transport: ToolTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Return the provider-facing tool call id, falling back to the invocation id for legacy flows.
    #[must_use]
    pub fn wire_tool_call_id(&self) -> &str {
        match &self.provider_tool_call_id {
            Some(provider_tool_call_id) => provider_tool_call_id.as_str(),
            None => self.invocation_id.as_str(),
        }
    }

    /// Return the legacy internal id used by the current runtime and persisted history.
    #[must_use]
    pub fn legacy_tool_call_id(&self) -> &str {
        self.invocation_id.as_str()
    }
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
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
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

/// Parameters for a tool-enabled chat completion.
#[derive(Debug, Clone, Copy)]
pub struct ChatWithToolsRequest<'a> {
    /// System prompt for the request.
    pub system_prompt: &'a str,
    /// Conversation history.
    pub messages: &'a [Message],
    /// Available tool definitions.
    pub tools: &'a [ToolDefinition],
    /// Provider-specific model identifier.
    pub model_id: &'a str,
    /// Maximum number of output tokens.
    pub max_tokens: u32,
    /// Whether structured JSON mode is required.
    pub json_mode: bool,
}

/// How strictly a provider enforces tool-call history consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolHistoryMode {
    /// Reject only clearly invalid references such as orphaned tool results.
    BestEffort,
    /// Require every tool call batch to have a fully matching set of tool results.
    Strict,
}

/// Provider-specific request behavior relevant to history validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Tool history matching mode enforced before a request is sent.
    pub tool_history_mode: ToolHistoryMode,
}

impl ProviderCapabilities {
    #[must_use]
    /// Returns true when the provider expects exact tool-call/result matching.
    pub const fn strict_tool_history(self) -> bool {
        matches!(self.tool_history_mode, ToolHistoryMode::Strict)
    }

    #[must_use]
    /// Returns a short label for logs and progress updates.
    pub const fn tool_history_label(self) -> &'static str {
        match self.tool_history_mode {
            ToolHistoryMode::BestEffort => "best_effort",
            ToolHistoryMode::Strict => "strict",
        }
    }
}

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
        let capabilities = Self::provider_capabilities(&model_info.provider);

        validate_tool_history(messages, capabilities)?;

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
        let tool_history_mode = match provider_name.to_ascii_lowercase().as_str() {
            "minimax" | "mistral" => ToolHistoryMode::Strict,
            _ => ToolHistoryMode::BestEffort,
        };

        ProviderCapabilities { tool_history_mode }
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
        let capabilities = Self::provider_capabilities(&model_info.provider);

        validate_tool_history(messages, capabilities)?;

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
    pub const MAX_RETRIES: usize = 5;

    /// Calculates the delay before the next retry attempt based on the error type.
    /// Returns `None` if the error is not retryable.
    pub fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<std::time::Duration> {
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
            LlmError::NetworkError(msg) => {
                // "builder" errors indicate a configuration/endpoint problem, not a transient failure.
                if msg.to_lowercase().contains("builder") {
                    return None;
                }
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            LlmError::JsonError(_) => {
                // JSON parsing errors can be transient (bad proxy, network issues,
                // malformed response). Retry with exponential backoff.
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            _ => None,
        }
    }

    /// Returns true if the error is retryable.
    pub fn is_retryable_error(error: &LlmError) -> bool {
        Self::get_retry_delay(error, 1).is_some()
    }

    /// Returns true if the error is a rate limit (429 or RateLimit variant).
    pub fn is_rate_limit_error(error: &LlmError) -> bool {
        match error {
            LlmError::RateLimit { .. } => true,
            LlmError::ApiError(msg) => msg.to_lowercase().contains("429"),
            _ => false,
        }
    }

    /// Returns the wait time in seconds from a rate limit error, if available.
    pub fn get_rate_limit_wait_secs(error: &LlmError) -> Option<u64> {
        match error {
            LlmError::RateLimit { wait_secs, .. } => *wait_secs,
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
        const MAX_RETRIES: usize = 5;

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
                            Self::get_retry_delay_with_initial(&e, attempt, initial_backoff_ms)
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

    /// Calculates the delay before the next retry attempt based on the error type and initial backoff.
    /// Returns `None` if the error is not retryable.
    fn get_retry_delay_with_initial(
        error: &LlmError,
        attempt: usize,
        initial_backoff_ms: u64,
    ) -> Option<std::time::Duration> {
        match error {
            LlmError::RateLimit { wait_secs, .. } => {
                // If the server provided a wait time, use it (plus a small buffer)
                if let Some(secs) = wait_secs {
                    return Some(std::time::Duration::from_secs(*secs + 1));
                }
                // Otherwise use exponential backoff based on initial value
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            LlmError::ApiError(msg) => {
                let msg_lower = msg.to_lowercase();
                if msg_lower.contains("429") {
                    let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                    return Some(std::time::Duration::from_millis(backoff_ms));
                }
                if msg_lower.contains("500")
                    || msg_lower.contains("502")
                    || msg_lower.contains("503")
                    || msg_lower.contains("504")
                    || msg_lower.contains("timeout")
                    || msg_lower.contains("overloaded")
                {
                    let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                    return Some(std::time::Duration::from_millis(backoff_ms));
                }
                None
            }
            LlmError::NetworkError(msg) => {
                // Don't retry DNS or connection refused errors immediately
                let msg_lower = msg.to_lowercase();
                if msg_lower.contains("dns")
                    || msg_lower.contains("refused")
                    || msg_lower.contains("reset")
                {
                    let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                    return Some(std::time::Duration::from_millis(backoff_ms));
                }
                // Retry other network errors with backoff
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            LlmError::JsonError(_) => {
                // JSON errors might be transient, retry with backoff
                let backoff_ms = initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                Some(std::time::Duration::from_millis(backoff_ms))
            }
            _ => None,
        }
    }
}

/// Extract and validate invocation IDs from an assistant message's tool calls.
fn extract_expected_invocation_ids(message: &Message) -> Result<HashSet<InvocationId>, LlmError> {
    let mut expected_ids = HashSet::new();

    for correlation in message
        .resolved_tool_call_correlations()
        .unwrap_or_default()
    {
        let invocation_id = correlation.invocation_id.as_str().trim();
        if invocation_id.is_empty() {
            return Err(LlmError::RepairableHistory(
                "assistant tool call has an empty invocation_id".to_string(),
            ));
        }
        if !expected_ids.insert(correlation.invocation_id.clone()) {
            return Err(LlmError::RepairableHistory(format!(
                "assistant tool call batch contains duplicate invocation_id `{}`",
                correlation.invocation_id
            )));
        }
        if has_empty_explicit_provider_tool_call_id(&correlation) {
            return Err(LlmError::RepairableHistory(format!(
                "assistant tool call `{}` has an empty provider_tool_call_id",
                correlation.invocation_id
            )));
        }
    }

    Ok(expected_ids)
}

/// Validate a sequence of tool result messages following an assistant batch.
fn validate_tool_result_sequence(
    messages: &[Message],
    start_index: usize,
    expected_ids: &HashSet<InvocationId>,
) -> Result<(usize, HashSet<InvocationId>), LlmError> {
    let mut seen_results = HashSet::new();
    let mut cursor = start_index;

    while cursor < messages.len() && messages[cursor].role == "tool" {
        let result = &messages[cursor];
        let Some(result_correlation) = result.resolved_tool_call_correlation() else {
            return Err(LlmError::RepairableHistory(
                "tool result is missing invocation_id".to_string(),
            ));
        };

        if has_empty_explicit_provider_tool_call_id(&result_correlation) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result for invocation_id `{}` has an empty provider_tool_call_id",
                result_correlation.invocation_id
            )));
        }

        let Some(invocation_id) = Some(result_correlation.invocation_id.clone())
            .filter(|id| !id.as_str().trim().is_empty())
        else {
            return Err(LlmError::RepairableHistory(
                "tool result is missing invocation_id".to_string(),
            ));
        };

        if !expected_ids.contains(&invocation_id) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result references unknown invocation_id `{invocation_id}`"
            )));
        }

        if !seen_results.insert(invocation_id.clone()) {
            return Err(LlmError::RepairableHistory(format!(
                "tool result for invocation_id `{invocation_id}` is duplicated"
            )));
        }

        cursor += 1;
    }

    Ok((cursor, seen_results))
}

/// Check batch completion policy and return error if incomplete.
fn check_batch_completion(
    cursor: usize,
    messages_len: usize,
    expected_ids: &HashSet<InvocationId>,
    seen_results: &HashSet<InvocationId>,
    capabilities: ProviderCapabilities,
) -> Result<(), LlmError> {
    let batch_is_terminal = cursor == messages_len;
    let should_require_complete_batch = capabilities.strict_tool_history() || !batch_is_terminal;

    if should_require_complete_batch && seen_results.len() != expected_ids.len() {
        return Err(LlmError::RepairableHistory(format!(
            "assistant tool call batch is incomplete for {} tool history: {} tool calls but {} tool results",
            capabilities.tool_history_label(),
            expected_ids.len(),
            seen_results.len()
        )));
    }

    Ok(())
}

/// Generate error detail for an orphaned tool result message.
fn orphaned_tool_result_error(message: &Message) -> LlmError {
    let detail = message
        .resolved_tool_call_correlation()
        .map(|correlation| correlation.invocation_id)
        .filter(|id| !id.as_str().trim().is_empty())
        .map_or_else(
            || "orphaned tool result without invocation_id".to_string(),
            |invocation_id| {
                format!(
                    "orphaned tool result references missing assistant tool call `{invocation_id}`"
                )
            },
        );
    LlmError::RepairableHistory(detail)
}

fn validate_tool_history(
    messages: &[Message],
    capabilities: ProviderCapabilities,
) -> Result<(), LlmError> {
    let mut index = 0;

    while index < messages.len() {
        let message = &messages[index];

        if message.role == "assistant" {
            if let Some(tool_calls) = &message.tool_calls {
                if tool_calls.is_empty() {
                    return Err(LlmError::RepairableHistory(
                        "assistant tool call batch is empty".to_string(),
                    ));
                }

                let expected_ids = extract_expected_invocation_ids(message)?;
                let (cursor, seen_results) =
                    validate_tool_result_sequence(messages, index + 1, &expected_ids)?;
                check_batch_completion(
                    cursor,
                    messages.len(),
                    &expected_ids,
                    &seen_results,
                    capabilities,
                )?;

                index = cursor;
                continue;
            }
        }

        if message.role == "tool" {
            return Err(orphaned_tool_result_error(message));
        }

        index += 1;
    }

    Ok(())
}

fn has_empty_explicit_provider_tool_call_id(correlation: &ToolCallCorrelation) -> bool {
    correlation
        .provider_tool_call_id
        .as_ref()
        .is_some_and(|provider_tool_call_id| provider_tool_call_id.as_str().trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        validate_tool_history, InvocationId, LlmError, Message, ProviderCapabilities,
        ProviderItemId, ProviderToolCallId, ToolCall, ToolCallCorrelation, ToolCallFunction,
        ToolHistoryMode, ToolProtocol, ToolTransport,
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
    fn validate_tool_history_rejects_orphaned_tool_result() {
        let messages = vec![
            Message::user("hi"),
            Message::tool("call-1", "search", "result"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_incomplete_parallel_batch() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_duplicate_tool_call_ids_in_assistant_batch() {
        let messages = vec![Message::assistant_with_tools(
            "calling tools",
            vec![
                tool_call("call-1", "search"),
                tool_call("call-1", "read_file"),
            ],
        )];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_duplicate_tool_results_for_same_call() {
        let messages = vec![
            Message::assistant_with_tools("calling tools", vec![tool_call("call-1", "search")]),
            Message::tool("call-1", "search", "result-1"),
            Message::tool("call-1", "search", "result-2"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_allows_terminal_open_batch_for_best_effort_provider() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
        ];

        let result = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::BestEffort,
            },
        );

        assert!(
            result.is_ok(),
            "best-effort providers should allow an open terminal batch"
        );
    }

    #[test]
    fn validate_tool_history_rejects_nonterminal_open_batch_even_for_best_effort_provider() {
        let messages = vec![
            Message::assistant_with_tools(
                "calling tools",
                vec![
                    tool_call("call-1", "search"),
                    tool_call("call-2", "read_file"),
                ],
            ),
            Message::tool("call-1", "search", "result"),
            Message::user("follow up"),
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::BestEffort,
            },
        )
        .expect_err("history must be rejected");
        assert!(matches!(error, LlmError::RepairableHistory(_)));
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

    #[test]
    fn validate_tool_history_matches_on_invocation_id_not_raw_wire_id() {
        let correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "calling tools".to_string(),
                tool_call_id: None,
                tool_call_correlation: None,
                name: None,
                tool_calls: Some(vec![tool_call("provider-a", "search")]),
                tool_call_correlations: Some(vec![correlation.clone()]),
            },
            Message {
                role: "tool".to_string(),
                content: "result".to_string(),
                tool_call_id: Some("provider-b".to_string()),
                tool_call_correlation: Some(correlation),
                name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
            },
        ];

        let result = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        );

        assert!(
            result.is_ok(),
            "canonical invocation ids should drive matching"
        );
    }

    #[test]
    fn validate_tool_history_rejects_empty_explicit_provider_tool_call_id_in_assistant_batch() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: "calling tools".to_string(),
            tool_call_id: None,
            tool_call_correlation: None,
            name: None,
            tool_calls: Some(vec![tool_call("call-1", "search")]),
            tool_call_correlations: Some(vec![
                ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("")
            ]),
        }];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }

    #[test]
    fn validate_tool_history_rejects_empty_explicit_provider_tool_call_id_in_tool_result() {
        let assistant_correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("provider-call-1");
        let tool_result_correlation =
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("");
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: "calling tools".to_string(),
                tool_call_id: None,
                tool_call_correlation: None,
                name: None,
                tool_calls: Some(vec![tool_call("call-1", "search")]),
                tool_call_correlations: Some(vec![assistant_correlation]),
            },
            Message {
                role: "tool".to_string(),
                content: "result".to_string(),
                tool_call_id: Some("invoke-1".to_string()),
                tool_call_correlation: Some(tool_result_correlation),
                name: Some("search".to_string()),
                tool_calls: None,
                tool_call_correlations: None,
            },
        ];

        let error = validate_tool_history(
            &messages,
            ProviderCapabilities {
                tool_history_mode: ToolHistoryMode::Strict,
            },
        )
        .expect_err("history must be rejected");

        assert!(matches!(error, LlmError::RepairableHistory(_)));
    }
}
