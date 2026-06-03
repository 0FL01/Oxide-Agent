use thiserror::Error;

#[derive(Debug, Error)]
pub enum BraveSearchError {
    #[error("search query cannot be empty")]
    EmptyQuery,
    #[error("Brave Search API key is not configured")]
    MissingApiKey,
    #[error("Brave Search authentication failed with HTTP {status}")]
    Auth { status: reqwest::StatusCode },
    #[error("Brave Search is rate-limited")]
    RateLimited,
    #[error("Brave Search server returned HTTP {status}: {body}")]
    Server {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Brave Search request timed out")]
    Timeout,
    #[error("Brave Search network request failed: {0}")]
    Network(String),
    #[error("Brave Search returned HTTP {status}: {body}")]
    HttpStatus {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("Brave Search request failed: {0}")]
    Request(String),
    #[error("Brave Search response could not be parsed: {0}")]
    InvalidResponse(String),
}

impl BraveSearchError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EmptyQuery => "empty_query",
            Self::MissingApiKey => "missing_api_key",
            Self::Auth { .. } => "auth",
            Self::RateLimited => "rate_limited",
            Self::Server { .. } => "server",
            Self::Timeout => "timeout",
            Self::Network(_) => "network",
            Self::HttpStatus { .. } => "http_status",
            Self::Request(_) => "request",
            Self::InvalidResponse(_) => "invalid_response",
        }
    }

    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Server { .. } | Self::Timeout | Self::Network(_))
    }

    #[must_use]
    pub const fn provider_unavailable(&self) -> bool {
        matches!(
            self,
            Self::MissingApiKey
                | Self::Auth { .. }
                | Self::RateLimited
                | Self::Server { .. }
                | Self::Timeout
                | Self::Network(_)
        )
    }
}
