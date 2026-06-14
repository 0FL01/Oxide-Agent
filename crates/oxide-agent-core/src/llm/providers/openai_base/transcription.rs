//! Profile-driven audio transcription for OpenAI-compatible providers.
//!
//! Shared implementation used by providers that expose a
//! `POST /audio/transcriptions` (or similar) multipart endpoint.
//! Profile parameters come from [`AudioTranscriptionProfile`].

use crate::llm::LlmError;
use crate::llm::support::http::parse_retry_after;
use crate::llm::providers::openai_base::profile::AudioTranscriptionProfile;
use reqwest::{
    Client as HttpClient, StatusCode,
    multipart::{Form, Part},
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

/// Build the full transcription URL from `api_base` and the profile endpoint path.
///
/// Handles trailing/leading slashes gracefully:
/// `https://api.mistral.ai/v1` + `/audio/transcriptions`
///   -> `https://api.mistral.ai/v1/audio/transcriptions`
pub fn transcription_url(api_base: &str, endpoint_path: &str) -> String {
    let base = api_base.trim_end_matches('/');
    let path = endpoint_path.trim_start_matches('/');
    format!("{base}/{path}")
}

/// Map MIME type to file extension for the multipart form filename.
pub fn mime_to_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/flac" => "flac",
        "audio/ogg" | "audio/vorbis" => "ogg",
        "audio/aac" => "aac",
        _ => "wav",
    }
}

/// Transcribe audio using the provider's transcription endpoint with retry logic.
///
/// # Arguments
/// * `http_client` - Shared HTTP client.
/// * `api_key` - Optional Bearer token.
/// * `api_base` - Base URL (e.g. `https://api.mistral.ai/v1`).
/// * `audio_bytes` - Raw audio file bytes.
/// * `mime_type` - MIME type (e.g. `audio/wav`).
/// * `model_id` - Model ID (e.g. `voxtral-mini-latest`).
/// * `profile` - Audio transcription profile from the provider.
/// * `provider_name` - Human-readable name for log context.
pub async fn transcribe_audio(
    http_client: &HttpClient,
    api_key: Option<&str>,
    api_base: &str,
    audio_bytes: Vec<u8>,
    mime_type: &str,
    model_id: &str,
    profile: &AudioTranscriptionProfile,
    provider_name: &str,
) -> Result<String, LlmError> {
    let url = transcription_url(api_base, profile.endpoint_path);
    let auth = api_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| format!("Bearer {k}"));

    retry_transcription(
        || async {
            transcribe_audio_once(
                http_client,
                auth.as_deref(),
                &url,
                audio_bytes.clone(),
                mime_type,
                model_id,
                profile,
            )
            .await
        },
        model_id,
        provider_name,
        profile,
    )
    .await
}

/// Retry wrapper with exponential backoff.
async fn retry_transcription<F, Fut>(
    operation: F,
    model_id: &str,
    provider_name: &str,
    profile: &AudioTranscriptionProfile,
) -> Result<String, LlmError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<String, LlmError>>,
{
    let context = format!("{} transcription with {}", provider_name, model_id);
    let max_retries = profile.max_retries;

    for attempt in 1..=max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    info!("{} succeeded after {} attempts", context, attempt);
                }
                return Ok(result);
            }
            Err(e) => {
                if attempt < max_retries
                    && let Some(backoff) = get_retry_delay(&e, attempt, profile)
                {
                    warn!(
                        "{} failed (attempt {}/{}): {}, retrying after {:?}",
                        context, attempt, max_retries, e, backoff
                    );
                    sleep(backoff).await;
                    continue;
                }
                warn!("{} failed after {} attempts: {}", context, attempt, e);
                return Err(e);
            }
        }
    }

    Err(LlmError::ApiError(
        "All transcription retry attempts exhausted".to_string(),
    ))
}

/// Calculate retry delay based on error type and attempt number.
fn get_retry_delay(error: &LlmError, attempt: usize, profile: &AudioTranscriptionProfile) -> Option<Duration> {
    match error {
        LlmError::RateLimit { wait_secs, .. } => {
            let delay = if let Some(secs) = wait_secs {
                Duration::from_secs(*secs + 1)
            } else {
                Duration::from_millis(profile.initial_backoff_ms * 2u64.pow((attempt - 1) as u32))
            };
            Some(delay)
        }

        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();

            if msg_lower.contains("429")
                || msg_lower.contains("502")
                || msg_lower.contains("503")
                || msg_lower.contains("504")
                || msg_lower.contains("gateway")
                || msg_lower.contains("unavailable")
                || msg_lower.contains("timeout")
                || msg_lower.contains("timed out")
            {
                let backoff_ms = profile.initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }

            None
        }

        LlmError::NetworkError(msg) => {
            if msg.contains("builder") || msg.contains("configuration") {
                return None;
            }
            let backoff_ms = profile.initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }

        LlmError::JsonError(_) => {
            let backoff_ms = profile.initial_backoff_ms * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }

        _ => None,
    }
}

/// Single transcription attempt (no retry).
async fn transcribe_audio_once(
    http_client: &HttpClient,
    auth: Option<&str>,
    url: &str,
    audio_bytes: Vec<u8>,
    mime_type: &str,
    model_id: &str,
    profile: &AudioTranscriptionProfile,
) -> Result<String, LlmError> {
    let extension = mime_to_extension(mime_type);

    let part = Part::bytes(audio_bytes)
        .file_name(format!("audio.{extension}"))
        .mime_str(mime_type)
        .map_err(|e| LlmError::NetworkError(format!("Invalid MIME type: {e}")))?;

    let form = Form::new()
        .part("file", part)
        .text("model", model_id.to_string())
        .text("temperature", profile.temperature.to_string());

    let mut request = http_client.post(url).multipart(form);

    if let Some(auth) = auth {
        request = request.header("Authorization", auth);
    }

    let response = request
        .timeout(Duration::from_secs(profile.timeout_secs))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::NetworkError(format!("Request timeout: {e}"))
            } else {
                LlmError::NetworkError(e.to_string())
            }
        })?;

    // Handle rate limiting
    let status = response.status();
    if status == StatusCode::TOO_MANY_REQUESTS {
        let wait_secs = parse_retry_after(response.headers());
        let error_text = response.text().await.unwrap_or_default();
        return Err(LlmError::RateLimit {
            wait_secs,
            message: format!("Rate limit: {error_text}"),
        });
    }

    // Handle other errors
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(LlmError::ApiError(format!(
            "Transcription error {status}: {error_text}"
        )));
    }

    // Parse response
    let json: Value = response
        .json()
        .await
        .map_err(|e| LlmError::JsonError(format!("Failed to parse response: {e}")))?;

    json.get("text")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            LlmError::ApiError("Missing 'text' field in transcription response".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_to_extension_variants() {
        assert_eq!(mime_to_extension("audio/wav"), "wav");
        assert_eq!(mime_to_extension("audio/x-wav"), "wav");
        assert_eq!(mime_to_extension("audio/wave"), "wav");
        assert_eq!(mime_to_extension("audio/mpeg"), "mp3");
        assert_eq!(mime_to_extension("audio/mp3"), "mp3");
        assert_eq!(mime_to_extension("audio/mp4"), "m4a");
        assert_eq!(mime_to_extension("audio/x-m4a"), "m4a");
        assert_eq!(mime_to_extension("audio/flac"), "flac");
        assert_eq!(mime_to_extension("audio/ogg"), "ogg");
        assert_eq!(mime_to_extension("audio/vorbis"), "ogg");
        assert_eq!(mime_to_extension("audio/aac"), "aac");
        assert_eq!(mime_to_extension("unknown/type"), "wav");
    }

    #[test]
    fn transcription_url_combines_base_and_path() {
        assert_eq!(
            transcription_url("https://api.mistral.ai/v1", "/audio/transcriptions"),
            "https://api.mistral.ai/v1/audio/transcriptions"
        );
        assert_eq!(
            transcription_url("https://api.mistral.ai/v1/", "/audio/transcriptions"),
            "https://api.mistral.ai/v1/audio/transcriptions"
        );
        assert_eq!(
            transcription_url("http://localhost:8080/v1", "/audio/transcriptions"),
            "http://localhost:8080/v1/audio/transcriptions"
        );
    }

    #[test]
    fn retry_delay_502_exponential_backoff() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::ApiError("502 Bad Gateway".to_string());
        assert_eq!(get_retry_delay(&err, 1, &profile), Some(Duration::from_millis(3000)));
        assert_eq!(get_retry_delay(&err, 2, &profile), Some(Duration::from_millis(6000)));
        assert_eq!(get_retry_delay(&err, 3, &profile), Some(Duration::from_millis(12000)));
    }

    #[test]
    fn retry_delay_rate_limit_server_wait() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::RateLimit {
            wait_secs: Some(10),
            message: "Rate limited".to_string(),
        };
        assert_eq!(
            get_retry_delay(&err, 1, &profile),
            Some(Duration::from_secs(11))
        );
    }

    #[test]
    fn retry_delay_rate_limit_no_server_wait() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::RateLimit {
            wait_secs: None,
            message: "Rate limited".to_string(),
        };
        assert_eq!(
            get_retry_delay(&err, 1, &profile),
            Some(Duration::from_millis(3000))
        );
    }

    #[test]
    fn retry_delay_429_in_api_error() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::ApiError("HTTP 429 Too Many Requests".to_string());
        assert_eq!(get_retry_delay(&err, 1, &profile), Some(Duration::from_millis(3000)));
        assert_eq!(get_retry_delay(&err, 2, &profile), Some(Duration::from_millis(6000)));
    }

    #[test]
    fn retry_delay_503_unavailable() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::ApiError("503 Service Unavailable".to_string());
        assert!(get_retry_delay(&err, 1, &profile).is_some());
    }

    #[test]
    fn retry_delay_non_retryable_error() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::ApiError("400 Bad Request".to_string());
        assert!(get_retry_delay(&err, 1, &profile).is_none());
    }

    #[test]
    fn retry_delay_network_config_error_not_retried() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::NetworkError("builder configuration error".to_string());
        assert!(get_retry_delay(&err, 1, &profile).is_none());
    }

    #[test]
    fn retry_delay_network_error_is_retried() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::NetworkError("connection refused".to_string());
        assert!(get_retry_delay(&err, 1, &profile).is_some());
    }

    #[test]
    fn retry_delay_json_error_is_retried() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 3000,
        };
        let err = LlmError::JsonError("unexpected token".to_string());
        assert!(get_retry_delay(&err, 1, &profile).is_some());
    }

    #[test]
    fn retry_delay_custom_backoff() {
        let profile = AudioTranscriptionProfile {
            endpoint_path: "/audio/transcriptions",
            temperature: 0.4,
            timeout_secs: 120,
            max_retries: 5,
            initial_backoff_ms: 1000,
        };
        let err = LlmError::ApiError("502 Bad Gateway".to_string());
        assert_eq!(get_retry_delay(&err, 1, &profile), Some(Duration::from_millis(1000)));
        assert_eq!(get_retry_delay(&err, 2, &profile), Some(Duration::from_millis(2000)));
        assert_eq!(get_retry_delay(&err, 3, &profile), Some(Duration::from_millis(4000)));
    }
}
