//! Audio transcription for Mistral provider
//!
//! Uses Mistral's dedicated audio transcription API endpoint:
//! POST https://api.mistral.ai/v1/audio/transcriptions
//!
//! Supports Voxtral models:
//! - voxtral-mini-latest
//! - voxtral-mini-transcribe-26-02 (batch)
//! - voxtral-mini-realtime-26-02 (streaming)

use crate::config::MISTRAL_AUDIO_TRANSCRIBE_TEMPERATURE;
use crate::llm::http_utils::parse_retry_after;
use crate::llm::LlmError;
use reqwest::{
    multipart::{Form, Part},
    Client as HttpClient, StatusCode,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

const MAX_RETRIES: usize = 5;
const INITIAL_BACKOFF_MS: u64 = 3000; // 3s initial, then 6s, 12s, 24s, 48s

/// Transcribe audio using Mistral Voxtral models with retry logic
///
/// # Arguments
/// * `http_client` - HTTP client for requests
/// * `api_key` - Mistral API key
/// * `audio_bytes` - Raw audio file bytes
/// * `mime_type` - MIME type (audio/wav, audio/mpeg supported)
/// * `model_id` - Model ID (e.g., "voxtral-mini-latest")
///
/// # Returns
/// Transcription text or LlmError
pub async fn transcribe_audio(
    http_client: &HttpClient,
    api_key: &str,
    audio_bytes: Vec<u8>,
    mime_type: &str,
    model_id: &str,
) -> Result<String, LlmError> {
    retry_transcription(
        || async {
            transcribe_audio_once(
                http_client,
                api_key,
                audio_bytes.clone(),
                mime_type,
                model_id,
            )
            .await
        },
        model_id,
    )
    .await
}

/// Retry wrapper for transcription with exponential backoff
async fn retry_transcription<F, Fut>(operation: F, model_id: &str) -> Result<String, LlmError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<String, LlmError>>,
{
    let context = format!("Mistral transcription with {}", model_id);

    for attempt in 1..=MAX_RETRIES {
        match operation().await {
            Ok(result) => {
                if attempt > 1 {
                    info!("{} succeeded after {} attempts", context, attempt);
                }
                return Ok(result);
            }
            Err(e) => {
                if attempt < MAX_RETRIES {
                    if let Some(backoff) = get_retry_delay(&e, attempt) {
                        warn!(
                            "{} failed (attempt {}/{}): {}, retrying after {:?}",
                            context, attempt, MAX_RETRIES, e, backoff
                        );
                        sleep(backoff).await;
                        continue;
                    }
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

/// Calculate retry delay based on error type and attempt number
fn get_retry_delay(error: &LlmError, attempt: usize) -> Option<Duration> {
    match error {
        // Rate limit: use server-provided wait time or exponential backoff
        LlmError::RateLimit { wait_secs, .. } => {
            let delay = if let Some(secs) = wait_secs {
                Duration::from_secs(*secs + 1)
            } else {
                Duration::from_millis(INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32))
            };
            Some(delay)
        }

        // API errors: check for retryable status codes
        LlmError::ApiError(msg) => {
            let msg_lower = msg.to_lowercase();

            // 429 Too Many Requests
            if msg_lower.contains("429") {
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }

            // 502 Bad Gateway, 503 Service Unavailable, 504 Gateway Timeout
            if msg_lower.contains("502")
                || msg_lower.contains("503")
                || msg_lower.contains("504")
                || msg_lower.contains("gateway")
                || msg_lower.contains("unavailable")
            {
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }

            // Timeout errors
            if msg_lower.contains("timeout") || msg_lower.contains("timed out") {
                let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
                return Some(Duration::from_millis(backoff_ms));
            }

            None
        }

        // Network errors (except configuration errors)
        LlmError::NetworkError(msg) => {
            if msg.contains("builder") || msg.contains("configuration") {
                return None;
            }
            let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }

        // JSON parsing errors might be transient
        LlmError::JsonError(_) => {
            let backoff_ms = INITIAL_BACKOFF_MS * 2u64.pow((attempt - 1) as u32);
            Some(Duration::from_millis(backoff_ms))
        }

        // Other errors are not retryable
        _ => None,
    }
}

/// Single transcription attempt (no retry)
async fn transcribe_audio_once(
    http_client: &HttpClient,
    api_key: &str,
    audio_bytes: Vec<u8>,
    mime_type: &str,
    model_id: &str,
) -> Result<String, LlmError> {
    let url = "https://api.mistral.ai/v1/audio/transcriptions";

    // Determine file extension from MIME type
    let extension = mime_to_extension(mime_type);

    // Create multipart form with file
    let part = Part::bytes(audio_bytes)
        .file_name(format!("audio.{}", extension))
        .mime_str(mime_type)
        .map_err(|e| LlmError::NetworkError(format!("Invalid MIME type: {}", e)))?;

    let form = Form::new()
        .part("file", part)
        .text("model", model_id.to_string())
        .text(
            "temperature",
            MISTRAL_AUDIO_TRANSCRIBE_TEMPERATURE.to_string(),
        );

    // Send request
    let response = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .timeout(Duration::from_secs(120)) // 2 min timeout for audio processing
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                LlmError::NetworkError(format!("Request timeout: {}", e))
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
            message: format!("Mistral rate limit: {}", error_text),
        });
    }

    // Handle other errors
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(LlmError::ApiError(format!(
            "Mistral transcription error {}: {}",
            status, error_text
        )));
    }

    // Parse response
    let json: Value = response
        .json()
        .await
        .map_err(|e| LlmError::JsonError(format!("Failed to parse response: {}", e)))?;

    // Extract transcription text
    json.get("text")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| {
            LlmError::ApiError("Missing 'text' field in transcription response".to_string())
        })
}

/// Map MIME type to file extension
fn mime_to_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/flac" => "flac",
        "audio/ogg" | "audio/vorbis" => "ogg",
        "audio/aac" => "aac",
        _ => "wav", // Default fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_to_extension() {
        assert_eq!(mime_to_extension("audio/wav"), "wav");
        assert_eq!(mime_to_extension("audio/x-wav"), "wav");
        assert_eq!(mime_to_extension("audio/mpeg"), "mp3");
        assert_eq!(mime_to_extension("audio/mp3"), "mp3");
        assert_eq!(mime_to_extension("audio/mp4"), "m4a");
        assert_eq!(mime_to_extension("audio/flac"), "flac");
        assert_eq!(mime_to_extension("audio/ogg"), "ogg");
        assert_eq!(mime_to_extension("unknown/type"), "wav"); // fallback
    }

    #[test]
    fn test_retry_delay_calculation() {
        // Test exponential backoff
        let err = LlmError::ApiError("502 Bad Gateway".to_string());
        assert_eq!(get_retry_delay(&err, 1), Some(Duration::from_millis(3000)));
        assert_eq!(get_retry_delay(&err, 2), Some(Duration::from_millis(6000)));
        assert_eq!(get_retry_delay(&err, 3), Some(Duration::from_millis(12000)));
    }

    #[test]
    fn test_retry_delay_rate_limit() {
        // Test rate limit with server-provided wait time
        let err = LlmError::RateLimit {
            wait_secs: Some(10),
            message: "Rate limited".to_string(),
        };
        assert_eq!(
            get_retry_delay(&err, 1),
            Some(Duration::from_secs(11)) // wait_secs + 1
        );
    }

    #[test]
    fn test_retry_delay_no_server_time() {
        // Test rate limit without server wait time
        let err = LlmError::RateLimit {
            wait_secs: None,
            message: "Rate limited".to_string(),
        };
        assert_eq!(get_retry_delay(&err, 1), Some(Duration::from_millis(3000)));
    }
}
