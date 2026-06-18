use thiserror::Error;

/// Errors that can occur during LLM operations
#[derive(Debug, Clone, Error)]
pub enum LlmError {
    /// Error returned by the provider's API, optionally carrying the HTTP status code.
    #[error("API error: {message}")]
    ApiError {
        /// HTTP status code when the error originated from an HTTP response.
        /// `None` for non-HTTP errors (capability checks, response parsing, retry exhaustion).
        status: Option<u16>,
        /// Human-readable error message.
        message: String,
    },
    /// Provider returned a successful response envelope without usable content.
    #[error("API error: Empty response{0}")]
    EmptyResponse(String),
    /// Transient network error (connection refused, timeout, DNS failure, reset).
    /// Retryable.
    #[error("Network error: {0}")]
    NetworkError(String),
    /// Deterministic error while building an HTTP request (invalid URL, invalid header,
    /// invalid MIME type, etc.). NOT retryable — the same request will fail identically.
    #[error("Request builder error: {0}")]
    RequestBuilder(String),
    /// Error during JSON serialization or deserialization
    #[error("JSON error: {0}")]
    JsonError(String),
    /// Missing provider configuration or API key
    #[error("Missing client/API key: {0}")]
    MissingConfig(String),
    /// Rate limit exceeded (429), optionally with a wait time
    #[error("Rate limit exceeded: {message} (wait: {wait_secs:?}s)")]
    RateLimit {
        /// Retry-After duration in seconds, if provided by the server
        wait_secs: Option<u64>,
        /// Error message from the server
        message: String,
    },
    /// Request history is internally inconsistent but can be repaired locally.
    #[error("Repairable history error: {0}")]
    RepairableHistory(String),
    /// Any other unexpected error
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl LlmError {
    /// Construct an `ApiError` without an HTTP status code.
    ///
    /// Use [`Self::api_error_status`] when the error originates from an HTTP response
    /// so that retry/backoff logic can match on the typed status instead of string contents.
    #[must_use]
    pub fn api_error(message: impl Into<String>) -> Self {
        Self::ApiError {
            status: None,
            message: message.into(),
        }
    }

    /// Construct an `ApiError` with a known HTTP status code.
    #[must_use]
    pub fn api_error_status(status: u16, message: impl Into<String>) -> Self {
        Self::ApiError {
            status: Some(status),
            message: message.into(),
        }
    }

    /// Classify a `reqwest::Error` into `RequestBuilder` (deterministic) or `NetworkError`
    /// (transient) based on `is_builder()`. Use at every site that converts a reqwest error
    /// to `LlmError`, so retryability is determined from the error kind, not from string contents.
    #[must_use]
    pub fn from_reqwest_error(e: reqwest::Error) -> Self {
        if e.is_builder() {
            Self::RequestBuilder(e.to_string())
        } else {
            Self::NetworkError(e.to_string())
        }
    }
}
