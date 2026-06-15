use thiserror::Error;

/// Errors returned by CRW API operations.
#[derive(Debug, Error)]
pub enum CrwError {
    /// Search query was empty after trimming.
    #[error("search query cannot be empty")]
    EmptyQuery,
    /// URL was empty or invalid.
    #[error("invalid URL")]
    InvalidUrl,
    /// CRW returned a non-success HTTP status.
    #[error("CRW returned HTTP {status}: {body}")]
    HttpStatus {
        /// HTTP status code.
        status: reqwest::StatusCode,
        /// Truncated response body for diagnostics.
        body: String,
    },
    /// CRW returned a JSON failure envelope with HTTP success status.
    #[error("CRW API failure: {message}")]
    ApiFailure {
        /// Provider-supplied error message, truncated by the client.
        message: String,
    },
    /// Underlying reqwest transport error.
    #[error("CRW request failed: {0}")]
    Request(#[from] reqwest::Error),
}

impl CrwError {
    /// Classifies whether the error is transient and worth retrying.
    ///
    /// Retryable: 429, 502, 503, 504, network timeouts, connection refused/reset.
    /// Not retryable: 400, 401, 403, 404, `EmptyQuery`, `InvalidUrl`, builder errors.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::EmptyQuery | Self::InvalidUrl => false,
            Self::HttpStatus { status, .. } => matches!(status.as_u16(), 429 | 502 | 503 | 504),
            Self::ApiFailure { .. } => false,
            Self::Request(err) => is_retryable_reqwest(err),
        }
    }

    /// Returns a short, agent-friendly error message (no HTTP bodies or status codes).
    #[must_use]
    pub fn agent_message(&self) -> String {
        match self {
            Self::EmptyQuery => "Search query cannot be empty".to_string(),
            Self::InvalidUrl => "Invalid URL".to_string(),
            Self::HttpStatus { status, .. } => {
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    "Search authentication error".to_string()
                } else if status.is_client_error() {
                    "Search configuration error".to_string()
                } else {
                    "Search temporarily unavailable, please try again in a moment".to_string()
                }
            }
            Self::ApiFailure { message } => {
                if is_auth_message(message) {
                    "Search authentication error".to_string()
                } else {
                    "Search provider returned an error".to_string()
                }
            }
            Self::Request(err) => {
                if err.is_timeout() || err.is_connect() {
                    "Search temporarily unavailable, please try again in a moment".to_string()
                } else if err.is_decode() {
                    "Search request failed (invalid provider response format)".to_string()
                } else {
                    "Search request failed (transport error)".to_string()
                }
            }
        }
    }

    /// Short error-kind string for structured payload metadata.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "empty_query",
            Self::InvalidUrl => "invalid_url",
            Self::HttpStatus { status, .. } => {
                let code = status.as_u16();
                match code {
                    401 | 403 => "crw_auth_failed",
                    408 => "crw_timeout",
                    429 => "crw_rate_limited",
                    502..=504 => "crw_unavailable",
                    _ => "crw_http_status",
                }
            }
            Self::ApiFailure { message } => {
                if is_auth_message(message) {
                    "crw_auth_failed"
                } else {
                    "crw_api_failure"
                }
            }
            Self::Request(err) => {
                if err.is_timeout() {
                    "crw_timeout"
                } else if err.is_connect() {
                    "crw_connect"
                } else {
                    "crw_network"
                }
            }
        }
    }
}

fn is_auth_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("auth")
        || lower.contains("api key")
        || lower.contains("token")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
}

fn is_retryable_reqwest(err: &reqwest::Error) -> bool {
    if err.is_builder() {
        return false;
    }
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    let msg = err.to_string().to_lowercase();
    msg.contains("connection reset")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("eof")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn http_429_is_retryable() {
        let err = CrwError::HttpStatus {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: String::new(),
        };
        assert!(err.is_retryable());
        assert_eq!(err.kind(), "crw_rate_limited");
    }

    #[test]
    fn http_403_is_not_retryable() {
        let err = CrwError::HttpStatus {
            status: StatusCode::FORBIDDEN,
            body: String::new(),
        };
        assert!(!err.is_retryable());
        assert_eq!(err.kind(), "crw_auth_failed");
    }

    #[test]
    fn empty_query_is_not_retryable() {
        assert!(!CrwError::EmptyQuery.is_retryable());
        assert_eq!(CrwError::EmptyQuery.kind(), "empty_query");
    }

    #[test]
    fn invalid_url_is_not_retryable() {
        assert!(!CrwError::InvalidUrl.is_retryable());
        assert_eq!(CrwError::InvalidUrl.kind(), "invalid_url");
    }

    #[test]
    fn http_503_is_retryable_and_unavailable() {
        let err = CrwError::HttpStatus {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: String::new(),
        };
        assert!(err.is_retryable());
        assert_eq!(err.kind(), "crw_unavailable");
    }

    #[test]
    fn api_failure_auth_message_is_auth_failed() {
        let err = CrwError::ApiFailure {
            message: "Invalid API key".to_string(),
        };
        assert!(!err.is_retryable());
        assert_eq!(err.kind(), "crw_auth_failed");
        assert_eq!(err.agent_message(), "Search authentication error");
    }
}
