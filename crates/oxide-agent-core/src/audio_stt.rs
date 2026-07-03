//! Dedicated speech-to-text boundary for user audio.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Runtime configuration for the dedicated audio STT backend.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AudioSttConfig {
    /// Base URL of the STT backend, for example `http://127.0.0.1:8002`.
    pub base_url: Option<String>,
    /// Optional bearer token used by the STT backend.
    pub api_key: Option<String>,
    /// Optional VAD override. `None` leaves the backend default intact.
    pub vad: Option<bool>,
}

impl AudioSttConfig {
    /// Returns true when a non-empty STT backend URL is configured.
    #[must_use]
    pub fn has_backend(&self) -> bool {
        self.base_url
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    }
}

/// Input payload for audio transcription.
#[derive(Debug, Clone)]
pub struct AudioTranscriptionInput {
    /// Raw audio bytes.
    pub bytes: Vec<u8>,
    /// MIME type reported by transport or inferred from the file.
    pub mime_type: String,
    /// Optional file name to expose in multipart uploads.
    pub file_name: Option<String>,
    /// Optional per-request VAD override. `None` leaves the transcriber default.
    pub vad: Option<bool>,
}

impl AudioTranscriptionInput {
    /// Creates a transcription input from raw bytes and MIME type.
    #[must_use]
    pub fn new(bytes: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self {
            bytes,
            mime_type: mime_type.into(),
            file_name: None,
            vad: None,
        }
    }

    /// Adds a file name to the transcription input.
    #[must_use]
    pub fn with_file_name(mut self, file_name: impl Into<String>) -> Self {
        self.file_name = Some(file_name.into());
        self
    }

    /// Adds a per-request VAD override to the transcription input.
    #[must_use]
    pub const fn with_vad(mut self, vad: Option<bool>) -> Self {
        self.vad = vad;
        self
    }
}

/// One timestamped segment returned by the STT backend.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AudioTranscriptSegment {
    /// Segment start offset in seconds.
    pub start: f64,
    /// Segment end offset in seconds.
    pub end: f64,
    /// Segment text.
    pub text: String,
}

/// Structured transcript returned by the STT backend.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AudioTranscript {
    /// Full transcript text.
    pub text: String,
    /// Optional timestamped segments.
    #[serde(default)]
    pub segments: Vec<AudioTranscriptSegment>,
    /// Backend model identifier.
    pub model: String,
    /// Whether VAD was applied by the backend.
    pub vad: bool,
}

/// Errors raised by the dedicated audio STT boundary.
#[derive(Debug, Error)]
pub enum AudioSttError {
    /// STT backend is not configured or not compiled in.
    #[error("{0}")]
    MissingConfig(String),
    /// STT base URL is syntactically invalid.
    #[error("invalid AUDIO_STT_BASE_URL: {0}")]
    InvalidBaseUrl(String),
    /// HTTP transport error while calling the STT backend.
    #[cfg(feature = "http-client")]
    #[error("audio STT HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// STT backend returned a non-success status code.
    #[error("audio STT backend returned HTTP {status}: {body}")]
    ApiStatus {
        /// Numeric HTTP status code.
        status: u16,
        /// Response body returned by the backend.
        body: String,
    },
    /// STT backend returned a success status with an invalid response body.
    #[error("audio STT backend returned invalid response: {0}")]
    InvalidResponse(String),
}

/// Transport-independent speech-to-text interface.
#[async_trait]
pub trait AudioTranscriber: Send + Sync {
    /// Transcribes one audio payload.
    async fn transcribe(
        &self,
        input: AudioTranscriptionInput,
    ) -> Result<AudioTranscript, AudioSttError>;
}

/// Transcriber used when STT is unavailable in the current runtime.
#[derive(Debug, Clone)]
pub struct UnavailableAudioTranscriber {
    reason: String,
}

impl UnavailableAudioTranscriber {
    /// Creates an unavailable transcriber with an explicit operator-facing reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl AudioTranscriber for UnavailableAudioTranscriber {
    async fn transcribe(
        &self,
        _input: AudioTranscriptionInput,
    ) -> Result<AudioTranscript, AudioSttError> {
        Err(AudioSttError::MissingConfig(self.reason.clone()))
    }
}

/// HTTP client for the GigaAM-compatible `/v1/audio/transcriptions` API.
#[cfg(feature = "http-client")]
#[derive(Debug, Clone)]
pub struct GigaAmSttClient {
    http_client: reqwest::Client,
    base_url: Url,
    api_key: Option<String>,
    default_vad: Option<bool>,
}

#[cfg(feature = "http-client")]
impl GigaAmSttClient {
    /// Creates a GigaAM STT client using the shared project HTTP client policy.
    ///
    /// # Errors
    ///
    /// Returns [`AudioSttError::InvalidBaseUrl`] when `base_url` is not a URL.
    pub fn new(
        base_url: impl AsRef<str>,
        api_key: Option<String>,
        default_vad: Option<bool>,
    ) -> Result<Self, AudioSttError> {
        Self::new_with_client(
            base_url,
            api_key,
            default_vad,
            crate::llm::http::create_http_client(),
        )
    }

    /// Creates a GigaAM STT client with an injected HTTP client.
    ///
    /// # Errors
    ///
    /// Returns [`AudioSttError::InvalidBaseUrl`] when `base_url` is not a URL.
    pub fn new_with_client(
        base_url: impl AsRef<str>,
        api_key: Option<String>,
        default_vad: Option<bool>,
        http_client: reqwest::Client,
    ) -> Result<Self, AudioSttError> {
        let base_url = normalize_base_url(base_url.as_ref())?;
        Ok(Self {
            http_client,
            base_url,
            api_key,
            default_vad,
        })
    }

    fn transcriptions_url(&self) -> Result<Url, AudioSttError> {
        self.base_url
            .join("v1/audio/transcriptions")
            .map_err(|error| AudioSttError::InvalidBaseUrl(error.to_string()))
    }
}

#[cfg(feature = "http-client")]
#[async_trait]
impl AudioTranscriber for GigaAmSttClient {
    async fn transcribe(
        &self,
        input: AudioTranscriptionInput,
    ) -> Result<AudioTranscript, AudioSttError> {
        let file_name = input
            .file_name
            .clone()
            .unwrap_or_else(|| default_audio_file_name(&input.mime_type));
        let part = reqwest::multipart::Part::bytes(input.bytes)
            .file_name(file_name)
            .mime_str(&input.mime_type)?;
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(vad) = input.vad.or(self.default_vad) {
            form = form.text("vad", vad.to_string());
        }

        let mut request = self
            .http_client
            .post(self.transcriptions_url()?)
            .multipart(form);
        if let Some(api_key) = self.api_key.as_deref().filter(|value| !value.is_empty()) {
            request = request.bearer_auth(api_key);
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(AudioSttError::ApiStatus {
                status: status.as_u16(),
                body,
            });
        }

        serde_json::from_str(&body)
            .map_err(|error| AudioSttError::InvalidResponse(error.to_string()))
    }
}

/// Builds the configured audio transcriber for the current runtime.
#[must_use]
pub fn build_audio_transcriber(config: &AudioSttConfig) -> Arc<dyn AudioTranscriber> {
    let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Arc::new(UnavailableAudioTranscriber::new(
            "AUDIO_STT_BASE_URL is not configured",
        ));
    };

    build_configured_audio_transcriber(base_url, config)
}

#[cfg(all(feature = "http-client", oxide_module_tool_audio_stt))]
fn build_configured_audio_transcriber(
    base_url: &str,
    config: &AudioSttConfig,
) -> Arc<dyn AudioTranscriber> {
    match GigaAmSttClient::new(base_url, config.api_key.clone(), config.vad) {
        Ok(client) => Arc::new(client),
        Err(error) => Arc::new(UnavailableAudioTranscriber::new(error.to_string())),
    }
}

#[cfg(not(all(feature = "http-client", oxide_module_tool_audio_stt)))]
fn build_configured_audio_transcriber(
    _base_url: &str,
    _config: &AudioSttConfig,
) -> Arc<dyn AudioTranscriber> {
    Arc::new(UnavailableAudioTranscriber::new(
        "audio STT module is not compiled in this profile",
    ))
}

/// Validates `AUDIO_STT_BASE_URL` syntax without calling the external backend.
///
/// # Errors
///
/// Returns [`AudioSttError::InvalidBaseUrl`] when the configured URL is invalid.
pub fn validate_audio_stt_config(config: &AudioSttConfig) -> Result<(), AudioSttError> {
    if let Some(base_url) = config
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        normalize_base_url(base_url)?;
    }
    Ok(())
}

fn normalize_base_url(raw: &str) -> Result<Url, AudioSttError> {
    let mut base_url =
        Url::parse(raw).map_err(|error| AudioSttError::InvalidBaseUrl(error.to_string()))?;
    if !base_url.path().ends_with('/') {
        let path = format!("{}/", base_url.path());
        base_url.set_path(&path);
    }
    Ok(base_url)
}

#[cfg(feature = "http-client")]
fn default_audio_file_name(mime_type: &str) -> String {
    format!("audio.{}", audio_extension_from_mime_type(mime_type))
}

#[cfg(feature = "http-client")]
fn audio_extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or(mime_type).trim() {
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/flac" => "flac",
        "audio/webm" => "webm",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioSttConfig, validate_audio_stt_config};

    #[test]
    fn validate_audio_stt_config_rejects_invalid_base_url() {
        let config = AudioSttConfig {
            base_url: Some("not a url".to_string()),
            ..AudioSttConfig::default()
        };

        assert!(validate_audio_stt_config(&config).is_err());
    }

    #[test]
    fn validate_audio_stt_config_accepts_missing_backend() {
        assert!(validate_audio_stt_config(&AudioSttConfig::default()).is_ok());
    }
}
