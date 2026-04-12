use crate::llm::{ToolCallCorrelation, ToolProtocol, ToolTransport};

/// Shared outbound correlation normalizer for provider wire encoding.
#[derive(Debug, Clone, Copy)]
pub struct ToolCorrelationNormalizer {
    protocol: ToolProtocol,
    transport: ToolTransport,
}

impl ToolCorrelationNormalizer {
    /// Build a normalizer for one provider protocol family.
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    /// Normalize correlation metadata for outbound provider traffic.
    #[must_use]
    pub fn normalize(self, correlation: ToolCallCorrelation) -> ToolCallCorrelation {
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

#[cfg(test)]
mod tests {
    use super::ToolCorrelationNormalizer;
    use crate::llm::{ToolCallCorrelation, ToolProtocol, ToolTransport};

    #[test]
    fn normalizer_applies_protocol_transport_and_fallback_wire_id() {
        let normalizer = ToolCorrelationNormalizer::new(
            ToolProtocol::ResponsesLike,
            ToolTransport::ClientRoundTrip,
        );

        let normalized = normalizer.normalize(ToolCallCorrelation::new("invoke-1"));

        assert_eq!(normalized.protocol, ToolProtocol::ResponsesLike);
        assert_eq!(normalized.transport, ToolTransport::ClientRoundTrip);
        assert_eq!(normalized.wire_tool_call_id(), "invoke-1");
    }

    #[test]
    fn normalizer_preserves_existing_provider_ids() {
        let normalizer =
            ToolCorrelationNormalizer::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

        let normalized = normalizer
            .normalize(ToolCallCorrelation::new("invoke-2").with_provider_tool_call_id("wire-2"));

        assert_eq!(normalized.wire_tool_call_id(), "wire-2");
    }
}
