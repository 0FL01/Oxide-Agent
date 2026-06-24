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
        /// Provider that produced the error (e.g. `"openrouter"`, `"zai"`).
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
    /// Provider returned a typed error indicating the request exceeded the
    /// context window. Classified from `ApiError` by [`Self::try_classify_context_overflow`].
    #[error("Context overflow: {message}")]
    ContextOverflow {
        /// Error message from the provider.
        message: String,
        /// Provider that produced the error, if known.
        provider: Option<String>,
        /// Model identifier that produced the error, if known.
        model: Option<String>,
    },
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

    /// Attach the provider name to `ApiError`, `ContextOverflow`, or `Unknown` variants.
    /// Other variants are returned unchanged.
    #[must_use]
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        match &mut self {
            Self::ApiError { provider: p, .. } => *p = Some(provider.into()),
            Self::ContextOverflow { provider: p, .. } => *p = Some(provider.into()),
            Self::Unknown { provider: p, .. } => *p = Some(provider.into()),
            _ => {}
        }
        self
    }

    /// Attach the model identifier to `ApiError`, `ContextOverflow`, or `Unknown` variants.
    /// Other variants are returned unchanged.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        match &mut self {
            Self::ApiError { model: m, .. } => *m = Some(model.into()),
            Self::ContextOverflow { model: m, .. } => *m = Some(model.into()),
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

    /// Check if this error is a typed context-overflow error.
    ///
    /// Use this instead of string matching on `to_string()`. Providers should
    /// call [`Self::try_classify_context_overflow`] when constructing errors
    /// so that downstream code can use this typed check.
    #[must_use]
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, Self::ContextOverflow { .. })
    }

    /// Attempt to classify an `ApiError` as a typed `ContextOverflow`.
    ///
    /// This centralizes the classification logic that was previously scattered
    /// as string matching in the runner (`llm_error_suggests_context_overflow`).
    /// Providers return HTTP 400 or 413 for context-overflow errors; the
    /// response body contains provider-specific error text. This method checks
    /// the typed status code first, then inspects the message for known
    /// context-overflow indicators. This is HTTP API error response parsing,
    /// not heuristic over LLM output.
    ///
    /// Classification rules:
    /// - `ApiError { status: Some(400|413), .. }` + message contains overflow
    ///   indicator → `ContextOverflow`.
    /// - `ApiError { status: None, .. }` + message contains overflow indicator
    ///   → `ContextOverflow` (status unknown, but message is clear).
    /// - `ApiError { status: Some(other), .. }` → unchanged (different error
    ///   type, e.g. 429 rate limit).
    /// - Other variants → unchanged.
    ///
    /// If the error is not an `ApiError` with an overflow-indicating status and
    /// message, it is returned unchanged.
    #[must_use]
    pub fn try_classify_context_overflow(self) -> Self {
        const INDICATORS: &[&str] = &[
            "context length",
            "context window",
            "too many tokens",
            "token limit",
            "maximum context",
            "prompt is too long",
            "context overflow",
        ];

        let is_overflow_message = |message: &str| {
            let lower = message.to_ascii_lowercase();
            INDICATORS.iter().any(|needle| lower.contains(needle))
        };

        match &self {
            // HTTP 400 or 413 with overflow indicator → classify.
            Self::ApiError {
                status: Some(s),
                message,
                provider,
                model,
            } if (*s == 400 || *s == 413) && is_overflow_message(message) => {
                Self::ContextOverflow {
                    message: message.clone(),
                    provider: provider.clone(),
                    model: model.clone(),
                }
            }
            // Unknown HTTP status (None) with overflow indicator → classify.
            Self::ApiError {
                status: None,
                message,
                provider,
                model,
            } if is_overflow_message(message) => Self::ContextOverflow {
                message: message.clone(),
                provider: provider.clone(),
                model: model.clone(),
            },
            _ => self,
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
            .with_provider("zai")
            .with_model("glm-4.6");

        match error {
            LlmError::Unknown {
                provider,
                model,
                message,
            } => {
                assert_eq!(provider.as_deref(), Some("zai"));
                assert_eq!(model.as_deref(), Some("glm-4.6"));
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

    #[test]
    fn is_context_overflow_typed_match() {
        let overflow = LlmError::ContextOverflow {
            message: "context length exceeded".to_string(),
            provider: Some("openrouter".to_string()),
            model: Some("test-model".to_string()),
        };
        assert!(overflow.is_context_overflow());

        let api = LlmError::api_error("some other error");
        assert!(!api.is_context_overflow());

        let network = LlmError::NetworkError("timeout".to_string());
        assert!(!network.is_context_overflow());
    }

    #[test]
    fn try_classify_context_overflow_400_with_indicator() {
        let error =
            LlmError::api_error_status(400, "This request exceeds the context length limit")
                .with_provider("openrouter")
                .with_model("test-model");
        let classified = error.try_classify_context_overflow();
        assert!(classified.is_context_overflow());
        match classified {
            LlmError::ContextOverflow {
                provider,
                model,
                message,
            } => {
                assert_eq!(provider.as_deref(), Some("openrouter"));
                assert_eq!(model.as_deref(), Some("test-model"));
                assert!(message.contains("context length"));
            }
            _ => panic!("expected ContextOverflow"),
        }
    }

    #[test]
    fn try_classify_context_overflow_413_with_indicator() {
        let error = LlmError::api_error_status(413, "Prompt is too long for context window");
        let classified = error.try_classify_context_overflow();
        assert!(classified.is_context_overflow());
    }

    #[test]
    fn try_classify_context_overflow_none_status_with_indicator() {
        let error = LlmError::api_error("maximum context length exceeded");
        let classified = error.try_classify_context_overflow();
        assert!(classified.is_context_overflow());
    }

    #[test]
    fn try_classify_context_overflow_400_without_indicator_unchanged() {
        let error = LlmError::api_error_status(400, "Invalid request body");
        let classified = error.try_classify_context_overflow();
        assert!(!classified.is_context_overflow());
        assert!(matches!(classified, LlmError::ApiError { .. }));
    }

    #[test]
    fn try_classify_context_overflow_non_400_unchanged() {
        let error = LlmError::api_error_status(429, "Rate limited: too many tokens");
        let classified = error.try_classify_context_overflow();
        assert!(!classified.is_context_overflow());
        assert!(matches!(
            classified,
            LlmError::ApiError {
                status: Some(429),
                ..
            }
        ));
    }

    #[test]
    fn try_classify_context_overflow_non_api_error_unchanged() {
        let error = LlmError::NetworkError("context length connection error".to_string());
        let classified = error.try_classify_context_overflow();
        assert!(!classified.is_context_overflow());
        assert!(matches!(classified, LlmError::NetworkError(_)));
    }

    #[test]
    fn context_overflow_carries_provider_model() {
        let error = LlmError::ContextOverflow {
            message: "exceeded".to_string(),
            provider: None,
            model: None,
        }
        .with_provider("openrouter")
        .with_model("deepseek/deepseek-chat");
        match error {
            LlmError::ContextOverflow {
                provider, model, ..
            } => {
                assert_eq!(provider.as_deref(), Some("openrouter"));
                assert_eq!(model.as_deref(), Some("deepseek/deepseek-chat"));
            }
            _ => panic!("expected ContextOverflow"),
        }
    }

    #[test]
    fn with_provider_noop_on_other_variants() {
        let error = LlmError::NetworkError("timeout".to_string())
            .with_provider("test")
            .with_model("test");
        assert!(matches!(error, LlmError::NetworkError(_)));
    }
}
