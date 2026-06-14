//! Shared Anthropic Messages v1 wire helpers.
//!
//! Provides request body construction, message conversion, tool schema encoding,
//! response parsing, and usage extraction for the Anthropic Messages API format.
//! Used by `opencode_go` and `anthropic` providers.

pub(crate) mod request;
pub(crate) mod response;

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Profile for provider-specific behavior in the shared Anthropic Messages helpers.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AnthropicProfile {
    /// Provider label for error messages.
    pub label: &'static str,
    /// Prefix for generated fallback tool IDs when the provider returns empty IDs.
    /// `None` means empty IDs are kept as-is.
    pub empty_tool_id_fallback_prefix: Option<&'static str>,
}

impl AnthropicProfile {
    /// Profile for OpenCode Go provider.
    pub const fn opencode_go() -> Self {
        Self {
            label: "OpenCode Go",
            empty_tool_id_fallback_prefix: Some("opencode_go_tool_use_"),
        }
    }

    /// Profile for generic Anthropic Messages API provider.
    #[allow(dead_code)] // used when llm-minimax feature is active
    pub const fn anthropic() -> Self {
        Self {
            label: "Anthropic",
            empty_tool_id_fallback_prefix: Some("anthropic_fallback_"),
        }
    }
}
