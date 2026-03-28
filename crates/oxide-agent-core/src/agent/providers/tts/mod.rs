//! Kokoro TTS (Text-to-Speech) Provider
//!
//! Provides `text_to_speech_en` tool for generating voice messages.
//! Uses local Kokoro TTS server running at 127.0.0.1:8000.
//!
//! # Configuration
//!
//! Environment variables:
//! - `KOKORO_TTS_URL` - API base URL (default: http://127.0.0.1:8000)
//! - `KOKORO_TTS_VOICE` - Default voice (default: af_heart)
//! - `KOKORO_TTS_FORMAT` - Default format (default: ogg)
//! - `KOKORO_TTS_TIMEOUT_SECS` - Request timeout (default: 60)
//!
//! # Available Voices
//!
//! - `af_bella` - Default female voice
//! - `af_aoede` - Alternative female voice
//! - `af_alloy` - Neutral voice
//! - `af_heart` - Warm female voice (default)
//!
//! # Audio Formats
//!
//! - `ogg` - OGG/Opus, best for Telegram voice messages (default)
//! - `mp3` - MPEG, general compatibility
//! - `wav` - PCM, lossless for editing
//!
//! # Language Support
//!
//! **Important:** Kokoro TTS supports English language only.
//! The tool description instructs the agent to provide English text.

pub mod client;
pub mod provider;
pub mod types;

pub use client::KokoroClient;
pub use provider::KokoroTtsProvider;
pub use types::{
    TextToSpeechArgs, TtsConfig, TtsFormat, TtsRequest, TtsVoice, DEFAULT_FORMAT, DEFAULT_SPEED,
    DEFAULT_VOICE,
};
