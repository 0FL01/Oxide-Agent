//! LLM providers and client
//!
//! Provides a unified interface to agent-compatible LLM providers.

mod capabilities;
mod client;
mod error;
mod provider;
/// Implementations of specific LLM providers
pub mod providers;
mod support;
mod types;

pub use capabilities::{ProviderCapabilities, ToolHistoryMode};
pub(crate) use capabilities::{
    provider_capabilities_for_model, provider_media_capabilities_for_model,
};
pub(crate) use client::InternalTextPurpose;
pub use client::{DiscoveredLlmModel, LlmClient};
pub use error::LlmError;
pub use provider::LlmProvider;
#[cfg(test)]
pub use provider::MockLlmProvider;
#[cfg(any(
    feature = "llm-chatgpt",
    feature = "llm-mistral",
    feature = "llm-zai",
    feature = "llm-nvidia",
    feature = "llm-opencode-go",
    feature = "llm-openrouter"
))]
pub use support::http;
pub use types::{
    ChatResponse, ChatWithToolsRequest, InvocationId, Message, MessageContentPart, ProviderItemId,
    ProviderToolCallId, TokenUsage, ToolCall, ToolCallCorrelation, ToolCallFunction,
    ToolDefinition, ToolProtocol, ToolTransport,
};

#[cfg(test)]
mod tests {
    use super::{
        InvocationId, Message, ProviderItemId, ProviderToolCallId, ToolCall, ToolCallCorrelation,
        ToolCallFunction, ToolProtocol, ToolTransport,
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
    fn tool_call_correlation_defaults_to_invocation_id_for_wire_usage() {
        let correlation = ToolCallCorrelation::new("call-123");

        assert_eq!(correlation.invocation_id, InvocationId::from("call-123"));
        assert_eq!(correlation.wire_tool_call_id(), "call-123");
        assert_eq!(correlation.invocation_id.as_str(), "call-123");
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
        assert_eq!(correlation.invocation_id.as_str(), "invoke-1");
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
            "runtime-call-id",
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
    fn tool_message_serialization_includes_wire_and_canonical_correlation_fields() {
        let message = Message::tool("call-1", "search", "result");
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["tool_call_id"], json!("call-1"));
        assert_eq!(
            value["tool_call_correlation"]["invocation_id"],
            json!("call-1")
        );
    }

    #[test]
    fn tool_message_resolves_correlation_from_tool_call_id() {
        let value = json!({
            "role": "tool",
            "content": "result",
            "tool_call_id": "call-wire",
            "name": "search"
        });
        let message: Message = serde_json::from_value(value).expect("message deserializes");

        assert_eq!(message.tool_call_correlation, None);
        assert_eq!(
            message.resolved_tool_call_correlation(),
            Some(ToolCallCorrelation::new("call-wire"))
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
    fn assistant_tool_batch_serialization_includes_reasoning_content_when_present() {
        let message = Message::assistant_with_tools_and_reasoning(
            "calling tools",
            Some("thinking trace".to_string()),
            vec![tool_call("call-1", "search")],
        );
        let value = serde_json::to_value(&message).expect("message serializes");

        assert_eq!(value["reasoning_content"], json!("thinking trace"));
        assert_eq!(value["tool_calls"][0]["id"], json!("call-1"));
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
            Some(vec![
                ToolCallCorrelation::new("invoke-2")
                    .with_provider_tool_call_id("provider-call-2")
                    .with_protocol(ToolProtocol::ChatLike)
            ])
        );
    }

    #[test]
    fn assistant_tool_batch_resolves_correlations_from_tool_call_ids() {
        let value = json!({
            "role": "assistant",
            "content": "calling tools",
            "tool_calls": [{
                "id": "call-wire",
                "function": {
                    "name": "search",
                    "arguments": "{}"
                },
                "is_recovered": false
            }]
        });
        let message: Message = serde_json::from_value(value).expect("message deserializes");

        assert_eq!(message.tool_call_correlations, None);
        assert_eq!(
            message.resolved_tool_call_correlations(),
            Some(vec![ToolCallCorrelation::new("call-wire")])
        );
    }
}
