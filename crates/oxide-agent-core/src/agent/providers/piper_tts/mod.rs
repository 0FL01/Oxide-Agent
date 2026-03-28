//! Piper TTS (Text-to-Speech) Provider.
//!
//! Provides `text_to_speech_ru` tool for generating Russian voice messages.
//! Uses local Piper TTS server running at 127.0.0.1:8001.
//!
//! # Configuration
//!
//! Environment variables:
//! - `PIPER_TTS_URL` - API base URL (default: http://127.0.0.1:8001)
//! - `PIPER_TTS_VOICE` - Default voice (default: ruslan)
//! - `PIPER_TTS_FORMAT` - Default format (default: ogg)
//! - `PIPER_TTS_TIMEOUT_SECS` - Request timeout (default: 60)
//!
//! # Available Voices
//!
//! - `denis`
//! - `dmitri`
//! - `irina`
//! - `ruslan` (default)
//!
//! # Audio Formats
//!
//! - `ogg` - OGG/Opus, best for Telegram voice messages (default)
//! - `mp3` - MPEG, general compatibility
//! - `wav` - PCM, lossless for editing

pub mod client;
pub mod provider;
pub mod types;

pub use client::PiperClient;
pub use provider::PiperTtsProvider;
pub use types::{
    PiperTtsConfig, PiperTtsFormat, PiperTtsRequest, PiperTtsVoice, TextToSpeechRuArgs,
    DEFAULT_FORMAT, DEFAULT_NOISE_SCALE, DEFAULT_NOISE_W_SCALE, DEFAULT_SENTENCE_SILENCE,
    DEFAULT_SPEED, DEFAULT_VOICE, DEFAULT_VOLUME,
};
