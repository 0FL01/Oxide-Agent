//! Types and constants for Mistral AI provider

/// Reasoning model identifier
pub const MISTRAL_REASONING_MODEL_ID: &str = "mistral-small-2603";

/// Default reasoning effort for reasoning models
pub const MISTRAL_REASONING_EFFORT: &str = "high";

/// Supported transcription models
/// Note: voxtral-mini-realtime-26-02 is NOT supported (realtime streaming is incompatible)
pub const MISTRAL_TRANSCRIPTION_MODELS: &[&str] =
    &["voxtral-mini-latest", "voxtral-mini-transcribe-26-02"];
