use super::tool_call_adapter::ProviderToolCallAdapter;
use super::tool_call_encoder::{
    EncodedAssistantToolCall, ProviderToolCallEncoder, ToolCallEncoder,
};
#[cfg(test)]
use super::tool_correlation::ToolCorrelationNormalizer;
use super::tool_result_encoder::{EncodedToolResult, ProviderToolResultEncoder, ToolResultEncoder};
#[cfg(test)]
use crate::llm::ToolCallCorrelation;
use crate::llm::{InvocationId, Message, ToolCall, ToolProtocol, ToolTransport};

/// Unified tool protocol behavior profile for one provider wire family.
#[derive(Debug, Clone, Copy)]
pub struct ToolProtocolProfile {
    adapter: ProviderToolCallAdapter,
    tool_call_encoder: ProviderToolCallEncoder,
    tool_result_encoder: ProviderToolResultEncoder,
    #[cfg(test)]
    correlation_normalizer: ToolCorrelationNormalizer,
}

impl ToolProtocolProfile {
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            adapter: ProviderToolCallAdapter::new(protocol, transport),
            tool_call_encoder: ProviderToolCallEncoder::new(protocol, transport),
            tool_result_encoder: ProviderToolResultEncoder::new(protocol, transport),
            #[cfg(test)]
            correlation_normalizer: ToolCorrelationNormalizer::new(protocol, transport),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn normalize_correlation(self, correlation: ToolCallCorrelation) -> ToolCallCorrelation {
        self.correlation_normalizer.normalize(correlation)
    }

    #[must_use]
    pub fn encode_tool_call(self, tool_call: &ToolCall) -> Option<EncodedAssistantToolCall> {
        self.tool_call_encoder.encode(tool_call)
    }

    #[must_use]
    pub fn encode_tool_result(self, message: &Message) -> Option<EncodedToolResult> {
        self.tool_result_encoder.encode(message)
    }

    #[must_use]
    pub fn inbound_tool_call(
        self,
        invocation_id: impl Into<InvocationId>,
        provider_tool_call_id: Option<&str>,
        provider_item_id: Option<&str>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        self.adapter.inbound_tool_call(
            invocation_id,
            provider_tool_call_id,
            provider_item_id,
            name,
            arguments,
        )
    }

    #[must_use]
    pub fn inbound_provider_tool_call(
        self,
        provider_tool_call_id: &str,
        provider_item_id: Option<&str>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        self.adapter.inbound_provider_tool_call(
            provider_tool_call_id,
            provider_item_id,
            name,
            arguments,
        )
    }

    #[must_use]
    pub fn inbound_uncorrelated_tool_call(
        self,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        self.adapter.inbound_uncorrelated_tool_call(name, arguments)
    }
}

pub const CHAT_LIKE_TOOL_PROFILE: ToolProtocolProfile =
    ToolProtocolProfile::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

pub const ANTHROPIC_CLIENT_TOOL_PROFILE: ToolProtocolProfile = ToolProtocolProfile::new(
    ToolProtocol::AnthropicClientTools,
    ToolTransport::ClientRoundTrip,
);

#[cfg(test)]
mod tests {
    use super::{ToolProtocolProfile, CHAT_LIKE_TOOL_PROFILE};
    use crate::llm::{
        Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol, ToolTransport,
    };

    #[test]
    fn profile_builds_consistent_components() {
        let profile =
            ToolProtocolProfile::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);
        let normalized = profile.normalize_correlation(ToolCallCorrelation::new("invoke-1"));

        assert_eq!(normalized.protocol, ToolProtocol::ResponsesLike);
        assert_eq!(normalized.transport, ToolTransport::ClientRoundTrip);
    }

    #[test]
    fn chat_like_profile_preserves_expected_protocol_shape() {
        let normalized =
            CHAT_LIKE_TOOL_PROFILE.normalize_correlation(ToolCallCorrelation::new("invoke-2"));

        assert_eq!(normalized.protocol, ToolProtocol::ChatLike);
        assert_eq!(normalized.wire_tool_call_id(), "invoke-2");
    }

    #[test]
    fn profile_exposes_encoding_and_inbound_helpers() {
        let tool_call = ToolCall::new(
            "invoke-3",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        );
        let encoded_call = CHAT_LIKE_TOOL_PROFILE
            .encode_tool_call(&tool_call)
            .and_then(|call| call.into_chat_like())
            .expect("chat-like tool call");

        assert_eq!(encoded_call.id, "invoke-3");

        let encoded_result = CHAT_LIKE_TOOL_PROFILE
            .encode_tool_result(&Message::tool("invoke-3", "search", "done"))
            .and_then(|result| result.into_chat_like())
            .expect("chat-like tool result");

        assert_eq!(encoded_result.tool_call_id, "invoke-3");

        let inbound =
            CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call("wire-3", None, "search", "{}");
        assert_eq!(inbound.wire_tool_call_id(), "wire-3");
    }
}
