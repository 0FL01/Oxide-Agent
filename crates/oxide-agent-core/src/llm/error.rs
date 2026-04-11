use thiserror::Error;

/// Errors that can occur during LLM operations
#[derive(Debug, Error)]
pub enum LlmError {
    /// Error returned by the provider's API
    #[error("API error: {0}")]
    ApiError(String),
    /// Provider returned a successful response envelope without usable content.
    #[error("API error: Empty response{0}")]
    EmptyResponse(String),
    /// Error during network communication
    #[error("Network error: {0}")]
    NetworkError(String),
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
