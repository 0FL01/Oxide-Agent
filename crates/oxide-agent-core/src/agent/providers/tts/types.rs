//! Kokoro TTS types and configuration
//!
//! Defines request/response structures, voice/format enums, and configuration.

use serde::{Deserialize, Serialize};

/// Default Kokoro TTS API URL
pub const DEFAULT_KOKORO_URL: &str = "http://127.0.0.1:8000";

/// Default voice (af_heart)
pub const DEFAULT_VOICE: &str = "af_heart";

/// Default speech speed
pub const DEFAULT_SPEED: f32 = 1.0;

/// Default audio format for Telegram
pub const DEFAULT_FORMAT: &str = "ogg";

/// Available Kokoro voices
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TtsVoice {
    /// af_bella - Default female voice
    Bella,
    /// af_aoede - Alternative female voice
    Aoede,
    /// af_alloy - Neutral voice
    Alloy,
    /// af_heart - Warm female voice (default)
    #[default]
    Heart,
}

impl TtsVoice {
    /// Get voice name as string for API
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bella => "af_bella",
            Self::Aoede => "af_aoede",
            Self::Alloy => "af_alloy",
            Self::Heart => "af_heart",
        }
    }
}

impl std::str::FromStr for TtsVoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "af_bella" | "bella" => Ok(Self::Bella),
            "af_aoede" | "aoede" => Ok(Self::Aoede),
            "af_alloy" | "alloy" => Ok(Self::Alloy),
            "af_heart" | "heart" => Ok(Self::Heart),
            _ => Err(format!("Unknown voice: {s}")),
        }
    }
}

/// Available audio formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TtsFormat {
    /// OGG/Opus - Best for Telegram voice messages
    #[default]
    Ogg,
    /// MP3 - General playback
    Mp3,
    /// WAV - Lossless, for editing
    Wav,
}

impl TtsFormat {
    /// Get format name as string for API
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ogg => "ogg",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    /// Get MIME type for the format
    #[must_use]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Ogg => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }
}

impl std::str::FromStr for TtsFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ogg" | "opus" => Ok(Self::Ogg),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "wav" => Ok(Self::Wav),
            _ => Err(format!("Unknown format: {s}")),
        }
    }
}

/// TTS synthesis request
#[derive(Debug, Clone, Serialize)]
pub struct TtsRequest {
    /// Text to synthesize (must be in English)
    pub text: String,
    /// Language code (always "en" for Kokoro)
    pub lang: String,
    /// Voice name
    pub voice: String,
    /// Speech speed (default: 1.0)
    pub speed: f32,
    /// Output format
    pub format: String,
}

impl TtsRequest {
    /// Create a new TTS request with defaults
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            lang: "en".to_string(),
            voice: DEFAULT_VOICE.to_string(),
            speed: DEFAULT_SPEED,
            format: DEFAULT_FORMAT.to_string(),
        }
    }

    /// Set voice
    #[must_use]
    pub fn with_voice(mut self, voice: TtsVoice) -> Self {
        self.voice = voice.as_str().to_string();
        self
    }

    /// Set format
    #[must_use]
    pub fn with_format(mut self, format: TtsFormat) -> Self {
        self.format = format.as_str().to_string();
        self
    }

    /// Set speed
    #[must_use]
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed.clamp(0.5, 2.0);
        self
    }

    /// Validate that text contains only ASCII characters (English)
    ///
    /// Returns warning message if non-ASCII detected
    #[must_use]
    pub fn validate_english(&self) -> Option<String> {
        if !self.text.is_ascii() {
            Some(
                "Warning: Text contains non-ASCII characters. \
                 Kokoro TTS supports English only - pronunciation may be incorrect."
                    .to_string(),
            )
        } else {
            None
        }
    }
}

/// Kokoro TTS client configuration
#[derive(Debug, Clone)]
pub struct TtsConfig {
    /// Base URL for Kokoro API
    pub base_url: String,
    /// Default voice
    pub default_voice: TtsVoice,
    /// Default format
    pub default_format: TtsFormat,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl TtsConfig {
    /// Create config from environment
    #[must_use]
    pub fn from_env() -> Self {
        let base_url =
            std::env::var("KOKORO_TTS_URL").unwrap_or_else(|_| DEFAULT_KOKORO_URL.to_string());

        let default_voice = std::env::var("KOKORO_TTS_VOICE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let default_format = std::env::var("KOKORO_TTS_FORMAT")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();

        let timeout_secs = std::env::var("KOKORO_TTS_TIMEOUT_SECS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(60);

        Self {
            base_url,
            default_voice,
            default_format,
            timeout_secs,
        }
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_KOKORO_URL.to_string(),
            default_voice: TtsVoice::default(),
            default_format: TtsFormat::default(),
            timeout_secs: 60,
        }
    }
}

/// Tool arguments for text_to_speech
#[derive(Debug, Deserialize)]
pub struct TextToSpeechArgs {
    /// Text to convert to speech (must be in English)
    pub text: String,
    /// Voice name (optional, default: af_heart)
    pub voice: Option<String>,
    /// Audio format: "ogg", "mp3", or "wav" (optional, default: "ogg")
    pub format: Option<String>,
    /// Speech speed 0.5-2.0 (optional, default: 1.0)
    pub speed: Option<f32>,
}

impl TextToSpeechArgs {
    /// Convert to TtsRequest
    ///
    /// # Errors
    ///
    /// Returns error if voice or format is invalid
    pub fn to_request(&self, config: &TtsConfig) -> Result<TtsRequest, String> {
        let mut req = TtsRequest::new(&self.text);

        // Apply voice
        if let Some(ref v) = self.voice {
            let voice: TtsVoice = v.parse()?;
            req = req.with_voice(voice);
        } else {
            req = req.with_voice(config.default_voice);
        }

        // Apply format
        if let Some(ref f) = self.format {
            let format: TtsFormat = f.parse()?;
            req = req.with_format(format);
        } else {
            req = req.with_format(config.default_format);
        }

        // Apply speed
        if let Some(s) = self.speed {
            req = req.with_speed(s);
        }

        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_parsing() {
        assert_eq!("af_bella".parse::<TtsVoice>().unwrap(), TtsVoice::Bella);
        assert_eq!("bella".parse::<TtsVoice>().unwrap(), TtsVoice::Bella);
        assert_eq!("af_heart".parse::<TtsVoice>().unwrap(), TtsVoice::Heart);
        assert!("unknown".parse::<TtsVoice>().is_err());
    }

    #[test]
    fn format_parsing() {
        assert_eq!("ogg".parse::<TtsFormat>().unwrap(), TtsFormat::Ogg);
        assert_eq!("opus".parse::<TtsFormat>().unwrap(), TtsFormat::Ogg);
        assert_eq!("mp3".parse::<TtsFormat>().unwrap(), TtsFormat::Mp3);
        assert!("unknown".parse::<TtsFormat>().is_err());
    }

    #[test]
    fn request_validation() {
        let req = TtsRequest::new("Hello world");
        assert!(req.validate_english().is_none());

        let req = TtsRequest::new("Привет мир");
        assert!(req.validate_english().is_some());
    }

    #[test]
    fn args_to_request() {
        let config = TtsConfig::default();
        let args = TextToSpeechArgs {
            text: "Hello".to_string(),
            voice: Some("af_bella".to_string()),
            format: Some("mp3".to_string()),
            speed: Some(1.2),
        };

        let req = args.to_request(&config).unwrap();
        assert_eq!(req.text, "Hello");
        assert_eq!(req.voice, "af_bella");
        assert_eq!(req.format, "mp3");
        assert!((req.speed - 1.2).abs() < f32::EPSILON);
    }
}
