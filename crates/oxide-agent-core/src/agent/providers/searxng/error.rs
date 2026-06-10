use chrono::Utc;
use serde_json::{Value, json};
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
    /// Stable machine-readable error kind for structured tool payloads.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "empty_query",
            Self::HttpStatus { .. } => "searxng_http_status",
            Self::Request(err) if err.is_timeout() => "searxng_timeout",
            Self::Request(err) if err.is_connect() => "searxng_connect",
            Self::Request(err) if err.is_decode() => "searxng_decode",
            Self::Request(err) if err.is_body() => "searxng_body",
            Self::Request(err) if err.is_request() => "searxng_request",
            Self::Request(_) => "searxng_transport",
        }
    }

    /// Whether this failure means the configured SearXNG provider is unavailable.
    #[must_use]
    pub fn provider_unavailable(&self) -> bool {
        match self {
            Self::EmptyQuery => false,
            Self::HttpStatus { status, .. } => status.as_u16() == 429 || status.is_server_error(),
            Self::Request(err) => err.is_timeout() || err.is_connect() || err.is_decode(),
        }
    }

    /// Structured failure payload for typed runtime output.
    #[must_use]
    pub fn failure_payload(&self, query: &str) -> Value {
        json!({
            "provider": super::types::TOOL_NAME,
            "kind": "search",
            "query": query.trim(),
            "error_kind": self.code(),
            "status_code": self.status_code(),
            "message": self.agent_message(),
            "provider_unavailable": self.provider_unavailable(),
            "retryable": self.is_retryable(),
            "results": [],
            "snippet_only": true,
            "fetched_at": Utc::now().to_rfc3339(),
        })
    }

    fn status_code(&self) -> Option<u16> {
        match self {
            Self::HttpStatus { status, .. } => Some(status.as_u16()),
            Self::EmptyQuery | Self::Request(_) => None,
        }
    }

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
