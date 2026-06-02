use serde::{Deserialize, Serialize};

pub const TOOL_DUCKDUCKGO_SEARCH: &str = "duckduckgo_search";
pub const TOOL_DUCKDUCKGO_NEWS: &str = "duckduckgo_news";
pub const DEFAULT_MAX_RESULTS: u8 = 5;
pub const MAX_RESULTS_LIMIT: u8 = 10;
pub const DEFAULT_REGION: &str = "wt-wt";
pub const DEFAULT_SAFE_SEARCH: bool = true;

#[derive(Debug, Deserialize, Clone)]
pub struct DuckDuckGoSearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    #[serde(default = "default_region")]
    pub region: String,
}

impl DuckDuckGoSearchArgs {
    #[must_use]
    pub fn normalized_max_results(&self) -> usize {
        usize::from(self.max_results.clamp(1, MAX_RESULTS_LIMIT))
    }

    #[must_use]
    pub fn normalized_region(&self) -> &str {
        normalized_region(&self.region)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DuckDuckGoNewsArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default = "default_safe_search")]
    pub safe_search: bool,
}

impl DuckDuckGoNewsArgs {
    #[must_use]
    pub fn normalized_max_results(&self) -> usize {
        usize::from(self.max_results.clamp(1, MAX_RESULTS_LIMIT))
    }

    #[must_use]
    pub fn normalized_region(&self) -> &str {
        normalized_region(&self.region)
    }
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

fn default_region() -> String {
    DEFAULT_REGION.to_string()
}

const fn default_safe_search() -> bool {
    DEFAULT_SAFE_SEARCH
}

fn normalized_region(region: &str) -> &str {
    let trimmed = region.trim();
    if trimmed.is_empty() {
        DEFAULT_REGION
    } else {
        trimmed
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DuckDuckGoSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DuckDuckGoNewsResult {
    pub date: String,
    pub title: String,
    pub source: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DuckDuckGoResultKind {
    Search,
    News,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DuckDuckGoStructuredPayload<T> {
    pub provider: &'static str,
    pub kind: DuckDuckGoResultKind,
    pub query: String,
    pub region: String,
    pub results: Vec<T>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_args_clamp_result_count() {
        let args = DuckDuckGoSearchArgs {
            query: "rust".to_string(),
            max_results: 42,
            region: DEFAULT_REGION.to_string(),
        };

        assert_eq!(args.normalized_max_results(), 10);
    }

    #[test]
    fn news_args_default_safe_search_is_enabled() {
        let args: DuckDuckGoNewsArgs =
            serde_json::from_value(serde_json::json!({"query": "rust"})).expect("valid args");

        assert!(args.safe_search);
        assert_eq!(args.normalized_region(), DEFAULT_REGION);
        assert_eq!(args.normalized_max_results(), 5);
    }
}
