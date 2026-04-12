use std::fmt;

use serde::{Deserialize, Serialize};

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
    /// Optional temperature override for the request.
    pub temperature: Option<f32>,
    /// Whether structured JSON mode is required.
    pub json_mode: bool,
}
