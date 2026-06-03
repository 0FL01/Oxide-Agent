use super::client::BraveSearchClient;
use super::error::BraveSearchError;
use crate::config::{
    get_brave_search_api_key, get_brave_search_country, get_brave_search_lang,
    get_brave_search_max_concurrent, get_brave_search_min_delay_ms, get_brave_search_safesearch,
    get_brave_search_timeout, get_brave_search_ui_lang,
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

/// Brave Search provider construction defaults.
#[derive(Debug, Clone)]
pub struct BraveSearchProviderConfig {
    /// HTTP request timeout.
    pub timeout: Duration,
    /// Default Brave `country` query parameter.
    pub default_country: String,
    /// Default Brave `search_lang` query parameter.
    pub default_search_lang: String,
    /// Default Brave `ui_lang` query parameter.
    pub default_ui_lang: String,
    /// Default Brave `safesearch` query parameter.
    pub default_safesearch: String,
    /// Maximum concurrent Brave requests.
    pub max_concurrent: usize,
    /// Minimum delay between Brave request starts.
    pub min_delay: Duration,
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
            config_from_env(),
        )
    }

    /// Create a provider with explicit defaults.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when `api_key` is empty.
    pub fn new(
        api_key: impl Into<String>,
        config: BraveSearchProviderConfig,
    ) -> Result<Self, BraveSearchError> {
        Ok(Self {
            client: BraveSearchClient::new(
                api_key,
                config.timeout,
                config.max_concurrent,
                config.min_delay,
            )?,
            default_country: config.default_country,
            default_search_lang: config.default_search_lang,
            default_ui_lang: config.default_ui_lang,
            default_safesearch: config.default_safesearch,
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

fn config_from_env() -> BraveSearchProviderConfig {
    BraveSearchProviderConfig {
        timeout: Duration::from_secs(get_brave_search_timeout()),
        default_country: get_brave_search_country(),
        default_search_lang: get_brave_search_lang(),
        default_ui_lang: get_brave_search_ui_lang(),
        default_safesearch: get_brave_search_safesearch(),
        max_concurrent: get_brave_search_max_concurrent(),
        min_delay: Duration::from_millis(get_brave_search_min_delay_ms()),
    }
}
