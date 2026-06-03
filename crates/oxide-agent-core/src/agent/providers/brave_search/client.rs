use super::error::BraveSearchError;
use std::time::Duration;

pub const BRAVE_WEB_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

#[derive(Debug, Clone)]
pub struct BraveSearchClient {
    api_key: String,
    timeout: Duration,
}

impl BraveSearchClient {
    /// Create a Brave Search client skeleton without issuing network requests.
    ///
    /// HTTP execution is intentionally implemented in a later chunk.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when `api_key` is empty.
    pub fn new(api_key: impl Into<String>, timeout: Duration) -> Result<Self, BraveSearchError> {
        let api_key = api_key.into().trim().to_string();
        if api_key.is_empty() {
            return Err(BraveSearchError::MissingApiKey);
        }

        Ok(Self { api_key, timeout })
    }

    #[must_use]
    pub fn endpoint(&self) -> &'static str {
        BRAVE_WEB_SEARCH_ENDPOINT
    }

    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}
