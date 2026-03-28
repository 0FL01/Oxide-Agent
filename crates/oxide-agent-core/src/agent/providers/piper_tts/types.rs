//! Piper TTS types and configuration.

use serde::{Deserialize, Serialize};

/// Default Piper TTS API URL.
pub const DEFAULT_PIPER_URL: &str = "http://127.0.0.1:8001";

/// Default Piper voice alias.
pub const DEFAULT_VOICE: &str = "ruslan";

/// Default speech speed tuned for a more natural delivery.
pub const DEFAULT_SPEED: f32 = 0.9;

/// Default noise scale tuned for a more natural delivery.
pub const DEFAULT_NOISE_SCALE: f32 = 0.62;

/// Default word noise scale tuned for a more natural delivery.
pub const DEFAULT_NOISE_W_SCALE: f32 = 0.78;

/// Default output volume.
pub const DEFAULT_VOLUME: f32 = 1.0;

/// Default pause between sentences in seconds.
pub const DEFAULT_SENTENCE_SILENCE: f32 = 0.10;

/// Default audio format for Telegram.
pub const DEFAULT_FORMAT: &str = "ogg";

const DEFAULT_NORMALIZE_AUDIO: bool = true;

/// Available Piper voices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PiperTtsVoice {
    /// `denis` voice alias.
    Denis,
    /// `dmitri` voice alias.
    Dmitri,
    /// `irina` voice alias.
    Irina,
    /// `ruslan` voice alias.
    #[default]
    Ruslan,
}

impl PiperTtsVoice {
    /// Get voice alias as string for the API.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Denis => "denis",
            Self::Dmitri => "dmitri",
            Self::Irina => "irina",
            Self::Ruslan => "ruslan",
        }
    }
}

impl std::str::FromStr for PiperTtsVoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "denis" => Ok(Self::Denis),
            "dmitri" => Ok(Self::Dmitri),
            "irina" => Ok(Self::Irina),
            "ruslan" => Ok(Self::Ruslan),
            _ => Err(format!("Unknown Piper voice: {s}")),
        }
    }
}

/// Available Piper audio formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PiperTtsFormat {
    /// OGG/Opus audio.
    #[default]
    Ogg,
    /// MP3 audio.
    Mp3,
    /// WAV audio.
    Wav,
}

impl PiperTtsFormat {
    /// Get format name as string for the API.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ogg => "ogg",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }
}

impl std::str::FromStr for PiperTtsFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ogg" | "opus" => Ok(Self::Ogg),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "wav" => Ok(Self::Wav),
            _ => Err(format!("Unknown Piper format: {s}")),
        }
    }
}

/// Piper synthesis request.
#[derive(Debug, Clone, Serialize)]
pub struct PiperTtsRequest {
    /// Text to synthesize.
    pub text: String,
    /// Voice alias.
    pub voice: String,
    /// Output audio format.
    pub format: String,
    /// Optional duration scaling factor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_scale: Option<f32>,
    /// Speech variability.
    pub noise_scale: f32,
    /// Word-level variability.
    pub noise_w_scale: f32,
    /// Output volume multiplier.
    pub volume: f32,
    /// Whether to normalize the generated audio.
    pub normalize_audio: bool,
    /// Speech speed multiplier.
    pub speed: f32,
    /// Pause between sentences in seconds.
    pub sentence_silence: f32,
}

impl PiperTtsRequest {
    /// Create a new request with the natural-speech defaults.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice: DEFAULT_VOICE.to_string(),
            format: DEFAULT_FORMAT.to_string(),
            length_scale: None,
            noise_scale: DEFAULT_NOISE_SCALE,
            noise_w_scale: DEFAULT_NOISE_W_SCALE,
            volume: DEFAULT_VOLUME,
            normalize_audio: DEFAULT_NORMALIZE_AUDIO,
            speed: DEFAULT_SPEED,
            sentence_silence: DEFAULT_SENTENCE_SILENCE,
        }
    }

    /// Set voice.
    #[must_use]
    pub fn with_voice(mut self, voice: PiperTtsVoice) -> Self {
        self.voice = voice.as_str().to_string();
        self
    }

    /// Set format.
    #[must_use]
    pub fn with_format(mut self, format: PiperTtsFormat) -> Self {
        self.format = format.as_str().to_string();
        self
    }

    /// Set optional length scale.
    #[must_use]
    pub fn with_length_scale(mut self, length_scale: Option<f32>) -> Self {
        self.length_scale = length_scale;
        self
    }

    /// Set noise scale.
    #[must_use]
    pub fn with_noise_scale(mut self, noise_scale: f32) -> Self {
        self.noise_scale = noise_scale;
        self
    }

    /// Set word-level noise scale.
    #[must_use]
    pub fn with_noise_w_scale(mut self, noise_w_scale: f32) -> Self {
        self.noise_w_scale = noise_w_scale;
        self
    }

    /// Set output volume.
    #[must_use]
    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume;
        self
    }

    /// Set audio normalization flag.
    #[must_use]
    pub fn with_normalize_audio(mut self, normalize_audio: bool) -> Self {
        self.normalize_audio = normalize_audio;
        self
    }

    /// Set speech speed.
    #[must_use]
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Set sentence silence duration.
    #[must_use]
    pub fn with_sentence_silence(mut self, sentence_silence: f32) -> Self {
        self.sentence_silence = sentence_silence;
        self
    }
}

/// Piper TTS client configuration.
#[derive(Debug, Clone)]
pub struct PiperTtsConfig {
    /// Base URL for the Piper API.
    pub base_url: String,
    /// Default voice alias.
    pub default_voice: PiperTtsVoice,
    /// Default output audio format.
    pub default_format: PiperTtsFormat,
    /// HTTP request timeout in seconds.
    pub timeout_secs: u64,
}

impl PiperTtsConfig {
    /// Create config from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        let base_url =
            std::env::var("PIPER_TTS_URL").unwrap_or_else(|_| DEFAULT_PIPER_URL.to_string());

        let default_voice = std::env::var("PIPER_TTS_VOICE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let default_format = std::env::var("PIPER_TTS_FORMAT")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or_default();

        let timeout_secs = std::env::var("PIPER_TTS_TIMEOUT_SECS")
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

impl Default for PiperTtsConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_PIPER_URL.to_string(),
            default_voice: PiperTtsVoice::default(),
            default_format: PiperTtsFormat::default(),
            timeout_secs: 60,
        }
    }
}

/// Tool arguments for text_to_speech_ru.
#[derive(Debug, Deserialize)]
pub struct TextToSpeechRuArgs {
    /// Russian text to convert to speech.
    pub text: String,
    /// Voice alias override.
    pub voice: Option<String>,
    /// Output audio format override.
    pub format: Option<String>,
    /// Optional duration scaling factor.
    pub length_scale: Option<f32>,
    /// Speech speed override.
    pub speed: Option<f32>,
    /// Speech variability override.
    pub noise_scale: Option<f32>,
    /// Word-level variability override.
    pub noise_w_scale: Option<f32>,
    /// Volume override.
    pub volume: Option<f32>,
    /// Audio normalization override.
    pub normalize_audio: Option<bool>,
    /// Pause between sentences override.
    pub sentence_silence: Option<f32>,
}

impl TextToSpeechRuArgs {
    /// Convert tool arguments to a Piper request.
    pub fn to_request(&self, config: &PiperTtsConfig) -> Result<PiperTtsRequest, String> {
        let mut req = PiperTtsRequest::new(&self.text);

        if let Some(ref voice) = self.voice {
            req = req.with_voice(voice.parse()?);
        } else {
            req = req.with_voice(config.default_voice);
        }

        if let Some(ref format) = self.format {
            req = req.with_format(format.parse()?);
        } else {
            req = req.with_format(config.default_format);
        }

        if let Some(length_scale) = self.length_scale {
            validate_positive("length_scale", length_scale)?;
            req = req.with_length_scale(Some(length_scale));
        }

        if let Some(speed) = self.speed {
            validate_positive("speed", speed)?;
            req = req.with_speed(speed);
        }

        if let Some(noise_scale) = self.noise_scale {
            validate_positive("noise_scale", noise_scale)?;
            req = req.with_noise_scale(noise_scale);
        }

        if let Some(noise_w_scale) = self.noise_w_scale {
            validate_positive("noise_w_scale", noise_w_scale)?;
            req = req.with_noise_w_scale(noise_w_scale);
        }

        if let Some(volume) = self.volume {
            validate_positive("volume", volume)?;
            req = req.with_volume(volume);
        }

        if let Some(normalize_audio) = self.normalize_audio {
            req = req.with_normalize_audio(normalize_audio);
        }

        if let Some(sentence_silence) = self.sentence_silence {
            if !(0.0..=2.0).contains(&sentence_silence) {
                return Err("sentence_silence must be between 0.0 and 2.0 seconds".to_string());
            }
            req = req.with_sentence_silence(sentence_silence);
        }

        Ok(req)
    }
}

fn validate_positive(name: &str, value: f32) -> Result<(), String> {
    if value <= 0.0 {
        return Err(format!("{name} must be greater than 0"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_parsing() {
        assert_eq!(
            "ruslan"
                .parse::<PiperTtsVoice>()
                .expect("ruslan should parse into Ruslan"),
            PiperTtsVoice::Ruslan
        );
        assert_eq!(
            "irina"
                .parse::<PiperTtsVoice>()
                .expect("irina should parse into Irina"),
            PiperTtsVoice::Irina
        );
        assert!("unknown".parse::<PiperTtsVoice>().is_err());
    }

    #[test]
    fn format_parsing() {
        assert_eq!(
            "ogg"
                .parse::<PiperTtsFormat>()
                .expect("ogg should parse into Ogg"),
            PiperTtsFormat::Ogg
        );
        assert_eq!(
            "mp3"
                .parse::<PiperTtsFormat>()
                .expect("mp3 should parse into Mp3"),
            PiperTtsFormat::Mp3
        );
        assert!("pcm".parse::<PiperTtsFormat>().is_err());
    }

    #[test]
    fn request_defaults_use_natural_preset() {
        let req = PiperTtsRequest::new("Привет");

        assert_eq!(req.voice, "ruslan");
        assert_eq!(req.format, "ogg");
        assert!((req.speed - DEFAULT_SPEED).abs() < f32::EPSILON);
        assert!((req.noise_scale - DEFAULT_NOISE_SCALE).abs() < f32::EPSILON);
        assert!((req.noise_w_scale - DEFAULT_NOISE_W_SCALE).abs() < f32::EPSILON);
        assert!((req.sentence_silence - DEFAULT_SENTENCE_SILENCE).abs() < f32::EPSILON);
        assert!(req.normalize_audio);
    }

    #[test]
    fn args_to_request_applies_defaults() {
        let config = PiperTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            voice: None,
            format: None,
            length_scale: None,
            speed: None,
            noise_scale: None,
            noise_w_scale: None,
            volume: None,
            normalize_audio: None,
            sentence_silence: None,
        };

        let req = args
            .to_request(&config)
            .expect("args should convert into a valid Piper request");
        assert_eq!(req.voice, "ruslan");
        assert_eq!(req.format, "ogg");
        assert!((req.speed - DEFAULT_SPEED).abs() < f32::EPSILON);
    }

    #[test]
    fn args_to_request_applies_overrides() {
        let config = PiperTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            voice: Some("irina".to_string()),
            format: Some("wav".to_string()),
            length_scale: Some(1.1),
            speed: Some(1.2),
            noise_scale: Some(0.4),
            noise_w_scale: Some(0.5),
            volume: Some(1.3),
            normalize_audio: Some(false),
            sentence_silence: Some(0.5),
        };

        let req = args
            .to_request(&config)
            .expect("args should convert into a valid Piper request");
        assert_eq!(req.voice, "irina");
        assert_eq!(req.format, "wav");
        assert_eq!(req.length_scale, Some(1.1));
        assert!((req.speed - 1.2).abs() < f32::EPSILON);
        assert!((req.noise_scale - 0.4).abs() < f32::EPSILON);
        assert!((req.noise_w_scale - 0.5).abs() < f32::EPSILON);
        assert!((req.volume - 1.3).abs() < f32::EPSILON);
        assert!(!req.normalize_audio);
        assert!((req.sentence_silence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn args_to_request_rejects_invalid_values() {
        let config = PiperTtsConfig::default();
        let args = TextToSpeechRuArgs {
            text: "Привет".to_string(),
            voice: None,
            format: None,
            length_scale: None,
            speed: Some(0.0),
            noise_scale: None,
            noise_w_scale: None,
            volume: None,
            normalize_audio: None,
            sentence_silence: None,
        };

        assert!(args.to_request(&config).is_err());
    }
}
