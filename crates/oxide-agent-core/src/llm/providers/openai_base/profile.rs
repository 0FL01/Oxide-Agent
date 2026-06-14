//! OpenAI-compatible provider profile.
//!
//! Profile data that controls per-provider behavioral quirks (tool-call ID
//! mapping, message layout, response parsing, temperatures, reasoning, audio
//! transcription) without requiring a separate provider implementation.

use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};

// ---------------------------------------------------------------------------
// Policy enums
// ---------------------------------------------------------------------------

/// How tool-call IDs are transformed for the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallIdStrategy {
    /// Pass IDs through unchanged (generic OpenAI-compatible providers).
    Preserve,
    /// Mistral requires exactly 9 alphanumeric characters.
    MistralNineAlnum,
}

/// How the message array is assembled from history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageLayoutPolicy {
    /// Standard OpenAI layout: system prompt first, history as-is.
    GenericOpenAI,
    /// Mistral strict layout: collect history system messages, prepend before
    /// main system prompt; map tool-call IDs; include tool result `name`.
    MistralStrict,
}

/// How response `content` is parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseContentPolicy {
    /// Content is always a plain string.
    StringOnly,
    /// Content may be a string or a chunked array with interleaved
    /// `thinking`/`reasoning`/`text` segments.
    StringOrChunkArrayWithReasoning,
}

/// When to add `response_format: {"type":"json_object"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonModePolicy {
    /// Never add `response_format`.
    None,
    /// Add `json_object` when `json_mode` is requested and no tools are present.
    /// This is the standard behavior shared by both generic and Mistral profiles.
    Standard,
}

/// Model matching for reasoning support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelMatchPolicy {
    /// No model qualifies for reasoning.
    None,
    /// Case-insensitive exact match against a model ID substring.
    CaseInsensitiveContains(&'static str),
}

/// Reasoning effort policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningPolicy {
    /// No reasoning effort support.
    None,
    /// Mistral-style reasoning: send `reasoning_effort` only for models matching
    /// the policy.
    Mistral {
        default_effort: &'static str,
        model_match: ModelMatchPolicy,
    },
}

/// Audio transcription parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioTranscriptionProfile {
    /// Path appended to the base URL (e.g. `/audio/transcriptions`).
    pub endpoint_path: &'static str,
    /// Temperature sent in the multipart form.
    pub temperature: f32,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Maximum retry attempts.
    pub max_retries: usize,
    /// Initial exponential backoff in milliseconds.
    pub initial_backoff_ms: u64,
}

// ---------------------------------------------------------------------------
// Profile struct
// ---------------------------------------------------------------------------

/// Behavioral profile for an OpenAI-compatible provider.
///
/// All fields are `Copy` / `&'static str` so the entire struct is
/// const-constructible -- no heap allocation.
#[derive(Debug, Clone, Copy)]
pub struct OpenAICompatibleProfile {
    /// Human-readable profile name for logging.
    pub name: &'static str,
    /// Default API base URL (empty = configured per-instance via env).
    pub default_api_base: &'static str,
    /// Provider request capabilities.
    pub capabilities: ProviderCapabilities,
    /// Media modality support.
    pub media_capabilities: MediaCapabilities,
    /// Temperature for plain chat requests.
    pub chat_temperature: f32,
    /// Temperature for tool-enabled chat requests.
    pub tool_temperature: f32,
    /// Temperature for reasoning model requests.
    pub reasoning_temperature: f32,
    /// Temperature for audio transcription (if supported).
    pub audio_temperature: Option<f32>,
    /// Tool-call ID transformation strategy.
    pub tool_call_id_strategy: ToolCallIdStrategy,
    /// Message array assembly policy.
    pub message_layout: MessageLayoutPolicy,
    /// Response content parsing policy.
    pub response_content: ResponseContentPolicy,
    /// JSON mode policy.
    pub json_mode: JsonModePolicy,
    /// Whether to send `parallel_tool_calls` in tool bodies.
    pub parallel_tool_calls: Option<bool>,
    /// Audio transcription configuration (if supported).
    pub audio_transcription: Option<AudioTranscriptionProfile>,
    /// Reasoning effort policy.
    pub reasoning: ReasoningPolicy,
}

impl OpenAICompatibleProfile {
    /// Mistral profile.
    #[must_use]
    pub const fn mistral() -> Self {
        Self {
            name: "mistral",
            default_api_base: "https://api.mistral.ai/v1",
            capabilities: ProviderCapabilities::new(ToolHistoryMode::Strict, true, true),
            media_capabilities: MediaCapabilities::new(true, false, false),
            chat_temperature: 0.9,
            tool_temperature: 0.7,
            reasoning_temperature: 0.7,
            audio_temperature: Some(0.4),
            tool_call_id_strategy: ToolCallIdStrategy::MistralNineAlnum,
            message_layout: MessageLayoutPolicy::MistralStrict,
            response_content: ResponseContentPolicy::StringOrChunkArrayWithReasoning,
            json_mode: JsonModePolicy::Standard,
            parallel_tool_calls: Some(true),
            audio_transcription: Some(AudioTranscriptionProfile {
                endpoint_path: "/audio/transcriptions",
                temperature: 0.4,
                timeout_secs: 120,
                max_retries: 5,
                initial_backoff_ms: 3_000,
            }),
            reasoning: ReasoningPolicy::Mistral {
                default_effort: "high",
                model_match: ModelMatchPolicy::CaseInsensitiveContains("mistral-small-2603"),
            },
        }
    }

    /// Generic OpenAI-compatible profile (default for `openai-base:*` instances).
    #[must_use]
    pub const fn generic() -> Self {
        Self {
            name: "generic",
            default_api_base: "",
            capabilities: ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true),
            media_capabilities: MediaCapabilities::new(false, true, false),
            chat_temperature: 0.7,
            tool_temperature: 0.7,
            reasoning_temperature: 0.7,
            audio_temperature: None,
            tool_call_id_strategy: ToolCallIdStrategy::Preserve,
            message_layout: MessageLayoutPolicy::GenericOpenAI,
            response_content: ResponseContentPolicy::StringOnly,
            json_mode: JsonModePolicy::Standard,
            parallel_tool_calls: None,
            audio_transcription: None,
            reasoning: ReasoningPolicy::None,
        }
    }

    /// Returns `true` when the model ID matches the reasoning policy.
    #[must_use]
    pub fn is_reasoning_model(&self, model_id: &str) -> bool {
        match self.reasoning {
            ReasoningPolicy::None => false,
            ReasoningPolicy::Mistral { model_match, .. } => match model_match {
                ModelMatchPolicy::None => false,
                ModelMatchPolicy::CaseInsensitiveContains(needle) => model_id
                    .to_ascii_lowercase()
                    .contains(needle.to_ascii_lowercase().as_str()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_profile_has_expected_values() {
        let p = OpenAICompatibleProfile::mistral();
        assert_eq!(p.name, "mistral");
        assert_eq!(p.default_api_base, "https://api.mistral.ai/v1");
        assert!(p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(p.media_capabilities.supports_audio_transcription);
        assert!(!p.media_capabilities.supports_image_understanding);
        assert!(!p.media_capabilities.supports_video_understanding);
        assert!((p.chat_temperature - 0.9).abs() < f32::EPSILON);
        assert!((p.tool_temperature - 0.7).abs() < f32::EPSILON);
        assert!((p.reasoning_temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(p.audio_temperature, Some(0.4));
        assert_eq!(
            p.tool_call_id_strategy,
            ToolCallIdStrategy::MistralNineAlnum
        );
        assert_eq!(p.message_layout, MessageLayoutPolicy::MistralStrict);
        assert_eq!(
            p.response_content,
            ResponseContentPolicy::StringOrChunkArrayWithReasoning
        );
        assert_eq!(p.json_mode, JsonModePolicy::Standard);
        assert_eq!(p.parallel_tool_calls, Some(true));
        assert!(p.audio_transcription.is_some());
        if let Some(audio) = p.audio_transcription {
            assert_eq!(audio.endpoint_path, "/audio/transcriptions");
            assert_eq!(audio.timeout_secs, 120);
            assert_eq!(audio.max_retries, 5);
        }
    }

    #[test]
    fn generic_profile_has_expected_values() {
        let p = OpenAICompatibleProfile::generic();
        assert_eq!(p.name, "generic");
        assert!(!p.capabilities.strict_tool_history());
        assert!(p.capabilities.supports_tool_calling);
        assert!(p.capabilities.supports_structured_output);
        assert!(!p.media_capabilities.supports_audio_transcription);
        assert!(p.media_capabilities.supports_image_understanding);
        assert!(!p.media_capabilities.supports_video_understanding);
        assert_eq!(p.tool_call_id_strategy, ToolCallIdStrategy::Preserve);
        assert_eq!(p.message_layout, MessageLayoutPolicy::GenericOpenAI);
        assert_eq!(p.response_content, ResponseContentPolicy::StringOnly);
        assert_eq!(p.parallel_tool_calls, None);
        assert!(p.audio_transcription.is_none());
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
