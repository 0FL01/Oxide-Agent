use thiserror::Error;

#[derive(Debug, Error)]
pub enum BraveSearchError {
    #[error("search query cannot be empty")]
    EmptyQuery,
    #[error("Brave Search API key is not configured")]
    MissingApiKey,
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
            Self::Request(_) => "request",
            Self::InvalidResponse(_) => "invalid_response",
        }
    }
}
