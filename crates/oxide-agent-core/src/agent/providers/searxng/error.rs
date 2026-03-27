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
