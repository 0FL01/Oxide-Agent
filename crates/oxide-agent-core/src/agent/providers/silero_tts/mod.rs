//! Silero TTS (Text-to-Speech) Provider.
//!
//! Provides `text_to_speech_ru` tool for generating Russian voice messages.
//! Uses local Silero TTS server running at 127.0.0.1:8001 by default.
//!
//! # Configuration
//!
//! Environment variables:
//! - `SILERO_TTS_URL` - API base URL (default: http://127.0.0.1:8001)
//! - `SILERO_TTS_SPEAKER` - Default speaker (default: baya)
//! - `SILERO_TTS_FORMAT` - Default format (default: ogg)
//! - `SILERO_TTS_SAMPLE_RATE` - Default sample rate (default: 48000)
//! - `SILERO_TTS_TIMEOUT_SECS` - Request timeout (default: 60)
//!
//! # Available Speakers
//!
//! - `aidar` - male voice
//! - `baya` - female voice (default, recommended)
//! - `kseniya` - female voice
//! - `xenia` - female voice
//!
//! # Audio Formats
//!
//! - `ogg` - OGG/Opus, best for Telegram voice messages (default)
//! - `wav` - PCM, lossless
//!
//! # SSML Support
//!
//! Silero supports SSML markup for enhanced speech control:
//! - `<speak>` - root element
//! - `<break time="200ms"/>` - pauses
//! - `<prosody pitch="high" rate="slow">` - pitch and rate control
//!
//! Example SSML:
//! ```xml
//! <speak>Привет<break time="300ms"/>это тест</speak>
//! ```

pub mod client;
pub mod provider;
pub mod types;

pub use client::SileroClient;
pub use provider::SileroTtsProvider;
pub use types::{
    SileroSampleRate, SileroTtsConfig, SileroTtsFormat, SileroTtsRequest, SileroTtsSpeaker,
    TextToSpeechRuArgs, DEFAULT_FORMAT, DEFAULT_SAMPLE_RATE, DEFAULT_SPEAKER,
};
