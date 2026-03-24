use crate::llm::{
    InvocationId, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol,
    ToolTransport,
};
use uuid::Uuid;

/// Provider-local adapter for encoding and decoding tool-call wire identifiers.
#[derive(Debug, Clone, Copy)]
pub struct ProviderToolCallAdapter {
    protocol: ToolProtocol,
    transport: ToolTransport,
}

impl ProviderToolCallAdapter {
    /// Build a new adapter for one provider protocol family.
    #[must_use]
    pub const fn new(protocol: ToolProtocol, transport: ToolTransport) -> Self {
        Self {
            protocol,
            transport,
        }
    }

    /// Resolve the outbound provider wire id for an assistant tool call.
    #[must_use]
    pub fn assistant_tool_call_id(self, tool_call: &ToolCall) -> String {
        self.normalize_outbound_correlation(tool_call.correlation())
            .wire_tool_call_id()
            .to_string()
    }

    /// Resolve the outbound provider wire id for a tool result message.
    #[must_use]
    pub fn tool_result_call_id(self, message: &Message) -> Option<String> {
        message
            .resolved_tool_call_correlation()
            .map(|correlation| self.normalize_outbound_correlation(correlation))
            .map(|correlation| correlation.wire_tool_call_id().to_string())
    }

    /// Build a runtime tool call from provider wire identifiers.
    #[must_use]
    pub fn inbound_tool_call(
        self,
        invocation_id: impl Into<InvocationId>,
        provider_tool_call_id: Option<&str>,
        provider_item_id: Option<&str>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        let invocation_id = invocation_id.into();
        let mut correlation = ToolCallCorrelation::new(invocation_id.clone())
            .with_protocol(self.protocol)
            .with_transport(self.transport);

        if let Some(provider_tool_call_id) = provider_tool_call_id {
            correlation = correlation.with_provider_tool_call_id(provider_tool_call_id);
        }
        if let Some(provider_item_id) = provider_item_id {
            correlation = correlation.with_provider_item_id(provider_item_id);
        }

        ToolCall::new(
            invocation_id.into_inner(),
            ToolCallFunction {
                name: name.into(),
                arguments: arguments.into(),
            },
            false,
        )
        .with_correlation(correlation)
    }

    /// Build a runtime tool call from a provider-generated wire identifier.
    #[must_use]
    pub fn inbound_provider_tool_call(
        self,
        provider_tool_call_id: &str,
        provider_item_id: Option<&str>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        let invocation_id = InvocationId::new(format!("call_{}", Uuid::new_v4()));
        self.inbound_tool_call(
            invocation_id,
            Some(provider_tool_call_id),
            provider_item_id,
            name,
            arguments,
        )
    }

    /// Build a runtime tool call when the provider omits a wire correlation id.
    #[must_use]
    pub fn inbound_uncorrelated_tool_call(
        self,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> ToolCall {
        let invocation_id = InvocationId::new(format!("call_{}", Uuid::new_v4()));
        self.inbound_tool_call(invocation_id, None, None, name, arguments)
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

#[cfg(test)]
mod tests {
    use super::ProviderToolCallAdapter;
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolProtocol};

    #[test]
    fn adapter_prefers_provider_wire_ids_for_outbound_history() {
        let adapter = ProviderToolCallAdapter::new(
            ToolProtocol::AnthropicClientTools,
            crate::llm::ToolTransport::ClientRoundTrip,
        );
        let tool_call = ToolCall::new(
            "invoke-1",
            ToolCallFunction {
                name: "search".to_string(),
                arguments: "{}".to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-1")
                .with_provider_tool_call_id("wire-1")
                .with_protocol(ToolProtocol::AnthropicClientTools),
        );
        let tool_message =
            Message::tool_with_correlation("invoke-1", tool_call.correlation(), "search", "done");

        assert_eq!(adapter.assistant_tool_call_id(&tool_call), "wire-1");
        assert_eq!(
            adapter.tool_result_call_id(&tool_message),
            Some("wire-1".to_string())
        );
    }

    #[test]
    fn adapter_builds_inbound_tool_calls_with_protocol_metadata() {
        let adapter = ProviderToolCallAdapter::new(
            ToolProtocol::ChatLike,
            crate::llm::ToolTransport::ClientRoundTrip,
        );

        let tool_call =
            adapter.inbound_tool_call("invoke-2", Some("provider-2"), None, "search", "{}");

        assert_eq!(tool_call.id, "invoke-2");
        assert_eq!(tool_call.invocation_id().as_str(), "invoke-2");
        assert_eq!(tool_call.wire_tool_call_id(), "provider-2");
        assert_eq!(tool_call.correlation().protocol, ToolProtocol::ChatLike);
    }

    #[test]
    fn adapter_generates_fresh_invocation_ids_for_provider_generated_calls() {
        let adapter = ProviderToolCallAdapter::new(
            ToolProtocol::ChatLike,
            crate::llm::ToolTransport::ClientRoundTrip,
        );

        let tool_call = adapter.inbound_provider_tool_call("provider-3", None, "search", "{}");

        assert_ne!(tool_call.id, "provider-3");
        assert_eq!(tool_call.wire_tool_call_id(), "provider-3");
        assert_eq!(
            tool_call.correlation().provider_tool_call_id,
            Some("provider-3".into())
        );
    }

    #[test]
    fn adapter_generates_uncorrelated_runtime_ids_when_provider_omits_wire_id() {
        let adapter = ProviderToolCallAdapter::new(
            ToolProtocol::ChatLike,
            crate::llm::ToolTransport::ClientRoundTrip,
        );

        let tool_call = adapter.inbound_uncorrelated_tool_call("search", "{}");

        assert_ne!(tool_call.id, "search");
        assert_eq!(tool_call.wire_tool_call_id(), tool_call.id);
        assert!(tool_call.correlation().provider_tool_call_id.is_none());
    }
}
