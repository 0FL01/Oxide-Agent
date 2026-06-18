//! Profile and policy types for OpenAI-compatible Chat Completions providers.

use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};

pub(crate) const OPENROUTER_HEADERS: &[(&str, &str)] = &[
    ("HTTP-Referer", "https://github.com/0FL01/Oxide-Agent"),
    ("X-Title", "Oxide Agent"),
    ("X-OpenRouter-Title", "Oxide Agent"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointPolicy {
    UseConfiguredUrlAsExactEndpoint,
    AppendChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthPolicy {
    Bearer,
    NoAuth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCallIdPolicy {
    Preserve,
    MistralNineAlnum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmptyToolCallIdPolicy {
    Uncorrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatMessageLayoutPolicy {
    GenericOpenAI,
    MistralStrict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatToolSchemaPolicy {
    OpenAIChatCompletions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatToolChoicePolicy {
    AutoWhenToolsExist,
    Omit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonModePolicy {
    None,
    Standard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelMatchPolicy {
    None,
    CaseInsensitiveContains(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatReasoningPolicy {
    None,
    Mistral {
        default_effort: &'static str,
        model_match: ModelMatchPolicy,
    },
    OpenCodeGo {
        default_effort: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatThinkingPolicy {
    None,
    ZaiEnabledUnlessJsonMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatStreamingPolicy {
    NonStreaming,
    ZaiUnlessNativeJsonMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuredOutputPolicy {
    BaseCapability,
    ZaiGlmToolModelsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RateLimitPolicy {
    RetryAfterHeader,
    ZaiFlushTime,
    OpenRouterResetMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatResponseContentPolicy {
    StringOnly,
    StringOrChunkArrayWithReasoning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsagePolicy {
    PromptTokensDetailsCached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageInputPolicy {
    None,
    ImageUrlDataUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioInputPolicy {
    None,
    MultipartTranscription,
    OpenRouterInputAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VideoInputPolicy {
    None,
    OpenRouterVideoUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChatMediaPolicy {
    pub(crate) image: ImageInputPolicy,
    pub(crate) audio: AudioInputPolicy,
    pub(crate) video: VideoInputPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AudioTranscriptionProfile {
    pub(crate) endpoint_path: &'static str,
    pub(crate) temperature: f32,
    pub(crate) timeout_secs: u64,
    pub(crate) max_retries: usize,
    pub(crate) initial_backoff_ms: u64,
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
    pub(crate) endpoint: EndpointPolicy,
    pub(crate) auth: AuthPolicy,
    pub(crate) extra_headers: &'static [(&'static str, &'static str)],
    pub(crate) tool_call_ids: ToolCallIdPolicy,
    pub(crate) empty_tool_call_id: EmptyToolCallIdPolicy,
    pub(crate) message_layout: ChatMessageLayoutPolicy,
    pub(crate) tool_schema: ChatToolSchemaPolicy,
    pub(crate) tool_choice: ChatToolChoicePolicy,
    pub(crate) json_mode: JsonModePolicy,
    pub(crate) thinking: ChatThinkingPolicy,
    pub(crate) reasoning: ChatReasoningPolicy,
    pub(crate) streaming: ChatStreamingPolicy,
    pub(crate) include_stream_field: bool,
    pub(crate) rate_limit: RateLimitPolicy,
    pub(crate) response_content: ChatResponseContentPolicy,
    pub(crate) usage: UsagePolicy,
    pub(crate) media: ChatMediaPolicy,
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
    pub(crate) audio_transcription: Option<AudioTranscriptionProfile>,
}

impl ChatCompletionsProfile {
    #[must_use]
    pub(crate) fn endpoint_for(self, configured: &str) -> String {
        let trimmed = configured.trim().trim_end_matches('/');
        match self.endpoint {
            EndpointPolicy::UseConfiguredUrlAsExactEndpoint => trimmed.to_string(),
            EndpointPolicy::AppendChatCompletions => {
                if trimmed.ends_with("/chat/completions") {
                    trimmed.to_string()
                } else {
                    format!("{trimmed}/chat/completions")
                }
            }
        }
    }

    #[must_use]
    pub(crate) const fn generic() -> Self {
        Self {
            label: "generic",
            default_endpoint: "",
            endpoint: EndpointPolicy::AppendChatCompletions,
            auth: AuthPolicy::Bearer,
            extra_headers: &[],
            tool_call_ids: ToolCallIdPolicy::Preserve,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::GenericOpenAI,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            json_mode: JsonModePolicy::Standard,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::ImageUrlDataUrl,
                audio: AudioInputPolicy::None,
                video: VideoInputPolicy::None,
            },
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
            audio_transcription: None,
        }
    }

    #[must_use]
    pub(crate) const fn mistral() -> Self {
        Self {
            label: "mistral",
            default_endpoint: "https://api.mistral.ai/v1",
            endpoint: EndpointPolicy::AppendChatCompletions,
            auth: AuthPolicy::Bearer,
            extra_headers: &[],
            tool_call_ids: ToolCallIdPolicy::MistralNineAlnum,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::MistralStrict,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            json_mode: JsonModePolicy::Standard,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::Mistral {
                default_effort: "high",
                model_match: ModelMatchPolicy::CaseInsensitiveContains("mistral-small-2603"),
            },
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOrChunkArrayWithReasoning,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::None,
                audio: AudioInputPolicy::MultipartTranscription,
                video: VideoInputPolicy::None,
            },
            capabilities: ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
            media_capabilities: MediaCapabilities::new(true, false, false),
            temperatures: ChatTemperatures {
                chat: 0.9,
                tools: 0.7,
                reasoning: 0.7,
            },
            parallel_tool_calls: Some(true),
            parallel_tool_calls_only_with_tools: false,
            require_parameters_with_tools: false,
            include_empty_system_message: true,
            require_reasoning_content_on_tool_calls: false,
            structured_output: StructuredOutputPolicy::BaseCapability,
            audio_transcription: Some(AudioTranscriptionProfile {
                endpoint_path: "/audio/transcriptions",
                temperature: 0.4,
                timeout_secs: 120,
                max_retries: 5,
                initial_backoff_ms: 3_000,
            }),
        }
    }

    #[must_use]
    pub(crate) const fn zai() -> Self {
        Self {
            label: "zai",
            default_endpoint: "https://api.z.ai/api/coding/paas/v4",
            endpoint: EndpointPolicy::AppendChatCompletions,
            auth: AuthPolicy::Bearer,
            extra_headers: &[],
            tool_call_ids: ToolCallIdPolicy::Preserve,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::GenericOpenAI,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            json_mode: JsonModePolicy::Standard,
            thinking: ChatThinkingPolicy::ZaiEnabledUnlessJsonMode,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::ZaiUnlessNativeJsonMode,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::ZaiFlushTime,
            response_content: ChatResponseContentPolicy::StringOrChunkArrayWithReasoning,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::ImageUrlDataUrl,
                audio: AudioInputPolicy::None,
                video: VideoInputPolicy::None,
            },
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
            audio_transcription: None,
        }
    }

    #[must_use]
    pub(crate) const fn openrouter() -> Self {
        Self {
            label: "openrouter",
            default_endpoint: "https://openrouter.ai/api/v1/chat/completions",
            endpoint: EndpointPolicy::UseConfiguredUrlAsExactEndpoint,
            auth: AuthPolicy::Bearer,
            extra_headers: OPENROUTER_HEADERS,
            tool_call_ids: ToolCallIdPolicy::Preserve,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::GenericOpenAI,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::Omit,
            json_mode: JsonModePolicy::None,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::None,
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: false,
            rate_limit: RateLimitPolicy::OpenRouterResetMetadata,
            response_content: ChatResponseContentPolicy::StringOnly,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::ImageUrlDataUrl,
                audio: AudioInputPolicy::OpenRouterInputAudio,
                video: VideoInputPolicy::OpenRouterVideoUrl,
            },
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
            audio_transcription: None,
        }
    }

    #[must_use]
    pub(crate) const fn opencode_go() -> Self {
        Self {
            label: "opencode_go",
            default_endpoint: "https://opencode.ai/zen/go/v1/chat/completions",
            endpoint: EndpointPolicy::UseConfiguredUrlAsExactEndpoint,
            auth: AuthPolicy::Bearer,
            extra_headers: &[],
            tool_call_ids: ToolCallIdPolicy::Preserve,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::GenericOpenAI,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            json_mode: JsonModePolicy::Standard,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::OpenCodeGo {
                default_effort: "high",
            },
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::ImageUrlDataUrl,
                audio: AudioInputPolicy::None,
                video: VideoInputPolicy::None,
            },
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
            audio_transcription: None,
        }
    }

    #[must_use]
    pub(crate) const fn opencode_zen() -> Self {
        Self {
            label: "opencode_zen",
            default_endpoint: "https://opencode.ai/zen/v1/chat/completions",
            endpoint: EndpointPolicy::UseConfiguredUrlAsExactEndpoint,
            auth: AuthPolicy::Bearer,
            extra_headers: &[],
            tool_call_ids: ToolCallIdPolicy::Preserve,
            empty_tool_call_id: EmptyToolCallIdPolicy::Uncorrelated,
            message_layout: ChatMessageLayoutPolicy::GenericOpenAI,
            tool_schema: ChatToolSchemaPolicy::OpenAIChatCompletions,
            tool_choice: ChatToolChoicePolicy::AutoWhenToolsExist,
            json_mode: JsonModePolicy::Standard,
            thinking: ChatThinkingPolicy::None,
            reasoning: ChatReasoningPolicy::OpenCodeGo {
                default_effort: "high",
            },
            streaming: ChatStreamingPolicy::NonStreaming,
            include_stream_field: true,
            rate_limit: RateLimitPolicy::RetryAfterHeader,
            response_content: ChatResponseContentPolicy::StringOnly,
            usage: UsagePolicy::PromptTokensDetailsCached,
            media: ChatMediaPolicy {
                image: ImageInputPolicy::ImageUrlDataUrl,
                audio: AudioInputPolicy::None,
                video: VideoInputPolicy::None,
            },
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
            audio_transcription: None,
        }
    }

    #[must_use]
    pub(crate) fn is_reasoning_model(&self, model_id: &str) -> bool {
        match self.reasoning {
            ChatReasoningPolicy::None | ChatReasoningPolicy::OpenCodeGo { .. } => false,
            ChatReasoningPolicy::Mistral { model_match, .. } => match model_match {
                ModelMatchPolicy::None => false,
                ModelMatchPolicy::CaseInsensitiveContains(needle) => model_id
                    .to_ascii_lowercase()
                    .contains(needle.to_ascii_lowercase().as_str()),
            },
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
        assert_eq!(p.endpoint, EndpointPolicy::AppendChatCompletions);
        assert_eq!(p.auth, AuthPolicy::Bearer);
        assert_eq!(p.tool_call_ids, ToolCallIdPolicy::Preserve);
        assert_eq!(p.message_layout, ChatMessageLayoutPolicy::GenericOpenAI);
        assert_eq!(p.json_mode, JsonModePolicy::Standard);
        assert_eq!(p.response_content, ChatResponseContentPolicy::StringOnly);
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(p.media_capabilities.supports_image_understanding);
    }

    #[test]
    fn mistral_profile_preserves_strict_layout_and_audio_policy() {
        let p = ChatCompletionsProfile::mistral();

        assert_eq!(p.label, "mistral");
        assert_eq!(p.default_endpoint, "https://api.mistral.ai/v1");
        assert_eq!(p.tool_call_ids, ToolCallIdPolicy::MistralNineAlnum);
        assert_eq!(p.message_layout, ChatMessageLayoutPolicy::MistralStrict);
        assert_eq!(p.parallel_tool_calls, Some(true));
        assert!(p.capabilities.strict_tool_history());
        assert!(p.media_capabilities.supports_audio_transcription);
        assert_eq!(
            p.audio_transcription.map(|audio| audio.endpoint_path),
            Some("/audio/transcriptions")
        );
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
        assert_eq!(p.endpoint, EndpointPolicy::UseConfiguredUrlAsExactEndpoint);
        assert_eq!(p.extra_headers, OPENROUTER_HEADERS);
        assert_eq!(p.tool_choice, ChatToolChoicePolicy::Omit);
        assert_eq!(p.rate_limit, RateLimitPolicy::OpenRouterResetMetadata);
        assert_eq!(p.media.audio, AudioInputPolicy::OpenRouterInputAudio);
        assert_eq!(p.media.video, VideoInputPolicy::OpenRouterVideoUrl);
    }

    #[test]
    fn opencode_go_profile_preserves_router_owned_exact_endpoint_and_strict_tools() {
        let p = ChatCompletionsProfile::opencode_go();

        assert_eq!(p.label, "opencode_go");
        assert_eq!(p.endpoint, EndpointPolicy::UseConfiguredUrlAsExactEndpoint);
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
