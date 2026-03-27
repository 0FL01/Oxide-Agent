use serde::{Deserialize, Serialize};

pub const TOOL_NAME: &str = "searxng_search";
pub const DEFAULT_MAX_RESULTS: u8 = 5;
pub const MAX_RESULTS_LIMIT: u8 = 10;
pub const DEFAULT_PAGE: u16 = 1;

#[derive(Debug, Deserialize)]
pub struct SearxngSearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    pub language: Option<String>,
    pub time_range: Option<String>,
    pub safe_search: Option<u8>,
    pub categories: Option<Vec<String>>,
    pub engines: Option<Vec<String>>,
    #[serde(default = "default_page")]
    pub page: u16,
}

impl SearxngSearchArgs {
    #[must_use]
    pub fn normalized_max_results(&self) -> usize {
        usize::from(self.max_results.clamp(1, MAX_RESULTS_LIMIT))
    }

    #[must_use]
    pub fn normalized_page(&self) -> u16 {
        self.page.max(1)
    }

    #[must_use]
    pub fn normalized_safe_search(&self) -> Option<u8> {
        self.safe_search.map(|value| value.min(2))
    }
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

const fn default_page() -> u16 {
    DEFAULT_PAGE
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearxngSearchResponse {
    #[serde(default)]
    pub results: Vec<SearxngResult>,
    #[serde(default)]
    pub answers: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
    #[serde(default)]
    pub corrections: Vec<String>,
    pub number_of_results: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearxngResult {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub engine: Option<String>,
}
