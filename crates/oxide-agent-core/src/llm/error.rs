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
        /// Provider that produced the error (e.g. `"openrouter"`, `"mistral"`).
        /// Set by `LlmClient` when wrapping provider errors.
        provider: Option<String>,
        /// Model identifier that produced the error.
        /// Set by `LlmClient` when wrapping provider errors.
        model: Option<String>,
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
    #[error("Unknown error: {message}")]
    Unknown {
        /// Human-readable error message.
        message: String,
        /// Provider that produced the error, if known.
        provider: Option<String>,
        /// Model identifier that produced the error, if known.
        model: Option<String>,
    },
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
            provider: None,
            model: None,
        }
    }

    /// Construct an `ApiError` with a known HTTP status code.
    #[must_use]
    pub fn api_error_status(status: u16, message: impl Into<String>) -> Self {
        Self::ApiError {
            status: Some(status),
            message: message.into(),
            provider: None,
            model: None,
        }
    }

    /// Construct an `Unknown` error without provider/model context.
    #[must_use]
    pub fn unknown(message: impl Into<String>) -> Self {
        Self::Unknown {
            message: message.into(),
            provider: None,
            model: None,
        }
    }

    /// Attach the provider name to `ApiError` or `Unknown` variants.
    /// Other variants are returned unchanged.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        match &mut self {
            Self::ApiError { provider: p, .. } => *p = Some(provider.into()),
            Self::Unknown { provider: p, .. } => *p = Some(provider.into()),
            _ => {}
        }
        self
    }

    /// Attach the model identifier to `ApiError` or `Unknown` variants.
    /// Other variants are returned unchanged.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        match &mut self {
            Self::ApiError { model: m, .. } => *m = Some(model.into()),
            Self::Unknown { model: m, .. } => *m = Some(model.into()),
            _ => {}
        }
        self
    }

    /// Classify a `reqwest::Error` into `RequestBuilder` (deterministic) or `NetworkError`
    /// (transient) based on `is_builder()`. Use at every site that converts a reqwest error
    /// to `LlmError`, so retryability is determined from the error kind, not from string contents.
    #[cfg(feature = "http-client")]
    #[must_use]
    pub fn from_reqwest_error(e: reqwest::Error) -> Self {
        if e.is_builder() {
            Self::RequestBuilder(e.to_string())
        } else {
            Self::NetworkError(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LlmError;

    #[test]
    fn api_error_carries_provider_model() {
        let error = LlmError::api_error("test error")
            .with_provider("openrouter")
            .with_model("deepseek-v3.1");

        match error {
            LlmError::ApiError {
                provider,
                model,
                message,
                ..
            } => {
                assert_eq!(provider.as_deref(), Some("openrouter"));
                assert_eq!(model.as_deref(), Some("deepseek-v3.1"));
                assert_eq!(message, "test error");
            }
            _ => panic!("expected ApiError"),
        }
    }

    #[test]
    fn unknown_carries_provider_model() {
        let error = LlmError::unknown("something went wrong")
            .with_provider("mistral")
            .with_model("mistral-small-latest");

        match error {
            LlmError::Unknown {
                provider,
                model,
                message,
            } => {
                assert_eq!(provider.as_deref(), Some("mistral"));
                assert_eq!(model.as_deref(), Some("mistral-small-latest"));
                assert_eq!(message, "something went wrong");
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn with_provider_model_noop_on_other_variants() {
        let error = LlmError::NetworkError("timeout".to_string())
            .with_provider("test")
            .with_model("test");

        assert!(matches!(error, LlmError::NetworkError(_)));
    }

    #[test]
    fn api_error_defaults_to_none() {
        let error = LlmError::api_error("test");
        match error {
            LlmError::ApiError {
                provider: None,
                model: None,
                ..
            } => {}
            _ => panic!("expected ApiError with None provider/model"),
        }
    }
}
