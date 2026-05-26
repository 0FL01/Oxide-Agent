use crate::config::ModelInfo;

use super::providers;

/// Media modality types used for capability-based route resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaModality {
    AudioTranscription,
    ImageUnderstanding,
    VideoUnderstanding,
}

impl MediaModality {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AudioTranscription => "audio transcription",
            Self::ImageUnderstanding => "image understanding",
            Self::VideoUnderstanding => "video understanding",
        }
    }
}

/// Provider support matrix for media modalities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaCapabilities {
    pub supports_audio_transcription: bool,
    pub supports_image_understanding: bool,
    pub supports_video_understanding: bool,
}

impl MediaCapabilities {
    #[must_use]
    pub const fn new(audio: bool, image: bool, video: bool) -> Self {
        Self {
            supports_audio_transcription: audio,
            supports_image_understanding: image,
            supports_video_understanding: video,
        }
    }

    #[must_use]
    pub const fn supports(self, modality: MediaModality) -> bool {
        match modality {
            MediaModality::AudioTranscription => self.supports_audio_transcription,
            MediaModality::ImageUnderstanding => self.supports_image_understanding,
            MediaModality::VideoUnderstanding => self.supports_video_understanding,
        }
    }
}

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
    providers::provider_capabilities(provider_name).unwrap_or_else(default_provider_capabilities)
}

#[must_use]
/// Returns media modality support for a provider.
#[allow(dead_code)]
fn provider_media_capabilities(provider_name: &str) -> MediaCapabilities {
    providers::provider_media_capabilities(provider_name).unwrap_or_else(default_media_capabilities)
}

#[must_use]
/// Returns media modality support for a specific configured model route.
pub fn provider_media_capabilities_for_model(model_info: &ModelInfo) -> MediaCapabilities {
    providers::provider_media_capabilities_for_model(model_info)
        .unwrap_or_else(default_media_capabilities)
}

#[must_use]
/// Returns capabilities for a specific configured model route.
pub fn provider_capabilities_for_model(model_info: &ModelInfo) -> ProviderCapabilities {
    providers::provider_capabilities_for_model(model_info)
        .unwrap_or_else(|| provider_capabilities(&model_info.provider))
}

fn default_provider_capabilities() -> ProviderCapabilities {
    ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, false)
}

const fn default_media_capabilities() -> MediaCapabilities {
    MediaCapabilities::new(false, false, false)
}

#[must_use]
/// Returns whether structured output should be used for a specific model route.
pub fn supports_structured_output_for_model(model_info: &ModelInfo) -> bool {
    provider_capabilities_for_model(model_info).should_use_structured_output()
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "llm-nvidia")]
    #[test]
    fn provider_capabilities_for_nvidia_model_apply_model_specific_overrides() {
        let supported = crate::config::ModelInfo {
            id: "deepseek-ai/deepseek-v4-flash".to_string(),
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

        let supported_capabilities = super::provider_capabilities_for_model(&supported);
        let unsupported_capabilities = super::provider_capabilities_for_model(&unsupported);

        assert!(supported_capabilities.supports_tool_calling);
        assert!(supported_capabilities.supports_structured_output);
        assert!(!unsupported_capabilities.supports_tool_calling);
        assert!(!unsupported_capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-chatgpt")]
    #[test]
    fn chatgpt_capabilities_disable_structured_output() {
        let capabilities = super::provider_capabilities("chatgpt");

        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "best_effort");
    }

    #[cfg(feature = "llm-minimax")]
    #[test]
    fn minimax_capabilities_disable_structured_output() {
        let capabilities = super::provider_capabilities("minimax");

        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "strict");
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn openrouter_provider_capabilities_are_default_deny_without_model_policy() {
        let capabilities = super::provider_capabilities("openrouter");

        assert!(!capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "best_effort");
    }

    #[cfg(feature = "llm-openrouter")]
    #[test]
    fn openrouter_model_policy_allows_only_explicit_agent_routes() {
        for model_id in ["deepseek/deepseek-v4-flash", "deepseek/deepseek-v4-pro"] {
            let route = crate::config::ModelInfo {
                id: model_id.to_string(),
                max_output_tokens: 4096,
                context_window_tokens: 128_000,
                provider: "openrouter".to_string(),
                weight: 1,
            };
            let capabilities = super::provider_capabilities_for_model(&route);
            assert!(capabilities.supports_tool_calling, "{model_id}");
            assert!(capabilities.supports_structured_output, "{model_id}");
        }

        for model_id in [
            "google/gemini-3-flash-preview",
            "google/gemini-3.1-flash-lite-preview",
            "google/gemini-2.5-flash-lite",
        ] {
            let route = crate::config::ModelInfo {
                id: model_id.to_string(),
                max_output_tokens: 4096,
                context_window_tokens: 128_000,
                provider: "openrouter".to_string(),
                weight: 1,
            };
            let capabilities = super::provider_capabilities_for_model(&route);
            assert!(!capabilities.supports_tool_calling, "{model_id}");
            assert!(capabilities.supports_structured_output, "{model_id}");
        }

        let unknown = crate::config::ModelInfo {
            id: "unknown/model".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "openrouter".to_string(),
            weight: 1,
        };

        assert!(!super::provider_capabilities_for_model(&unknown).supports_tool_calling);
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_capabilities_enable_strict_tools() {
        let capabilities = super::provider_capabilities("opencode-go");

        assert!(capabilities.supports_tool_calling);
        assert!(capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "strict");

        let alias = super::provider_capabilities("opencode_go");
        assert_eq!(alias.tool_history_label(), "strict");
        assert!(alias.supports_tool_calling);
    }

    #[cfg(feature = "llm-zai")]
    #[test]
    fn zai_capabilities_disable_structured_output() {
        let capabilities = super::provider_capabilities("zai");

        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "best_effort");
    }

    #[cfg(feature = "llm-zai")]
    #[test]
    fn provider_capabilities_for_zai_model_apply_model_specific_overrides() {
        let structured = crate::config::ModelInfo {
            id: "glm-4.6".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "zai".to_string(),
            weight: 1,
        };
        let conservative = crate::config::ModelInfo {
            id: "glm-5".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "zai".to_string(),
            weight: 1,
        };

        let structured_capabilities = super::provider_capabilities_for_model(&structured);
        let conservative_capabilities = super::provider_capabilities_for_model(&conservative);

        assert!(structured_capabilities.supports_tool_calling);
        assert!(structured_capabilities.supports_structured_output);
        assert!(conservative_capabilities.supports_tool_calling);
        assert!(!conservative_capabilities.supports_structured_output);
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_deepseek_v4_flash_supports_structured_output() {
        let route = crate::config::ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "opencode-go".to_string(),
            weight: 1,
        };

        let capabilities = super::provider_capabilities_for_model(&route);

        assert!(capabilities.supports_tool_calling);
        assert!(capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "strict");
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_unknown_model_does_not_overclaim_structured_output() {
        let route = crate::config::ModelInfo {
            id: "kimi-k2.6".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "opencode-go".to_string(),
            weight: 1,
        };

        let capabilities = super::provider_capabilities_for_model(&route);

        assert!(capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "strict");
    }

    #[cfg(feature = "llm-opencode-go")]
    #[test]
    fn opencode_go_prefixed_model_id_is_normalized_for_capabilities() {
        let route = crate::config::ModelInfo {
            id: "opencode-go/deepseek-v4-pro".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "opencode_go".to_string(),
            weight: 1,
        };

        let capabilities = super::provider_capabilities_for_model(&route);

        assert!(capabilities.supports_structured_output);
    }

    #[cfg(all(
        feature = "llm-openrouter",
        feature = "llm-mistral",
        feature = "llm-opencode-go"
    ))]
    #[test]
    fn media_capabilities_are_modality_specific() {
        let openrouter_media = crate::config::ModelInfo {
            id: "google/gemini-3-flash-preview".to_string(),
            max_output_tokens: 4096,
            context_window_tokens: 128_000,
            provider: "openrouter".to_string(),
            weight: 1,
        };
        let openrouter = super::provider_media_capabilities_for_model(&openrouter_media);
        let mistral = super::provider_media_capabilities("mistral");
        let opencode_go = super::provider_media_capabilities("opencode-go");

        assert!(openrouter.supports(super::MediaModality::AudioTranscription));
        assert!(openrouter.supports(super::MediaModality::ImageUnderstanding));
        assert!(openrouter.supports(super::MediaModality::VideoUnderstanding));

        assert!(mistral.supports(super::MediaModality::AudioTranscription));
        assert!(!mistral.supports(super::MediaModality::ImageUnderstanding));
        assert!(!mistral.supports(super::MediaModality::VideoUnderstanding));

        assert!(!opencode_go.supports(super::MediaModality::AudioTranscription));
        assert!(!opencode_go.supports(super::MediaModality::ImageUnderstanding));
        assert!(!opencode_go.supports(super::MediaModality::VideoUnderstanding));
    }

    #[test]
    fn unknown_provider_capabilities_are_default_deny() {
        let capabilities = super::provider_capabilities("removed-provider");

        assert!(!capabilities.supports_tool_calling);
        assert!(!capabilities.supports_structured_output);
        assert_eq!(capabilities.tool_history_label(), "best_effort");
    }
}
