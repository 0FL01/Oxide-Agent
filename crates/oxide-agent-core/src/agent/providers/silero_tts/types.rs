//! Silero TTS types and configuration.

use serde::{Deserialize, Serialize};

/// Default Silero TTS API URL.
pub const DEFAULT_SILERO_URL: &str = "http://127.0.0.1:8001";

/// Default Silero speaker.
pub const DEFAULT_SPEAKER: &str = "baya";

/// Default audio format for Telegram.
pub const DEFAULT_FORMAT: &str = "ogg";

/// Default sample rate.
pub const DEFAULT_SAMPLE_RATE: u32 = 48000;

/// Available Silero speakers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SileroTtsSpeaker {
    /// `aidar` speaker.
    Aidar,
    /// `baya` speaker (default).
    #[default]
    Baya,
    /// `kseniya` speaker.
    Kseniya,
    /// `xenia` speaker.
    Xenia,
}

impl SileroTtsSpeaker {
    /// Get speaker name as string for the API.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Aidar => "aidar",
            Self::Baya => "baya",
            Self::Kseniya => "kseniya",
            Self::Xenia => "xenia",
        }
    }
}

impl std::str::FromStr for SileroTtsSpeaker {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "aidar" => Ok(Self::Aidar),
            "baya" => Ok(Self::Baya),
            "kseniya" => Ok(Self::Kseniya),
            "xenia" => Ok(Self::Xenia),
            _ => Err(format!("Unknown Silero speaker: {s}")),
        }
    }
}

/// Available Silero audio formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SileroTtsFormat {
    /// WAV audio.
    Wav,
    /// OGG/Opus audio (default, best for Telegram).
    #[default]
    Ogg,
}

impl SileroTtsFormat {
    /// Get format name as string for the API.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Ogg => "ogg",
        }
    }
}

impl std::str::FromStr for SileroTtsFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav" => Ok(Self::Wav),
            "ogg" | "opus" => Ok(Self::Ogg),
            _ => Err(format!("Unknown Silero format: {s}")),
        }
    }
}

/// Available sample rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SileroSampleRate {
    /// 8000 Hz.
    #[default]
    R8000 = 8000,
    /// 24000 Hz.
    R24000 = 24000,
    /// 48000 Hz.
    R48000 = 48000,
}

impl SileroSampleRate {
    /// Get sample rate as integer.
    #[must_use]
    pub fn as_u32(&self) -> u32 {
        *self as u32
    }
}

impl std::str::FromStr for SileroSampleRate {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "8000" => Ok(Self::R8000),
            "24000" => Ok(Self::R24000),
            "48000" => Ok(Self::R48000),
            _ => Err(format!(
                "Invalid sample_rate: {s}. Allowed: 8000, 24000, 48000"
            )),
        }
    }
}

/// Silero synthesis request.
#[derive(Debug, Clone, Serialize)]
pub struct SileroTtsRequest {
    /// Text to synthesize (plain text or SSML).
    pub text: String,
    /// Speaker name.
    pub speaker: String,
    /// Sample rate (8000, 24000, 48000).
    pub sample_rate: u32,
    /// Output audio format ("wav" or "ogg").
    pub format: String,
    /// Whether text is SSML.
    pub ssml: bool,
}

impl SileroTtsRequest {
    /// Create a new request with defaults.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            speaker: DEFAULT_SPEAKER.to_string(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            format: DEFAULT_FORMAT.to_string(),
            ssml: false,
        }
    }

    /// Set speaker.
    #[must_use]
    pub fn with_speaker(mut self, speaker: SileroTtsSpeaker) -> Self {
        self.speaker = speaker.as_str().to_string();
        self
    }

    /// Set format.
    #[must_use]
    pub fn with_format(mut self, format: SileroTtsFormat) -> Self {
        self.format = format.as_str().to_string();
        self
    }

    /// Set sample rate.
    #[must_use]
    pub fn with_sample_rate(mut self, sample_rate: SileroSampleRate) -> Self {
        self.sample_rate = sample_rate.as_u32();
        self
    }

    /// Set SSML flag.
    #[must_use]
    pub fn with_ssml(mut self, ssml: bool) -> Self {
        self.ssml = ssml;
        self
    }
}

/// Silero TTS client configuration.
#[derive(Debug, Clone)]
pub struct SileroTtsConfig {
    /// Base URL for the Silero API.
    pub base_url: String,
    /// Default speaker.
    pub default_speaker: SileroTtsSpeaker,
    /// Default output audio format.
    pub default_format: SileroTtsFormat,
    /// Default sample rate.
    pub default_sample_rate: SileroSampleRate,
    /// HTTP request timeout in seconds.
    pub timeout_secs: u64,
}

impl SileroTtsConfig {
    /// Create config from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let base_url =
            std::env::var("SILERO_TTS_URL").unwrap_or_else(|_| DEFAULT_SILERO_URL.to_string());

        let default_speaker = std::env::var("SILERO_TTS_SPEAKER")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let default_format = std::env::var("SILERO_TTS_FORMAT")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();

        let default_sample_rate = std::env::var("SILERO_TTS_SAMPLE_RATE")
            .ok()
            .and_then(|r| r.parse().ok())
            .unwrap_or_default();

        let timeout_secs = std::env::var("SILERO_TTS_TIMEOUT_SECS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(60);

        Self {
            base_url,
            default_speaker,
            default_format,
            default_sample_rate,
            timeout_secs,
        }
    }
}

impl Default for SileroTtsConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_SILERO_URL.to_string(),
            default_speaker: SileroTtsSpeaker::default(),
            default_format: SileroTtsFormat::default(),
            default_sample_rate: SileroSampleRate::default(),
            timeout_secs: 60,
        }
    }
}

/// Tool arguments for text_to_speech_ru.
#[derive(Debug, Deserialize)]
pub struct TextToSpeechRuArgs {
    /// Russian text to convert to speech.
    pub text: String,
    /// Speaker override.
    pub speaker: Option<String>,
    /// Sample rate override (8000, 24000, 48000).
    pub sample_rate: Option<u32>,
    /// Output audio format override ("wav" or "ogg").
    pub format: Option<String>,
    /// Whether text is SSML.
    pub ssml: Option<bool>,
}

impl TextToSpeechRuArgs {
    /// Convert tool arguments to a Silero request.
    pub fn to_request(&self, config: &SileroTtsConfig) -> Result<SileroTtsRequest, String> {
        let mut req = SileroTtsRequest::new(&self.text);

        if let Some(ref speaker) = self.speaker {
            req = req.with_speaker(speaker.parse()?);
        } else {
            req = req.with_speaker(config.default_speaker);
        }

        if let Some(ref format) = self.format {
            req = req.with_format(format.parse()?);
        } else {
            req = req.with_format(config.default_format);
        }

        if let Some(sample_rate) = self.sample_rate {
            let rate: SileroSampleRate = sample_rate.to_string().parse()?;
            req = req.with_sample_rate(rate);
        } else {
            req = req.with_sample_rate(config.default_sample_rate);
        }

        if let Some(ssml) = self.ssml {
            req = req.with_ssml(ssml);
        }

        Ok(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_parsing() {
        assert_eq!(
            "baya"
                .parse::<SileroTtsSpeaker>()
                .expect("baya should parse into Baya"),
            SileroTtsSpeaker::Baya
        );
        assert_eq!(
            "aidar"
                .parse::<SileroTtsSpeaker>()
                .expect("aidar should parse into Aidar"),
            SileroTtsSpeaker::Aidar
        );
        assert_eq!(
            "kseniya"
                .parse::<SileroTtsSpeaker>()
                .expect("kseniya should parse into Kseniya"),
            SileroTtsSpeaker::Kseniya
        );
        assert_eq!(
            "xenia"
                .parse::<SileroTtsSpeaker>()
                .expect("xenia should parse into Xenia"),
            SileroTtsSpeaker::Xenia
        );
        assert!("unknown".parse::<SileroTtsSpeaker>().is_err());
    }

    #[test]
    fn format_parsing() {
        assert_eq!(
            "ogg"
                .parse::<SileroTtsFormat>()
                .expect("ogg should parse into Ogg"),
            SileroTtsFormat::Ogg
        );
        assert_eq!(
            "wav"
                .parse::<SileroTtsFormat>()
                .expect("wav should parse into Wav"),
            SileroTtsFormat::Wav
        );
        assert!("mp3".parse::<SileroTtsFormat>().is_err());
    }

    #[test]
    fn sample_rate_parsing() {
        assert_eq!(
            "8000"
                .parse::<SileroSampleRate>()
                .expect("8000 should parse"),
            SileroSampleRate::R8000
        );
        assert_eq!(
            "24000"
                .parse::<SileroSampleRate>()
                .expect("24000 should parse"),
            SileroSampleRate::R24000
        );
        assert_eq!(
            "48000"
                .parse::<SileroSampleRate>()
                .expect("48000 should parse"),
            SileroSampleRate::R48000
        );
        assert!("44100".parse::<SileroSampleRate>().is_err());
    }

    #[test]
    fn request_defaults() {
        let req = SileroTtsRequest::new("Привет");

        assert_eq!(req.text, "Привет");
        assert_eq!(req.speaker, "baya");
        assert_eq!(req.format, "ogg");
        assert_eq!(req.sample_rate, 48000);
        assert!(!req.ssml);
    }

    #[test]
    fn args_to_request_applies_defaults() {
        let config = SileroTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            speaker: None,
            sample_rate: None,
            format: None,
            ssml: None,
        };

        let req = args
            .to_request(&config)
            .expect("args should convert into a valid Silero request");
        assert_eq!(req.speaker, "baya");
        assert_eq!(req.format, "ogg");
        assert_eq!(req.sample_rate, 48000);
        assert!(!req.ssml);
    }

    #[test]
    fn args_to_request_applies_overrides() {
        let config = SileroTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            speaker: Some("aidar".to_string()),
            sample_rate: Some(24000),
            format: Some("wav".to_string()),
            ssml: Some(true),
        };

        let req = args
            .to_request(&config)
            .expect("args should convert into a valid Silero request");
        assert_eq!(req.speaker, "aidar");
        assert_eq!(req.format, "wav");
        assert_eq!(req.sample_rate, 24000);
        assert!(req.ssml);
    }

    #[test]
    fn args_to_request_rejects_invalid_values() {
        let config = SileroTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            speaker: Some("invalid".to_string()),
            sample_rate: None,
            format: None,
            ssml: None,
        };

        assert!(args.to_request(&config).is_err());
    }
}
