use crate::llm::{ToolCall, ToolCallCorrelation, ToolProtocol, ToolTransport};
use serde_json::Value;

/// Protocol-shaped outbound assistant tool-call envelope.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodedAssistantToolCall {
    /// Chat-like `tool_calls[].function` payload.
    ChatLike(ChatLikeAssistantToolCall),
    /// Responses-style function-call item payload.
    ResponsesLike(ResponsesLikeAssistantToolCall),
    /// Anthropic-compatible `tool_use` payload.
    Anthropic(AnthropicAssistantToolCall),
}

impl EncodedAssistantToolCall {
    /// Extract the chat-like payload when available.
    #[must_use]
    pub fn into_chat_like(self) -> Option<ChatLikeAssistantToolCall> {
        match self {
            Self::ChatLike(call) => Some(call),
            Self::ResponsesLike(_) | Self::Anthropic(_) => None,
        }
    }

    /// Extract the responses-style payload when available.
    #[must_use]
    pub fn into_responses_like(self) -> Option<ResponsesLikeAssistantToolCall> {
        match self {
            Self::ResponsesLike(call) => Some(call),
            Self::ChatLike(_) | Self::Anthropic(_) => None,
        }
    }

    /// Extract the anthropic-compatible payload when available.
    #[must_use]
    pub fn into_anthropic(self) -> Option<AnthropicAssistantToolCall> {
        match self {
            Self::Anthropic(call) => Some(call),
            Self::ChatLike(_) | Self::ResponsesLike(_) => None,
        }
    }
}

/// Chat-like outbound assistant tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLikeAssistantToolCall {
    /// Opaque provider tool call id echoed on the wire.
    pub id: String,
    /// Tool function name.
    pub name: String,
    /// Serialized JSON arguments string.
    pub arguments: String,
}

/// Responses-compatible outbound assistant tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponsesLikeAssistantToolCall {
    /// Optional provider item id for APIs that distinguish item and call ids.
    pub item_id: Option<String>,
    /// Opaque provider call id echoed on the wire.
    pub call_id: String,
    /// Tool function name.
    pub name: String,
    /// Serialized JSON arguments string.
    pub arguments: String,
}

/// Anthropic-compatible outbound assistant tool use block.
#[derive(Debug, Clone, PartialEq)]
pub struct AnthropicAssistantToolCall {
    /// Opaque provider tool use id echoed on the wire.
    pub id: String,
    /// Tool function name.
    pub name: String,
    /// Parsed JSON input payload.
    pub input: Value,
}

/// Provider-local encoder for outbound assistant tool-call envelopes.
pub trait ToolCallEncoder {
    /// Encode one runtime tool call into a provider-shaped envelope.
    fn encode(self, tool_call: &ToolCall) -> Option<EncodedAssistantToolCall>;
}

/// Protocol-aware encoder for outbound assistant tool calls.
#[derive(Debug, Clone, Copy)]
pub struct ProviderToolCallEncoder {
    protocol: ToolProtocol,
    transport: ToolTransport,
}

impl ProviderToolCallEncoder {
    /// Build an encoder for one provider protocol family.
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    fn normalize_outbound_correlation(
        self,
        correlation: ToolCallCorrelation,
    ) -> ToolCallCorrelation {
        let mut normalized = correlation
            .with_protocol(self.protocol)
            .with_transport(self.transport);

        if normalized.provider_tool_call_id.is_none() {
            let invocation_id = normalized.invocation_id.as_str().to_string();
            normalized = normalized.with_provider_tool_call_id(invocation_id);
        }

        normalized
    }
}

impl ToolCallEncoder for ProviderToolCallEncoder {
    fn encode(self, tool_call: &ToolCall) -> Option<EncodedAssistantToolCall> {
        if matches!(self.transport, ToolTransport::ServerExecuted) {
            return None;
        }

        let correlation = self.normalize_outbound_correlation(tool_call.correlation());
        let wire_id = correlation.wire_tool_call_id().to_string();

        match self.protocol {
            ToolProtocol::ChatLike => Some(EncodedAssistantToolCall::ChatLike(
                ChatLikeAssistantToolCall {
                    id: wire_id,
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                },
            )),
            ToolProtocol::ResponsesLike => Some(EncodedAssistantToolCall::ResponsesLike(
                ResponsesLikeAssistantToolCall {
                    item_id: correlation
                        .provider_item_id
                        .as_ref()
                        .map(|item_id| item_id.as_str().to_string()),
                    call_id: wire_id,
                    name: tool_call.function.name.clone(),
                    arguments: tool_call.function.arguments.clone(),
                },
            )),
            ToolProtocol::AnthropicClientTools => {
                let input = serde_json::from_str(&tool_call.function.arguments)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                Some(EncodedAssistantToolCall::Anthropic(
                    AnthropicAssistantToolCall {
                        id: wire_id,
                        name: tool_call.function.name.clone(),
                        input,
                    },
                ))
            }
            ToolProtocol::AnthropicServerTools | ToolProtocol::GeminiNative => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EncodedAssistantToolCall, ProviderToolCallEncoder, ToolCallEncoder};
    use crate::llm::{
        ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol, ToolTransport,
    };
    use serde_json::json;

    #[test]
    fn chat_like_encoder_uses_provider_wire_id() {
        let encoder =
            ProviderToolCallEncoder::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);
        let tool_call = ToolCall::new(
            "invoke-1",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: r#"{"query":"oxide"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-1").with_provider_tool_call_id("wire-1"),
        );

        let encoded = encoder.encode(&tool_call).expect("tool call encodes");

        assert_eq!(
            encoded.into_chat_like(),
            Some(super::ChatLikeAssistantToolCall {
                id: "wire-1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query":"oxide"}"#.to_string(),
            })
        );
    }

    #[test]
    fn responses_encoder_preserves_item_and_call_ids() {
        let encoder = ProviderToolCallEncoder::new(
            ToolProtocol::ResponsesLike,
            ToolTransport::ClientRoundTrip,
        );
        let tool_call = ToolCall::new(
            "invoke-2",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-2")
                .with_provider_tool_call_id("call-2")
                .with_provider_item_id("item-2")
                .with_protocol(ToolProtocol::ResponsesLike),
        );

        let encoded = encoder.encode(&tool_call).expect("tool call encodes");

        assert_eq!(
            encoded.into_responses_like(),
            Some(super::ResponsesLikeAssistantToolCall {
                item_id: Some("item-2".to_string()),
                call_id: "call-2".to_string(),
                name: "search".to_string(),
                arguments: "{}".to_string(),
            })
        );
    }

    #[test]
    fn anthropic_encoder_builds_tool_use_input() {
        let encoder = ProviderToolCallEncoder::new(
            ToolProtocol::AnthropicClientTools,
            ToolTransport::ClientRoundTrip,
        );
        let tool_call = ToolCall::new(
            "invoke-3",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: r#"{"query":"oxide"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-3")
                .with_provider_tool_call_id("toolu_3")
                .with_protocol(ToolProtocol::AnthropicClientTools),
        );

        let encoded = encoder.encode(&tool_call).expect("tool call encodes");

        assert_eq!(
            encoded.into_anthropic(),
            Some(super::AnthropicAssistantToolCall {
                id: "toolu_3".to_string(),
                name: "search".to_string(),
                input: json!({"query": "oxide"}),
            })
        );
    }

    #[test]
    fn encoder_skips_server_executed_transports() {
        let encoder = ProviderToolCallEncoder::new(
            ToolProtocol::AnthropicServerTools,
            ToolTransport::ServerExecuted,
        );
        let tool_call = ToolCall::new(
            "invoke-4",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        );

        assert_eq!(encoder.encode(&tool_call), None);
        assert_eq!(
            EncodedAssistantToolCall::ChatLike(super::ChatLikeAssistantToolCall {
                id: "x".to_string(),
                name: "y".to_string(),
                arguments: "{}".to_string(),
            })
            .into_responses_like(),
            None
        );
    }
}
