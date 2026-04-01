use crate::config::ModelInfo;

use super::providers;

/// How strictly a provider enforces tool-call history consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolHistoryMode {
    /// Reject only clearly invalid references such as orphaned tool results.
    BestEffort,
    /// Require every tool call batch to have a fully matching set of tool results.
    Strict,
}

/// Provider-specific request behavior relevant to history validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    /// Tool history matching mode enforced before a request is sent.
    pub tool_history_mode: ToolHistoryMode,
    /// Whether the model/provider supports tool-enabled agent calls.
    pub supports_tool_calling: bool,
    /// Whether structured output should be enabled for this route.
    pub supports_structured_output: bool,
}

impl ProviderCapabilities {
    #[must_use]
    /// Build capabilities for one provider/model route.
    pub const fn new(
        tool_history_mode: ToolHistoryMode,
        supports_tool_calling: bool,
        supports_structured_output: bool,
    ) -> Self {
        Self {
            tool_history_mode,
            supports_tool_calling,
            supports_structured_output,
        }
    }

    #[must_use]
    /// Returns true when the provider expects exact tool-call/result matching.
    pub const fn strict_tool_history(self) -> bool {
        matches!(self.tool_history_mode, ToolHistoryMode::Strict)
    }

    #[must_use]
    /// Returns a short label for logs and progress updates.
    pub const fn tool_history_label(self) -> &'static str {
        match self.tool_history_mode {
            ToolHistoryMode::BestEffort => "best_effort",
            ToolHistoryMode::Strict => "strict",
        }
    }

    #[must_use]
    /// Returns true when the route can participate in the agent tool loop.
    pub const fn can_run_agent_tools(self) -> bool {
        self.supports_tool_calling
    }

    #[must_use]
    /// Returns true when the route can accept a `chat_with_tools` style request.
    ///
    /// Structured-output requests without tools are allowed on routes that do not support
    /// client tool calling but do support structured JSON responses.
    pub const fn can_run_chat_with_tools_request(self, has_tools: bool, json_mode: bool) -> bool {
        if has_tools {
            self.supports_tool_calling
        } else {
            self.supports_tool_calling || (json_mode && self.supports_structured_output)
        }
    }

    #[must_use]
    /// Returns true when structured-output prompts and parsing should stay enabled.
    pub const fn should_use_structured_output(self) -> bool {
        self.supports_structured_output
    }
}

/// Returns request-side capabilities for the named provider.
#[must_use]
pub fn provider_capabilities(provider_name: &str) -> ProviderCapabilities {
    match provider_name.to_ascii_lowercase().as_str() {
        "minimax" | "mistral" => ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
        "zai" => ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, false),
        "groq" | "gemini" => ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, true),
        _ => ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true),
    }
}

#[must_use]
/// Returns capabilities for a specific configured model route.
pub fn provider_capabilities_for_model(model_info: &ModelInfo) -> ProviderCapabilities {
    let mut capabilities = provider_capabilities(&model_info.provider);

    if model_info.provider.eq_ignore_ascii_case("nvidia") {
        let model_capabilities = providers::nvidia::model_capabilities(&model_info.id);
        capabilities.supports_tool_calling = model_capabilities.supports_tool_calling;
        capabilities.supports_structured_output = model_capabilities.supports_structured_output;
    }

    capabilities
}

#[must_use]
/// Returns whether structured output should be used for a specific model route.
pub fn supports_structured_output_for_model(model_info: &ModelInfo) -> bool {
    provider_capabilities_for_model(model_info).should_use_structured_output()
}

#[cfg(test)]
mod tests {
    use super::provider_capabilities_for_model;

    #[test]
    fn provider_capabilities_for_nvidia_model_apply_model_specific_overrides() {
        let supported = crate::config::ModelInfo {
            id: "meta/llama-3.1-70b-instruct".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "nvidia".to_string(),
            weight: 1,
        };
        let unsupported = crate::config::ModelInfo {
            id: "deepseek-ai/deepseek-r1".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "nvidia".to_string(),
            weight: 1,
        };

        let supported_capabilities = provider_capabilities_for_model(&supported);
        let unsupported_capabilities = provider_capabilities_for_model(&unsupported);

        assert!(supported_capabilities.supports_tool_calling);
        assert!(supported_capabilities.supports_structured_output);
        assert!(!unsupported_capabilities.supports_tool_calling);
        assert!(!unsupported_capabilities.supports_structured_output);
    }

    #[test]
    fn structured_only_requests_are_allowed_without_tools() {
        let capabilities = super::provider_capabilities("gemini");

        assert!(capabilities.can_run_chat_with_tools_request(false, true));
        assert!(!capabilities.can_run_chat_with_tools_request(false, false));
        assert!(!capabilities.can_run_chat_with_tools_request(true, true));
    }
}
