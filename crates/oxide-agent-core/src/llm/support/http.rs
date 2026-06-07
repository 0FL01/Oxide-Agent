//! HTTP utilities for LLM providers
//!
//! Provides common HTTP request/response handling to eliminate
//! code duplication across provider implementations.

use crate::config::get_llm_http_timeout_secs;
use crate::llm::LlmError;
use reqwest::{Client as HttpClient, ClientBuilder as HttpClientBuilder};
use serde_json::Value;
use std::error::Error;
use std::time::{Duration, Instant};
use tracing::{debug, trace};

/// Application name and version for User-Agent and provider attribution headers.
pub const APP_USER_AGENT: &str = "Oxide-Agent/0.1.0";

/// Creates an HTTP client configured with the standard LLM timeout.
///
/// Uses `LLM_HTTP_TIMEOUT_SECS` environment variable or default configuration.
/// This keeps long-running responses alive while preventing infinite hangs.
/// Sets User-Agent header to identify the application to LLM providers.
pub fn create_http_client_builder() -> HttpClientBuilder {
    let timeout = Duration::from_secs(get_llm_http_timeout_secs());
    HttpClient::builder()
        .pool_max_idle_per_host(10)
        .timeout(timeout)
        .user_agent(APP_USER_AGENT)
}

/// Creates an HTTP client configured with the standard LLM timeout.
#[must_use]
pub fn create_http_client() -> HttpClient {
    create_http_client_builder()
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
    let started_at = Instant::now();
    let (url_host, url_path) = sanitized_url_parts(url);
    let body_bytes = json_body_len(body);

    debug!(
        method = "POST",
        url_host = url_host.as_str(),
        url_path = url_path.as_str(),
        body_bytes,
        has_auth = auth_header.is_some(),
        extra_headers_count = extra_headers.len(),
        "Sending LLM JSON request"
    );

    // Always include User-Agent for provider identification
    request = request.header("User-Agent", APP_USER_AGENT);

    if let Some(auth) = auth_header {
        request = request.header("Authorization", auth);
    }

    for (key, value) in extra_headers {
        request = request.header(*key, *value);
    }

    let response = request.send().await.map_err(|e| {
        let diagnostic = format_reqwest_error(&e);
        debug!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            elapsed_ms = started_at.elapsed().as_millis(),
            error = %diagnostic,
            is_timeout = e.is_timeout(),
            is_connect = e.is_connect(),
            is_request = e.is_request(),
            is_body = e.is_body(),
            "LLM JSON request failed before response"
        );
        LlmError::NetworkError(diagnostic)
    })?;

    let status = response.status();
    debug!(
        method = "POST",
        url_host = url_host.as_str(),
        url_path = url_path.as_str(),
        elapsed_ms = started_at.elapsed().as_millis(),
        status = status.as_u16(),
        "LLM JSON request received response"
    );

    if !response.status().is_success() {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait_secs = parse_retry_after(response.headers());
            let error_text = response.text().await.unwrap_or_default();
            debug!(
                method = "POST",
                url_host = url_host.as_str(),
                url_path = url_path.as_str(),
                elapsed_ms = started_at.elapsed().as_millis(),
                status = status.as_u16(),
                retry_after_secs = wait_secs,
                body_chars = error_text.chars().count(),
                "LLM JSON request hit provider rate limit"
            );
            trace!(
                method = "POST",
                url_host = url_host.as_str(),
                url_path = url_path.as_str(),
                body_preview = truncate_for_log(&error_text, 500).as_str(),
                "LLM rate limit response body"
            );
            return Err(LlmError::RateLimit {
                wait_secs,
                message: error_text,
            });
        }

        let error_text = response.text().await.unwrap_or_default();
        debug!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            elapsed_ms = started_at.elapsed().as_millis(),
            status = status.as_u16(),
            body_chars = error_text.chars().count(),
            "LLM JSON request returned non-success status"
        );
        trace!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            status = status.as_u16(),
            body_preview = truncate_for_log(&error_text, 500).as_str(),
            "LLM error response body"
        );

        let is_html = error_text.trim_start().starts_with("<!DOCTYPE")
            || error_text.trim_start().starts_with("<html")
            || error_text.trim_start().starts_with("<HTML");

        let clean_message = if is_html {
            format!("API error: {status} (Server returned HTML error page)")
        } else {
            let truncated = truncate_for_log(&error_text, 500);
            format!("API error: {status} - {truncated}")
        };

        return Err(LlmError::ApiError(clean_message));
    }

    let response_text = response.text().await.map_err(|e| {
        let diagnostic = format_reqwest_error(&e);
        debug!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            elapsed_ms = started_at.elapsed().as_millis(),
            status = status.as_u16(),
            error = %diagnostic,
            is_timeout = e.is_timeout(),
            is_connect = e.is_connect(),
            is_request = e.is_request(),
            is_body = e.is_body(),
            "LLM JSON response body read failed"
        );
        LlmError::NetworkError(diagnostic)
    })?;

    trace!(
        method = "POST",
        url_host = url_host.as_str(),
        url_path = url_path.as_str(),
        elapsed_ms = started_at.elapsed().as_millis(),
        status = status.as_u16(),
        body_chars = response_text.chars().count(),
        body_preview = truncate_for_log(&response_text, 500).as_str(),
        "LLM success response body"
    );

    serde_json::from_str(&response_text).map_err(|e| {
        debug!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            elapsed_ms = started_at.elapsed().as_millis(),
            status = status.as_u16(),
            body_chars = response_text.chars().count(),
            error = %e,
            "LLM JSON response parse failed"
        );
        trace!(
            method = "POST",
            url_host = url_host.as_str(),
            url_path = url_path.as_str(),
            body_preview = truncate_for_log(&response_text, 500).as_str(),
            "Invalid LLM JSON response body"
        );
        LlmError::JsonError(e.to_string())
    })
}

fn json_body_len(body: &Value) -> usize {
    serde_json::to_vec(body).map_or(0, |bytes| bytes.len())
}

fn sanitized_url_parts(url: &str) -> (String, String) {
    match reqwest::Url::parse(url) {
        Ok(parsed) => (
            parsed.host_str().unwrap_or("unknown").to_string(),
            parsed.path().to_string(),
        ),
        Err(_) => ("invalid-url".to_string(), "invalid-url".to_string()),
    }
}

fn truncate_for_log(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}... (truncated)")
    } else {
        truncated
    }
}

fn format_reqwest_error(error: &reqwest::Error) -> String {
    let mut source_chain = Vec::new();
    let mut current = error.source();
    while let Some(source) = current {
        source_chain.push(source.to_string());
        current = source.source();
    }

    let source_chain = if source_chain.is_empty() {
        "none".to_string()
    } else {
        source_chain.join(" | ")
    };

    format!(
        "request failed: timeout={} connect={} request={} body={} decode={} status={:?} source_chain={}",
        error.is_timeout(),
        error.is_connect(),
        error.is_request(),
        error.is_body(),
        error.is_decode(),
        error.status(),
        source_chain
    )
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
    if let Some(header_val) = headers.get(reqwest::header::RETRY_AFTER)
        && let Ok(val_str) = header_val.to_str() {
            if let Ok(secs) = val_str.parse::<u64>() {
                return Some(secs);
            }
            if let Some(wait_secs) = parse_http_date(val_str) {
                return Some(wait_secs);
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
