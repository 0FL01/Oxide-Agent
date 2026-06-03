use super::error::BraveSearchError;
use serde::{Deserialize, Serialize};

pub const TOOL_NAME: &str = "brave_search";
pub const DEFAULT_MAX_RESULTS: u8 = 5;
pub const MAX_RESULTS_LIMIT: u8 = 10;
pub const DEFAULT_PAGE: u8 = 1;
pub const DEFAULT_SAFESEARCH: &str = "moderate";

#[derive(Debug, Deserialize, Clone)]
pub struct BraveSearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    pub country: Option<String>,
    pub search_lang: Option<String>,
    pub ui_lang: Option<String>,
    pub freshness: Option<String>,
    pub safesearch: Option<String>,
    #[serde(default)]
    pub extra_snippets: bool,
    #[serde(default = "default_page")]
    pub page: u8,
}

impl BraveSearchArgs {
    /// Normalize agent-facing arguments into Brave Web Search query parameters.
    ///
    /// Brave supports larger result counts, but the tool intentionally caps
    /// results at 10 to keep search output compact for agent context.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::EmptyQuery`] when `query` is empty after
    /// trimming whitespace.
    pub fn normalized(
        &self,
        default_safesearch: &str,
    ) -> Result<NormalizedBraveSearchArgs, BraveSearchError> {
        let query = self.query.trim();
        if query.is_empty() {
            return Err(BraveSearchError::EmptyQuery);
        }

        let page = self.page.clamp(1, 10);

        Ok(NormalizedBraveSearchArgs {
            query: query.to_string(),
            max_results: self.max_results.clamp(1, MAX_RESULTS_LIMIT),
            offset: page - 1,
            country: normalize_optional_string(self.country.as_deref()),
            search_lang: normalize_optional_string(self.search_lang.as_deref()),
            ui_lang: normalize_optional_string(self.ui_lang.as_deref()),
            freshness: normalize_freshness(self.freshness.as_deref()),
            safesearch: normalize_safesearch(self.safesearch.as_deref(), default_safesearch),
            extra_snippets: self.extra_snippets,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedBraveSearchArgs {
    pub query: String,
    pub max_results: u8,
    /// Brave Web Search offset is zero-based. Agent-facing `page` is one-based.
    pub offset: u8,
    pub country: Option<String>,
    pub search_lang: Option<String>,
    pub ui_lang: Option<String>,
    pub freshness: Option<String>,
    pub safesearch: String,
    pub extra_snippets: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BraveSearchResponse {
    #[serde(default)]
    pub web: Option<BraveWebResults>,
    #[serde(default)]
    pub query: Option<BraveQuery>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BraveQuery {
    #[serde(flatten)]
    pub raw: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BraveWebResults {
    #[serde(default)]
    pub results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct BraveWebResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub age: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub family_friendly: Option<bool>,
    #[serde(default)]
    pub extra_snippets: Vec<String>,
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

const fn default_page() -> u8 {
    DEFAULT_PAGE
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_freshness(value: Option<&str>) -> Option<String> {
    normalize_optional_string(value)
}

fn normalize_safesearch(value: Option<&str>, default_safesearch: &str) -> String {
    if let Some(value) = value.and_then(normalize_safesearch_value) {
        return value.to_string();
    }

    normalize_safesearch_value(default_safesearch)
        .unwrap_or(DEFAULT_SAFESEARCH)
        .to_string()
}

fn normalize_safesearch_value(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some("off"),
        "moderate" => Some("moderate"),
        "strict" => Some("strict"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_limits_page_offset_and_options() {
        let args = BraveSearchArgs {
            query: "  rust async  ".to_string(),
            max_results: 42,
            country: Some(" US ".to_string()),
            search_lang: Some(" en ".to_string()),
            ui_lang: Some(" en-US ".to_string()),
            freshness: Some(" pw ".to_string()),
            safesearch: Some(" STRICT ".to_string()),
            extra_snippets: true,
            page: 12,
        };

        let normalized = args.normalized(DEFAULT_SAFESEARCH).expect("valid args");

        assert_eq!(normalized.query, "rust async");
        assert_eq!(normalized.max_results, MAX_RESULTS_LIMIT);
        assert_eq!(normalized.offset, 9);
        assert_eq!(normalized.country.as_deref(), Some("US"));
        assert_eq!(normalized.search_lang.as_deref(), Some("en"));
        assert_eq!(normalized.ui_lang.as_deref(), Some("en-US"));
        assert_eq!(normalized.freshness.as_deref(), Some("pw"));
        assert_eq!(normalized.safesearch, "strict");
        assert!(normalized.extra_snippets);
    }

    #[test]
    fn keeps_custom_freshness_and_defaults_safesearch_from_config() {
        let args: BraveSearchArgs = serde_json::from_value(json!({
            "query": "rust",
            "max_results": 0,
            "freshness": "2024-01-01to2024-01-31",
            "safesearch": "invalid",
            "page": 0
        }))
        .expect("valid args");

        let normalized = args.normalized("off").expect("valid args");

        assert_eq!(normalized.max_results, 1);
        assert_eq!(normalized.offset, 0);
        assert_eq!(
            normalized.freshness.as_deref(),
            Some("2024-01-01to2024-01-31")
        );
        assert_eq!(normalized.safesearch, "off");
    }

    #[test]
    fn empty_query_is_rejected() {
        let args: BraveSearchArgs =
            serde_json::from_value(json!({"query": "   "})).expect("valid JSON args");

        let error = args
            .normalized(DEFAULT_SAFESEARCH)
            .expect_err("query is empty");

        assert!(matches!(error, BraveSearchError::EmptyQuery));
        assert_eq!(error.code(), "empty_query");
    }

    #[test]
    fn deserializes_web_results_and_ignores_other_sections() {
        let value = json!({
            "query": {"original": "rust"},
            "web": {
                "results": [
                    {
                        "title": "Rust",
                        "url": "https://www.rust-lang.org/",
                        "description": "A language empowering everyone.",
                        "age": "2 days ago",
                        "language": "en",
                        "family_friendly": true,
                        "extra_snippets": ["memory safety", "fearless concurrency"]
                    },
                    {"url": "https://example.com/minimal"}
                ]
            },
            "news": {"results": [{"title": "ignored"}]},
            "videos": {"results": [{"title": "ignored"}]}
        });

        let parsed: BraveSearchResponse = serde_json::from_value(value).expect("response parses");
        let results = parsed.web.expect("web results").results;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust");
        assert_eq!(results[0].extra_snippets.len(), 2);
        assert_eq!(results[1].title, "");
        assert_eq!(results[1].description, "");
    }
}
