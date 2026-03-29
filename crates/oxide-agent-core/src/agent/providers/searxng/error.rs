use thiserror::Error;

#[derive(Debug, Error)]
pub enum SearxngError {
    #[error("search query cannot be empty")]
    EmptyQuery,
    #[error("SearXNG returned HTTP {status}: {body}")]
    HttpStatus {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("SearXNG request failed: {0}")]
    Request(#[from] reqwest::Error),
}

impl SearxngError {
    /// Classifies whether the error is transient and worth retrying.
    ///
    /// Retryable: 429, 502, 503, 504, network timeouts, connection refused/reset,
    /// JSON deserialization failures (often caused by incomplete responses).
    ///
    /// Not retryable: 400, 401, 403, 404, `EmptyQuery`, client builder errors.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::EmptyQuery => false,
            Self::HttpStatus { status, .. } => {
                matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
            }
            Self::Request(err) => is_retryable_reqwest(err),
        }
    }

    /// Returns a short, agent-friendly error message (no HTTP bodies or status codes).
    ///
    /// - Transient errors (429, 5xx, timeouts) → retry suggestion
    /// - Client errors (4xx) → configuration hint  
    /// - Other failures → generic message
    #[must_use]
    pub fn agent_message(&self) -> String {
        match self {
            Self::EmptyQuery => "Search query cannot be empty".to_string(),
            Self::HttpStatus { status, .. } => {
                if status.is_client_error() {
                    "Search configuration error".to_string()
                } else {
                    // 5xx, 429, or other server-side issues
                    "Search temporarily unavailable, please try again in a moment".to_string()
                }
            }
            Self::Request(err) => {
                if err.is_timeout() || err.is_connect() {
                    "Search temporarily unavailable, please try again in a moment".to_string()
                } else if err.is_decode() {
                    "Search request failed (invalid provider response format)".to_string()
                } else if err.is_body() {
                    "Search request failed (response body read error)".to_string()
                } else if err.is_request() {
                    "Search request failed (request construction error)".to_string()
                } else {
                    "Search request failed (transport error)".to_string()
                }
            }
        }
    }
}

fn is_retryable_reqwest(err: &reqwest::Error) -> bool {
    if err.is_builder() {
        return false;
    }
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    // Inspect the inner error for connection-reset / refused patterns.
    let msg = err.to_string().to_lowercase();
    msg.contains("connection reset")
        || msg.contains("connection refused")
        || msg.contains("broken pipe")
        || msg.contains("eof")
}
