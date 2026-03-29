//! HTTP utilities for LLM providers
//!
//! Provides common HTTP request/response handling to eliminate
//! code duplication across provider implementations.

use crate::config::get_llm_http_timeout_secs;
use crate::llm::LlmError;
use reqwest::Client as HttpClient;
use serde_json::Value;
use std::time::Duration;

/// Application name and version for User-Agent and provider attribution headers.
pub const APP_USER_AGENT: &str = "Oxide-Agent/0.1.0";

/// Creates an HTTP client configured with the standard LLM timeout.
///
/// Uses `LLM_HTTP_TIMEOUT_SECS` environment variable or default configuration.
/// This keeps long-running responses alive while preventing infinite hangs.
/// Sets User-Agent header to identify the application to LLM providers.
#[must_use]
pub fn create_http_client() -> HttpClient {
    let timeout = Duration::from_secs(get_llm_http_timeout_secs());
    HttpClient::builder()
        .pool_max_idle_per_host(10)
        .timeout(timeout)
        .user_agent(APP_USER_AGENT)
        .build()
        .unwrap_or_else(|_| HttpClient::new())
}

/// Sends an HTTP POST request with JSON body and returns parsed JSON response.
///
/// This function handles:
/// - Sending the request with optional authorization and custom headers
/// - Checking the response status
/// - Parsing the JSON response
///
/// # Arguments
/// * `client` - HTTP client to use
/// * `url` - Target URL
/// * `body` - JSON body to send
/// * `auth_header` - Optional authorization header value (e.g., "Bearer token")
/// * `extra_headers` - Additional headers as key-value pairs
///
/// # Returns
/// Parsed JSON response or `LlmError`
///
/// # Errors
///
/// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
/// or `LlmError::JsonError` if parsing fails.
pub async fn send_json_request(
    client: &HttpClient,
    url: &str,
    body: &Value,
    auth_header: Option<&str>,
    extra_headers: &[(&str, &str)],
) -> Result<Value, LlmError> {
    let mut request = client.post(url).json(body);

    // Always include User-Agent for provider identification
    request = request.header("User-Agent", APP_USER_AGENT);

    if let Some(auth) = auth_header {
        request = request.header("Authorization", auth);
    }

    for (key, value) in extra_headers {
        request = request.header(*key, *value);
    }

    let response = request
        .send()
        .await
        .map_err(|e| LlmError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait_secs = parse_retry_after(response.headers());
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::RateLimit {
                wait_secs,
                message: error_text,
            });
        }

        let error_text = response.text().await.unwrap_or_default();

        let is_html = error_text.trim_start().starts_with("<!DOCTYPE")
            || error_text.trim_start().starts_with("<html")
            || error_text.trim_start().starts_with("<HTML");

        let clean_message = if is_html {
            format!("API error: {status} (Server returned HTML error page)")
        } else {
            let truncated = if error_text.len() > 500 {
                format!("{}... (truncated)", &error_text[..500])
            } else {
                error_text
            };
            format!("API error: {status} - {truncated}")
        };

        return Err(LlmError::ApiError(clean_message));
    }

    response
        .json()
        .await
        .map_err(|e| LlmError::JsonError(e.to_string()))
}

/// Extracts text content from a JSON response by navigating a path.
///
/// # Arguments
/// * `response` - JSON response to extract from
/// * `path` - Path segments to navigate (supports string keys and numeric indices)
///
/// # Errors
///
/// Returns `LlmError::ApiError` if the path is invalid or the target is not a string.
pub fn extract_text_content(response: &Value, path: &[&str]) -> Result<String, LlmError> {
    let mut current = response;

    for segment in path {
        if let Ok(index) = segment.parse::<usize>() {
            current = current.get(index).ok_or_else(|| {
                LlmError::ApiError(format!("Invalid path: missing index {index}"))
            })?;
        } else {
            current = current.get(*segment).ok_or_else(|| {
                LlmError::ApiError(format!("Invalid path: missing key {segment}"))
            })?;
        }
    }

    current
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| LlmError::ApiError(format!("Expected string at path, got: {current:?}")))
}

/// Helper to parse Retry-After header
/// Returns number of seconds to wait if present and valid
///
/// Supports two formats:
/// - Delta seconds (e.g., "120")
/// - HTTP-date (RFC 7231, e.g., "Wed, 21 Oct 2015 07:28:00 GMT")
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(header_val) = headers.get(reqwest::header::RETRY_AFTER) {
        if let Ok(val_str) = header_val.to_str() {
            if let Ok(secs) = val_str.parse::<u64>() {
                return Some(secs);
            }
            if let Some(wait_secs) = parse_http_date(val_str) {
                return Some(wait_secs);
            }
        }
    }
    None
}

/// Parse HTTP-date (IMF-fixdate, RFC 7231) and return seconds until that time.
fn parse_http_date(date_str: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc2822(date_str)
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(date_str))
        .ok()
        .and_then(|dt| {
            let now = chrono::Utc::now();
            let duration = dt.signed_duration_since(now);
            if duration.num_seconds() > 0 {
                Some(duration.num_seconds() as u64)
            } else {
                None
            }
        })
}
