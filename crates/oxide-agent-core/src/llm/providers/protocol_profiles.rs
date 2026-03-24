use super::tool_call_adapter::ProviderToolCallAdapter;
use super::tool_call_encoder::ProviderToolCallEncoder;
use super::tool_result_encoder::ProviderToolResultEncoder;
use crate::llm::{ToolProtocol, ToolTransport};

pub const CHAT_LIKE_TOOL_ADAPTER: ProviderToolCallAdapter =
    ProviderToolCallAdapter::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

pub const CHAT_LIKE_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    ProviderToolCallEncoder::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

pub const CHAT_LIKE_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    ProviderToolResultEncoder::new(ToolProtocol::ChatLike, ToolTransport::ClientRoundTrip);

pub const ANTHROPIC_CLIENT_TOOL_ADAPTER: ProviderToolCallAdapter = ProviderToolCallAdapter::new(
    ToolProtocol::AnthropicClientTools,
    ToolTransport::ClientRoundTrip,
);

pub const ANTHROPIC_CLIENT_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    ProviderToolCallEncoder::new(
        ToolProtocol::AnthropicClientTools,
        ToolTransport::ClientRoundTrip,
    );

pub const ANTHROPIC_CLIENT_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    ProviderToolResultEncoder::new(
        ToolProtocol::AnthropicClientTools,
        ToolTransport::ClientRoundTrip,
    );

pub const RESPONSES_LIKE_TOOL_ADAPTER: ProviderToolCallAdapter =
    ProviderToolCallAdapter::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);

pub const RESPONSES_LIKE_TOOL_CALL_ENCODER: ProviderToolCallEncoder =
    ProviderToolCallEncoder::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);

pub const RESPONSES_LIKE_TOOL_RESULT_ENCODER: ProviderToolResultEncoder =
    ProviderToolResultEncoder::new(ToolProtocol::ResponsesLike, ToolTransport::ClientRoundTrip);
