//! Profile and policy types for OpenAI-compatible Chat Completions providers.

use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};

#[allow(dead_code)]
pub(crate) const OPENROUTER_HEADERS: &[(&str, &str)] = &[
    ("HTTP-Referer", "https://github.com/0FL01/Oxide-Agent"),
    ("X-Title", "Oxide Agent"),
    ("X-OpenRouter-Title", "Oxide Agent"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatToolChoicePolicy {
    AutoWhenToolsExist,
    #[allow(dead_code)]
    Omit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatReasoningPolicy {
    None,
    OpenCodeGo { default_effort: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatThinkingPolicy {
    None,
    #[allow(dead_code)]
    ZaiEnabledUnlessJsonMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatStreamingPolicy {
    NonStreaming,
    #[allow(dead_code)]
    ZaiUnlessNativeJsonMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredOutputPolicy {
    BaseCapability,
    #[allow(dead_code)]
    ZaiGlmToolModelsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitPolicy {
    RetryAfterHeader,
    #[allow(dead_code)]
    ZaiFlushTime,
    #[allow(dead_code)]
    OpenRouterResetMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatResponseContentPolicy {
    StringOnly,
    #[allow(dead_code)]
    StringOrChunkArrayWithReasoning,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChatTemperatures {
    pub(crate) chat: f32,
    pub(crate) tools: f32,
    pub(crate) reasoning: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatCompletionsProfile {
    pub(crate) label: &'static str,
    pub(crate) default_endpoint: &'static str,
    pub(crate) extra_headers: &'static [(&'static str, &'static str)],
    pub(crate) tool_choice: ChatToolChoicePolicy,
    pub(crate) thinking: ChatThinkingPolicy,
    pub(crate) reasoning: ChatReasoningPolicy,
    pub(crate) streaming: ChatStreamingPolicy,
    pub(crate) include_stream_field: bool,
    pub(crate) rate_limit: RateLimitPolicy,
    pub(crate) response_content: ChatResponseContentPolicy,
    pub(crate) capabilities: ProviderCapabilities,
    pub(crate) media_capabilities: MediaCapabilities,
    pub(crate) temperatures: ChatTemperatures,
    pub(crate) parallel_tool_calls: Option<bool>,
    pub(crate) parallel_tool_calls_only_with_tools: bool,
    pub(crate) require_parameters_with_tools: bool,
    pub(crate) include_empty_system_message: bool,
    /// When true, assistant messages carrying tool_calls must include a
    /// `reasoning_content` field (even if empty string) on every subsequent
    /// request. Some reasoning-capable providers (e.g. Xiaomi MiMo, DeepSeek)
    /// reject tool-only assistant messages that omit this field with a
    /// 400 "text is not set" / "Param Incorrect" error.
    pub(crate) require_reasoning_content_on_tool_calls: bool,
    pub(crate) structured_output: StructuredOutputPolicy,
}

impl ChatCompletionsProfile {
    #[must_use]
    #[allow(dead_code)]
    pub(crate) const fn generic() -> Self {
        Self {
            label: "generic",
            default_endpoint: "",
            extra_headers: &[],
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            capabilities: ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true),
            media_capabilities: MediaCapabilities::new(false, true, false),
            temperatures: ChatTemperatures {
                chat: 0.7,
                tools: 0.7,
                reasoning: 0.7,
            },
            parallel_tool_calls: None,
            parallel_tool_calls_only_with_tools: false,
            require_parameters_with_tools: false,
            include_empty_system_message: false,
            require_reasoning_content_on_tool_calls: false,
            structured_output: StructuredOutputPolicy::BaseCapability,
        }
    }
    #[must_use]
    #[allow(dead_code)]
    pub(crate) const fn zai() -> Self {
        Self {
            label: "zai",
            default_endpoint: "https://api.z.ai/api/coding/paas/v4",
            extra_headers: &[],
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            thinking: ChatThinkingPolicy::ZaiEnabledUnlessJsonMode,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::ZaiUnlessNativeJsonMode,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::ZaiFlushTime,
            response_content: ChatResponseContentPolicy::StringOrChunkArrayWithReasoning,
            capabilities: ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, false),
            media_capabilities: MediaCapabilities::new(false, true, false),
            temperatures: ChatTemperatures {
                chat: 0.95,
                tools: 0.95,
                reasoning: 0.95,
            },
            parallel_tool_calls: None,
            parallel_tool_calls_only_with_tools: false,
            require_parameters_with_tools: false,
            include_empty_system_message: false,
            require_reasoning_content_on_tool_calls: false,
            structured_output: StructuredOutputPolicy::ZaiGlmToolModelsOnly,
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) const fn openrouter() -> Self {
        Self {
            label: "openrouter",
            default_endpoint: "https://openrouter.ai/api/v1/chat/completions",
            extra_headers: OPENROUTER_HEADERS,
            tool_choice: ChatToolChoicePolicy::Omit,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: false,
            rate_limit: RateLimitPolicy::OpenRouterResetMetadata,
            response_content: ChatResponseContentPolicy::StringOnly,
            capabilities: ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, false),
            media_capabilities: MediaCapabilities::new(false, false, false),
            temperatures: ChatTemperatures {
                chat: 0.7,
                tools: 0.7,
                reasoning: 0.7,
            },
            parallel_tool_calls: None,
            parallel_tool_calls_only_with_tools: true,
            require_parameters_with_tools: true,
            include_empty_system_message: true,
            require_reasoning_content_on_tool_calls: false,
            structured_output: StructuredOutputPolicy::BaseCapability,
        }
    }

    #[must_use]
    pub(crate) const fn opencode_go() -> Self {
        Self {
            label: "opencode_go",
            default_endpoint: "https://opencode.ai/zen/go/v1/chat/completions",
            extra_headers: &[],
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::OpenCodeGo {
                default_effort: "high",
            },
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            capabilities: ProviderCapabilities::new(ToolHistoryMode::Strict, true, false),
            media_capabilities: MediaCapabilities::new(false, true, false),
            temperatures: ChatTemperatures {
                chat: 0.7,
                tools: 0.7,
                reasoning: 0.7,
            },
            parallel_tool_calls: Some(true),
            parallel_tool_calls_only_with_tools: true,
            require_parameters_with_tools: false,
            include_empty_system_message: false,
            require_reasoning_content_on_tool_calls: true,
            structured_output: StructuredOutputPolicy::BaseCapability,
        }
    }

    #[must_use]
    pub(crate) const fn opencode_zen() -> Self {
        Self {
            label: "opencode_zen",
            default_endpoint: "https://opencode.ai/zen/v1/chat/completions",
            extra_headers: &[],
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::OpenCodeGo {
                default_effort: "high",
            },
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            capabilities: ProviderCapabilities::new(ToolHistoryMode::Strict, true, false),
            media_capabilities: MediaCapabilities::new(false, true, false),
            temperatures: ChatTemperatures {
                chat: 0.7,
                tools: 0.7,
                reasoning: 0.7,
            },
            parallel_tool_calls: Some(true),
            parallel_tool_calls_only_with_tools: true,
            require_parameters_with_tools: false,
            include_empty_system_message: false,
            require_reasoning_content_on_tool_calls: true,
            structured_output: StructuredOutputPolicy::BaseCapability,
        }
    }

    #[must_use]
    pub(crate) fn capabilities_for_model(&self, model_id: &str) -> ProviderCapabilities {
        let mut capabilities = self.capabilities;
        if matches!(
            self.structured_output,
            StructuredOutputPolicy::ZaiGlmToolModelsOnly
        ) {
            capabilities.supports_structured_output = zai_supports_structured_output(model_id);
        }
        capabilities
    }
}

fn zai_supports_structured_output(model_id: &str) -> bool {
    matches!(
        model_id.trim().to_ascii_lowercase().as_str(),
        "glm-4.7" | "glm-4" | "mainagent" | "glm-4.6" | "glm-4.5-air" | "glm-4-air" | "subagent"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_profile_matches_openai_base_defaults() {
        let p = ChatCompletionsProfile::generic();

        assert_eq!(p.label, "generic");
        assert_eq!(p.response_content, ChatResponseContentPolicy::StringOnly);
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(p.media_capabilities.supports_image_understanding);
    }

    #[test]
    fn zai_profile_preserves_thinking_streaming_and_structured_output_policies() {
        let p = ChatCompletionsProfile::zai();

        assert_eq!(p.label, "zai");
        assert_eq!(p.thinking, ChatThinkingPolicy::ZaiEnabledUnlessJsonMode);
        assert_eq!(p.streaming, ChatStreamingPolicy::ZaiUnlessNativeJsonMode);
        assert_eq!(p.rate_limit, RateLimitPolicy::ZaiFlushTime);
        assert_eq!(
            p.structured_output,
            StructuredOutputPolicy::ZaiGlmToolModelsOnly
        );
        assert!(p.media_capabilities.supports_image_understanding);
        assert!(!p.capabilities.supports_structured_output);
    }

    #[test]
    fn openrouter_profile_adds_attribution_headers() {
        let p = ChatCompletionsProfile::openrouter();

        assert_eq!(p.label, "openrouter");
        assert_eq!(p.extra_headers, OPENROUTER_HEADERS);
        assert_eq!(p.tool_choice, ChatToolChoicePolicy::Omit);
        assert_eq!(p.rate_limit, RateLimitPolicy::OpenRouterResetMetadata);
    }

    #[test]
    fn opencode_go_profile_preserves_router_owned_exact_endpoint_and_strict_tools() {
        let p = ChatCompletionsProfile::opencode_go();

        assert_eq!(p.label, "opencode_go");
        assert_eq!(
            p.reasoning,
            ChatReasoningPolicy::OpenCodeGo {
                default_effort: "high"
            }
        );
        assert!(p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(!p.capabilities.supports_structured_output);
        assert!(p.media_capabilities.supports_image_understanding);
        assert!(
            p.require_reasoning_content_on_tool_calls,
            "opencode_go must require reasoning_content on tool-call assistant messages for MiMo/DeepSeek compatibility"
        );
    }
}
