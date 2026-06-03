use super::client::BraveSearchClient;
use super::error::BraveSearchError;
use crate::config::{
    get_brave_search_api_key, get_brave_search_country, get_brave_search_lang,
    get_brave_search_safesearch, get_brave_search_timeout, get_brave_search_ui_lang,
};
use std::time::Duration;

#[derive(Debug, Clone)]
/// Tool provider skeleton for Brave Web Search.
pub struct BraveSearchProvider {
    client: BraveSearchClient,
    default_country: String,
    default_search_lang: String,
    default_ui_lang: String,
    default_safesearch: String,
}

impl BraveSearchProvider {
    /// Create a provider from global configuration.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when the Brave API key is not configured.
    pub fn new_from_config() -> Result<Self, BraveSearchError> {
        Self::new(
            get_brave_search_api_key().unwrap_or_default(),
            Duration::from_secs(get_brave_search_timeout()),
            get_brave_search_country(),
            get_brave_search_lang(),
            get_brave_search_ui_lang(),
            get_brave_search_safesearch(),
        )
    }

    /// Create a provider with explicit defaults.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when `api_key` is empty.
    pub fn new(
        api_key: impl Into<String>,
        timeout: Duration,
        default_country: impl Into<String>,
        default_search_lang: impl Into<String>,
        default_ui_lang: impl Into<String>,
        default_safesearch: impl Into<String>,
    ) -> Result<Self, BraveSearchError> {
        Ok(Self {
            client: BraveSearchClient::new(api_key, timeout)?,
            default_country: default_country.into(),
            default_search_lang: default_search_lang.into(),
            default_ui_lang: default_ui_lang.into(),
            default_safesearch: default_safesearch.into(),
        })
    }

    /// Return the underlying client skeleton.
    #[must_use]
    pub const fn client(&self) -> &BraveSearchClient {
        &self.client
    }

    /// Return the default Brave `country` query parameter.
    #[must_use]
    pub fn default_country(&self) -> &str {
        &self.default_country
    }

    /// Return the default Brave `search_lang` query parameter.
    #[must_use]
    pub fn default_search_lang(&self) -> &str {
        &self.default_search_lang
    }

    /// Return the default Brave `ui_lang` query parameter.
    #[must_use]
    pub fn default_ui_lang(&self) -> &str {
        &self.default_ui_lang
    }

    /// Return the default Brave `safesearch` query parameter.
    #[must_use]
    pub fn default_safesearch(&self) -> &str {
        &self.default_safesearch
    }
}
