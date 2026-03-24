use crate::llm::providers::tool_correlation::ToolCorrelationNormalizer;
use crate::llm::{Message, ToolProtocol, ToolTransport};

/// Protocol-shaped outbound tool result envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodedToolResult {
    /// Chat-like `role=tool` message payload.
    ChatLike(ChatLikeToolResult),
    /// Responses-style `function_call_output` item payload.
    ResponsesLike(ResponsesLikeToolResult),
    /// Anthropic-compatible `tool_result` content block payload.
    Anthropic(AnthropicToolResult),
}

impl EncodedToolResult {
    /// Extract the chat-like envelope when available.
    #[must_use]
    pub fn into_chat_like(self) -> Option<ChatLikeToolResult> {
        match self {
            Self::ChatLike(result) => Some(result),
            Self::ResponsesLike(_) => None,
            Self::Anthropic(_) => None,
        }
    }

    /// Extract the Anthropic-compatible envelope when available.
    #[must_use]
    pub fn into_anthropic(self) -> Option<AnthropicToolResult> {
        match self {
            Self::Anthropic(result) => Some(result),
            Self::ResponsesLike(_) | Self::ChatLike(_) => None,
        }
    }

    /// Extract the Responses-style envelope when available.
    #[must_use]
    pub fn into_responses_like(self) -> Option<ResponsesLikeToolResult> {
        match self {
            Self::ResponsesLike(result) => Some(result),
            Self::ChatLike(_) => None,
            Self::Anthropic(_) => None,
        }
    }
}

/// Chat-style outbound tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLikeToolResult {
    /// Opaque provider tool call id echoed back on the wire.
    pub tool_call_id: String,
    /// Optional tool name carried by some chat-like providers.
    pub name: Option<String>,
    /// Tool output payload serialized as a string.
    pub content: String,
}

/// Responses-compatible outbound tool result item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsesLikeToolResult {
    /// Optional provider item id for APIs that separate item and call ids.
    pub item_id: Option<String>,
    /// Opaque provider call id echoed back on the wire.
    pub call_id: String,
    /// Tool output payload serialized as a string.
    pub output: String,
}

/// Anthropic-compatible outbound tool result block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnthropicToolResult {
    /// Opaque provider tool-use id echoed back on the wire.
    pub tool_use_id: String,
    /// Tool output payload serialized as a string.
    pub content: String,
    /// Optional error flag for tool execution failures.
    pub is_error: Option<bool>,
}

/// Provider-local encoder for outbound tool result envelopes.
pub trait ToolResultEncoder {
    /// Encode a runtime tool result message into a provider-shaped envelope.
    fn encode(self, message: &Message) -> Option<EncodedToolResult>;
}

/// Protocol-aware encoder for outbound tool result wire payloads.
#[derive(Debug, Clone, Copy)]
pub struct ProviderToolResultEncoder {
    protocol: ToolProtocol,
    transport: ToolTransport,
}

impl ProviderToolResultEncoder {
    /// Build an encoder for one provider protocol family.
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }
}

impl ToolResultEncoder for ProviderToolResultEncoder {
    fn encode(self, message: &Message) -> Option<EncodedToolResult> {
        if message.role != "tool" || matches!(self.transport, ToolTransport::ServerExecuted) {
            return None;
        }

        let correlation = ToolCorrelationNormalizer::new(self.protocol, self.transport)
            .normalize(message.resolved_tool_call_correlation()?);
        let wire_id = correlation.wire_tool_call_id().to_string();

        match self.protocol {
            ToolProtocol::ChatLike => Some(EncodedToolResult::ChatLike(ChatLikeToolResult {
                tool_call_id: wire_id,
                name: message.name.clone(),
                content: message.content.clone(),
            })),
            ToolProtocol::ResponsesLike => {
                Some(EncodedToolResult::ResponsesLike(ResponsesLikeToolResult {
                    item_id: correlation
                        .provider_item_id
                        .as_ref()
                        .map(|item_id| item_id.as_str().to_string()),
                    call_id: wire_id,
                    output: message.content.clone(),
                }))
            }
            ToolProtocol::AnthropicClientTools => {
                Some(EncodedToolResult::Anthropic(AnthropicToolResult {
                    tool_use_id: wire_id,
                    content: message.content.clone(),
                    is_error: None,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EncodedToolResult, ProviderToolResultEncoder, ToolResultEncoder};
    use crate::llm::{Message, ToolCallCorrelation, ToolProtocol, ToolTransport};

    #[test]
    fn chat_like_encoder_uses_provider_wire_id() {
        let encoder =
            ProviderToolResultEncoder::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);
        let message = Message::tool_with_correlation(
            "invoke-1",
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("wire-1"),
            "search",
            "done",
        );

        let encoded = encoder.encode(&message).expect("tool result encodes");

        assert_eq!(
            encoded.into_chat_like(),
            Some(super::ChatLikeToolResult {
                tool_call_id: "wire-1".to_string(),
                name: Some("search".to_string()),
                content: "done".to_string(),
            })
        );
    }

    #[test]
    fn anthropic_encoder_builds_tool_result_block() {
        let encoder = ProviderToolResultEncoder::new(
            ToolProtocol::AnthropicClientTools,
            ToolTransport::ClientRoundTrip,
        );
        let message = Message::tool_with_correlation(
            "invoke-2",
            ToolCallCorrelation::new("invoke-2")
                .with_provider_tool_call_id("toolu_2")
                .with_protocol(ToolProtocol::AnthropicClientTools),
            "search",
            "done",
        );

        let encoded = encoder.encode(&message).expect("tool result encodes");

        assert_eq!(
            encoded.into_anthropic(),
            Some(super::AnthropicToolResult {
                tool_use_id: "toolu_2".to_string(),
                content: "done".to_string(),
                is_error: None,
            })
        );
    }

    #[test]
    fn responses_encoder_preserves_provider_item_and_call_ids() {
        let encoder = ProviderToolResultEncoder::new(
            ToolProtocol::ResponsesLike,
            ToolTransport::ClientRoundTrip,
        );
        let message = Message::tool_with_correlation(
            "invoke-3",
            ToolCallCorrelation::new("invoke-3")
                .with_provider_tool_call_id("call-3")
                .with_provider_item_id("item-3")
                .with_protocol(ToolProtocol::ResponsesLike),
            "search",
            "done",
        );

        let encoded = encoder.encode(&message).expect("tool result encodes");

        assert_eq!(
            encoded.into_responses_like(),
            Some(super::ResponsesLikeToolResult {
                item_id: Some("item-3".to_string()),
                call_id: "call-3".to_string(),
                output: "done".to_string(),
            })
        );
    }

    #[test]
    fn anthropic_encoder_upgrades_legacy_tool_messages_to_provider_protocol() {
        let encoder = ProviderToolResultEncoder::new(
            ToolProtocol::AnthropicClientTools,
            ToolTransport::ClientRoundTrip,
        );
        let message = Message::tool("legacy-call", "search", "done");

        let encoded = encoder.encode(&message).expect("tool result encodes");

        assert_eq!(
            encoded,
            EncodedToolResult::Anthropic(super::AnthropicToolResult {
                tool_use_id: "legacy-call".to_string(),
                content: "done".to_string(),
                is_error: None,
            })
        );
    }

    #[test]
    fn encoder_skips_server_executed_tool_transports() {
        let encoder =
            ProviderToolResultEncoder::new(ToolProtocol::ChatLike, ToolTransport::ServerExecuted);
        let message = Message::tool("call-1", "search", "done");

        assert!(encoder.encode(&message).is_none());
    }
}
