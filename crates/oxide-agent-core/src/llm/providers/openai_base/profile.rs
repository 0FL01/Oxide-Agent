//! Compatibility profile exports for the OpenAI-compatible provider wrapper.
//!
//! The canonical profile and policy definitions live in
//! `providers::chat_completions::profile`; this module keeps the legacy
//! `openai_base::profile` names used by modules and tests.

pub(crate) use crate::llm::providers::chat_completions::profile::{
    AudioTranscriptionProfile, ChatCompletionsProfile as OpenAICompatibleProfile,
};
#[cfg(test)]
pub(crate) use crate::llm::providers::chat_completions::profile::{
    ChatMessageLayoutPolicy as MessageLayoutPolicy, ChatReasoningPolicy as ReasoningPolicy,
    ChatResponseContentPolicy as ResponseContentPolicy, ChatStreamingPolicy as StreamPolicy,
    ChatThinkingPolicy as ThinkingPolicy, JsonModePolicy, StructuredOutputPolicy,
    ToolCallIdPolicy as ToolCallIdStrategy,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_profile_has_expected_values() {
        let p = OpenAICompatibleProfile::mistral();
        assert_eq!(p.label, "mistral");
        assert_eq!(p.default_endpoint, "https://api.mistral.ai/v1");
        assert!(p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(p.media_capabilities.supports_audio_transcription);
        assert!(!p.media_capabilities.supports_image_understanding);
        assert!(!p.media_capabilities.supports_video_understanding);
        assert!((p.temperatures.chat - 0.9).abs() < f32::EPSILON);
        assert!((p.temperatures.tools - 0.7).abs() < f32::EPSILON);
        assert!((p.temperatures.reasoning - 0.7).abs() < f32::EPSILON);
        assert_eq!(p.tool_call_ids, ToolCallIdStrategy::MistralNineAlnum);
        assert_eq!(p.message_layout, MessageLayoutPolicy::MistralStrict);
        assert_eq!(
            p.response_content,
            ResponseContentPolicy::StringOrChunkArrayWithReasoning
        );
        assert_eq!(p.json_mode, JsonModePolicy::Standard);
        assert_eq!(p.parallel_tool_calls, Some(true));
        assert!(p.audio_transcription.is_some());
        assert_eq!(p.thinking, ThinkingPolicy::None);
        assert_eq!(p.streaming, StreamPolicy::NonStreaming);
        assert_eq!(p.structured_output, StructuredOutputPolicy::BaseCapability);
        if let Some(audio) = p.audio_transcription {
            assert_eq!(audio.endpoint_path, "/audio/transcriptions");
            assert_eq!(audio.temperature, 0.4);
            assert_eq!(audio.timeout_secs, 120);
            assert_eq!(audio.max_retries, 5);
        }
    }

    #[test]
    fn zai_profile_has_expected_values() {
        let p = OpenAICompatibleProfile::zai();
        assert_eq!(p.label, "zai");
        assert_eq!(p.default_endpoint, "https://api.z.ai/api/coding/paas/v4");
        assert!(!p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(!p.capabilities.supports_structured_output);
        assert!(!p.media_capabilities.supports_audio_transcription);
        assert!(p.media_capabilities.supports_image_understanding);
        assert!(!p.media_capabilities.supports_video_understanding);
        assert!((p.temperatures.chat - 0.95).abs() < f32::EPSILON);
        assert!((p.temperatures.tools - 0.95).abs() < f32::EPSILON);
        assert!((p.temperatures.reasoning - 0.95).abs() < f32::EPSILON);
        assert_eq!(p.tool_call_ids, ToolCallIdStrategy::Preserve);
        assert_eq!(p.message_layout, MessageLayoutPolicy::GenericOpenAI);
        assert_eq!(
            p.response_content,
            ResponseContentPolicy::StringOrChunkArrayWithReasoning
        );
        assert_eq!(p.json_mode, JsonModePolicy::Standard);
        assert_eq!(p.parallel_tool_calls, None);
        assert!(p.audio_transcription.is_none());
        assert_eq!(p.reasoning, ReasoningPolicy::None);
        assert_eq!(p.thinking, ThinkingPolicy::ZaiEnabledUnlessJsonMode);
        assert_eq!(p.streaming, StreamPolicy::ZaiUnlessNativeJsonMode);
        assert_eq!(
            p.structured_output,
            StructuredOutputPolicy::ZaiGlmToolModelsOnly
        );
    }

    #[test]
    fn zai_structured_output_is_model_gated() {
        let p = OpenAICompatibleProfile::zai();
        assert!(
            p.capabilities_for_model("glm-4.6")
                .supports_structured_output
        );
        assert!(
            p.capabilities_for_model(" subagent ")
                .supports_structured_output
        );
        assert!(!p.capabilities_for_model("glm-5").supports_structured_output);
    }

    #[test]
    fn generic_profile_has_expected_values() {
        let p = OpenAICompatibleProfile::generic();
        assert_eq!(p.label, "generic");
        assert!(!p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(!p.media_capabilities.supports_audio_transcription);
        assert!(p.media_capabilities.supports_image_understanding);
        assert!(!p.media_capabilities.supports_video_understanding);
        assert_eq!(p.tool_call_ids, ToolCallIdStrategy::Preserve);
        assert_eq!(p.message_layout, MessageLayoutPolicy::GenericOpenAI);
        assert_eq!(p.response_content, ResponseContentPolicy::StringOnly);
        assert_eq!(p.parallel_tool_calls, None);
        assert!(p.audio_transcription.is_none());
        assert_eq!(p.thinking, ThinkingPolicy::None);
        assert_eq!(p.streaming, StreamPolicy::NonStreaming);
        assert_eq!(p.structured_output, StructuredOutputPolicy::BaseCapability);
    }

    #[test]
    fn mistral_reasoning_model_match() {
        let p = OpenAICompatibleProfile::mistral();
        assert!(p.is_reasoning_model("mistral-small-2603"));
        assert!(p.is_reasoning_model("Mistral-Small-2603"));
        assert!(!p.is_reasoning_model("mistral-large-latest"));
    }

    #[test]
    fn generic_never_reasoning() {
        let p = OpenAICompatibleProfile::generic();
        assert!(!p.is_reasoning_model("anything"));
    }
}
