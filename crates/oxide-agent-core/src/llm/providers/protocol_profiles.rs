use super::tool_call_adapter::ProviderToolCallAdapter;
use super::tool_call_encoder::ProviderToolCallEncoder;
use super::tool_correlation::ToolCorrelationNormalizer;
use super::tool_result_encoder::ProviderToolResultEncoder;
use crate::llm::{ToolProtocol, ToolTransport};

/// Unified tool protocol behavior profile for one provider wire family.
#[derive(Debug, Clone, Copy)]
pub struct ToolProtocolProfile {
    pub adapter: ProviderToolCallAdapter,
    pub tool_call_encoder: ProviderToolCallEncoder,
    pub tool_result_encoder: ProviderToolResultEncoder,
    pub correlation_normalizer: ToolCorrelationNormalizer,
}

impl ToolProtocolProfile {
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            adapter: ProviderToolCallAdapter::new(protocol, transport),
            tool_call_encoder: ProviderToolCallEncoder::new(protocol, transport),
            tool_result_encoder: ProviderToolResultEncoder::new(protocol, transport),
            correlation_normalizer: ToolCorrelationNormalizer::new(protocol, transport),
        }
    }
}

pub const CHAT_LIKE_TOOL_PROFILE: ToolProtocolProfile =
    ToolProtocolProfile::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

pub const ANTHROPIC_CLIENT_TOOL_PROFILE: ToolProtocolProfile = ToolProtocolProfile::new(
    ToolProtocol::AnthropicClientTools,
    ToolTransport::ClientRoundTrip,
);

pub const RESPONSES_LIKE_TOOL_PROFILE: ToolProtocolProfile =
    ToolProtocolProfile::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);

pub const CHAT_LIKE_TOOL_ADAPTER: ProviderToolCallAdapter = CHAT_LIKE_TOOL_PROFILE.adapter;
pub const CHAT_LIKE_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    CHAT_LIKE_TOOL_PROFILE.tool_call_encoder;
pub const CHAT_LIKE_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    CHAT_LIKE_TOOL_PROFILE.tool_result_encoder;

pub const ANTHROPIC_CLIENT_TOOL_ADAPTER: ProviderToolCallAdapter =
    ANTHROPIC_CLIENT_TOOL_PROFILE.adapter;
pub const ANTHROPIC_CLIENT_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    ANTHROPIC_CLIENT_TOOL_PROFILE.tool_call_encoder;
pub const ANTHROPIC_CLIENT_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    ANTHROPIC_CLIENT_TOOL_PROFILE.tool_result_encoder;

pub const RESPONSES_LIKE_TOOL_ADAPTER: ProviderToolCallAdapter =
    RESPONSES_LIKE_TOOL_PROFILE.adapter;
pub const RESPONSES_LIKE_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    RESPONSES_LIKE_TOOL_PROFILE.tool_call_encoder;
pub const RESPONSES_LIKE_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    RESPONSES_LIKE_TOOL_PROFILE.tool_result_encoder;

#[cfg(test)]
mod tests {
    use super::{ToolProtocolProfile, CHAT_LIKE_TOOL_PROFILE};
    use crate::llm::{ToolCallCorrelation, ToolProtocol, ToolTransport};

    #[test]
    fn profile_builds_consistent_components() {
        let profile =
            ToolProtocolProfile::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);
        let normalized = profile
            .correlation_normalizer
            .normalize(ToolCallCorrelation::new("invoke-1"));

        assert_eq!(normalized.protocol, ToolProtocol::ResponsesLike);
        assert_eq!(normalized.transport, ToolTransport::ClientRoundTrip);
    }

    #[test]
    fn chat_like_profile_preserves_expected_protocol_shape() {
        let normalized = CHAT_LIKE_TOOL_PROFILE
            .correlation_normalizer
            .normalize(ToolCallCorrelation::new("invoke-2"));

        assert_eq!(normalized.protocol, ToolProtocol::ChatLike);
        assert_eq!(normalized.wire_tool_call_id(), "invoke-2");
    }
}
