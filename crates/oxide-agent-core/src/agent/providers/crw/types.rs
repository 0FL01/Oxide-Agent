use serde::{Deserialize, Deserializer, Serialize};

/// LLM-facing tool name for CRW-backed web search.
pub const TOOL_WEB_SEARCH: &str = "web_search";

/// Default number of search results.
pub const DEFAULT_MAX_RESULTS: u8 = 5;
/// Maximum allowed search results.
pub const MAX_RESULTS_LIMIT: u8 = 10;

// --- Search request ---

/// Minimal CRW `POST /v1/search` request body.
#[derive(Debug, Clone, Serialize)]
pub struct CrwSearchRequest {
    /// Search query string.
    pub query: String,
    /// Maximum number of results.
    pub limit: u8,
    /// CRW search sources. Keep web explicit to avoid backend-default drift.
    pub sources: Vec<String>,
    /// Preferred search language code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// Search recency filter in SearXNG/Google-style qdr notation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tbs: Option<String>,
    /// Optional CRW/SearXNG search categories.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
}

// --- Search response (Firecrawl-compatible) ---

/// CRW search API response.
#[derive(Debug)]
pub struct CrwSearchResponse {
    /// Whether the request succeeded.
    pub success: bool,
    /// Search result entries.
    pub data: Vec<CrwSearchResult>,
    /// Optional provider error/message when CRW returns `success: false`.
    pub error: Option<String>,
}

impl<'de> Deserialize<'de> for CrwSearchResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawResponse {
            #[serde(default)]
            success: Option<bool>,
            #[serde(default)]
            data: serde_json::Value,
            #[serde(default)]
            results: serde_json::Value,
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            message: Option<String>,
        }

        let raw = RawResponse::deserialize(deserializer)?;
        let data = search_results_from_value(&raw.data)
            .or_else(|| search_results_from_value(&raw.results))
            .unwrap_or_default();

        Ok(Self {
            success: raw.success.unwrap_or(true),
            data,
            error: raw.error.or(raw.message),
        })
    }
}

fn search_results_from_value(value: &serde_json::Value) -> Option<Vec<CrwSearchResult>> {
    if value.is_null() {
        return Some(Vec::new());
    }

    if value.is_array() {
        return Some(parse_search_result_array(value));
    }

    let object = value.as_object()?;
    if let Some(results) = object.get("results") {
        if results.is_array() {
            return Some(parse_search_result_array(results));
        }
        if let Some(grouped) = results.as_object() {
            return Some(flatten_search_result_groups(grouped));
        }
    }

    let flattened = flatten_search_result_groups(object);
    if flattened.is_empty() {
        None
    } else {
        Some(flattened)
    }
}

fn flatten_search_result_groups(
    grouped: &serde_json::Map<String, serde_json::Value>,
) -> Vec<CrwSearchResult> {
    let mut flattened = Vec::new();
    for entries in grouped.values() {
        if entries.is_array() {
            let mut parsed = parse_search_result_array(entries);
            flattened.append(&mut parsed);
            continue;
        }

        if let Some(nested_results) = entries.get("results")
            && nested_results.is_array()
        {
            let mut parsed = parse_search_result_array(nested_results);
            flattened.append(&mut parsed);
        }
    }
    flattened
}

fn parse_search_result_array(value: &serde_json::Value) -> Vec<CrwSearchResult> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(CrwSearchResult::from_json_value)
                .collect()
        })
        .unwrap_or_default()
}

/// Single search result entry.
#[derive(Debug)]
pub struct CrwSearchResult {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Snippet or content excerpt.
    pub content: String,
}

impl<'de> Deserialize<'de> for CrwSearchResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_json_value(&value)
            .ok_or_else(|| serde::de::Error::custom("expected search result object"))
    }
}

impl CrwSearchResult {
    fn from_json_value(value: &serde_json::Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            title: first_non_empty_string(object, &["title", "name"]),
            url: first_non_empty_string(object, &["url", "link", "href"]),
            content: first_non_empty_string(object, &["content", "description", "snippet"]),
        })
    }
}

fn first_non_empty_string(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> String {
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        let text = json_value_to_string(value);
        if !text.trim().is_empty() {
            return text;
        }
    }
    String::new()
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

// --- Scrape request ---

/// CRW `POST /v1/scrape` request body.
#[derive(Debug, Clone, Serialize)]
pub struct CrwScrapeRequest {
    /// URL to scrape.
    pub url: String,
    /// Output formats (always `["markdown"]`).
    pub formats: Vec<String>,
}

// --- Scrape response (Firecrawl-compatible) ---

/// CRW scrape API response.
#[derive(Debug, Deserialize)]
pub struct CrwScrapeResponse {
    /// Whether the request succeeded.
    #[serde(default)]
    pub success: bool,
    /// Scraped page data.
    #[serde(default)]
    pub data: CrwScrapeData,
}

/// Scraped page content.
#[derive(Debug, Default, Deserialize)]
pub struct CrwScrapeData {
    /// Markdown content of the page.
    #[serde(default)]
    pub markdown: String,
    /// Page metadata.
    #[serde(default)]
    pub metadata: CrwScrapeMetadata,
}

/// Metadata returned by CRW scrape.
#[derive(Debug, Default, Deserialize)]
pub struct CrwScrapeMetadata {
    /// Final URL after redirects.
    #[serde(default)]
    pub url: Option<String>,
    /// HTTP status code of the scraped page.
    #[serde(default, alias = "statusCode")]
    pub status_code: Option<u16>,
}

/// Arguments for the LLM-facing `web_search` tool.
#[derive(Debug, Deserialize, Clone)]
pub struct CrwSearchArgs {
    /// Search query.
    pub query: String,
    /// Maximum results (1-10, default 5).
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    /// Preferred search language code.
    pub language: Option<String>,
    /// Recency filter: `day`, `week`, `month`, `year`.
    pub time_range: Option<String>,
    /// Safe-search level (0-2).
    pub safe_search: Option<u8>,
    /// Optional categories (string or array, accepted for caller compatibility).
    pub categories: Option<serde_json::Value>,
    /// Result page number (accepted for caller compatibility).
    pub page: Option<u16>,
}

impl CrwSearchArgs {
    /// Clamp `max_results` to the valid 1..=10 range.
    #[must_use]
    pub fn normalized_max_results(&self) -> u8 {
        self.max_results.clamp(1, MAX_RESULTS_LIMIT)
    }

    /// Build the minimal CRW-compatible request body.
    #[must_use]
    pub fn to_request(&self) -> CrwSearchRequest {
        CrwSearchRequest {
            query: self.query.trim().to_string(),
            limit: self.normalized_max_results(),
            sources: vec!["web".to_string()],
            lang: normalize_optional_string(self.language.as_deref()),
            tbs: self
                .time_range
                .as_deref()
                .and_then(normalize_time_range_to_tbs),
            categories: normalize_categories(self.categories.as_ref()),
        }
    }
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_time_range_to_tbs(value: &str) -> Option<String> {
    match value.trim() {
        "day" => Some("qdr:d".to_string()),
        "week" => Some("qdr:w".to_string()),
        "month" => Some("qdr:m".to_string()),
        "year" => Some("qdr:y".to_string()),
        tbs if tbs.starts_with("qdr:") => Some(tbs.to_string()),
        _ => None,
    }
}

fn normalize_categories(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };

    let mut categories = match value {
        serde_json::Value::String(category) => split_category_string(category),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .flat_map(split_category_string)
            .collect(),
        _ => Vec::new(),
    };

    categories.sort();
    categories.dedup();
    categories
}

fn split_category_string(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

/// Arguments for CRW scrape (used internally by `web_crawler` fallback).
#[derive(Debug, Clone)]
pub struct CrwScrapeArgs {
    /// URL to scrape.
    pub url: String,
}

impl CrwScrapeArgs {
    /// Build the CRW scrape request body.
    #[must_use]
    pub fn to_request(&self) -> CrwScrapeRequest {
        CrwScrapeRequest {
            url: self.url.trim().to_string(),
            formats: vec!["markdown".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_args_builds_minimal_request() {
        let args = CrwSearchArgs {
            query: "rust async reqwest".to_string(),
            max_results: 5,
            language: None,
            time_range: None,
            safe_search: None,
            categories: None,
            page: None,
        };
        let req = args.to_request();
        assert_eq!(req.query, "rust async reqwest");
        assert_eq!(req.limit, 5);
        assert_eq!(req.sources, vec!["web"]);
        assert_eq!(req.lang, None);
        assert_eq!(req.tbs, None);
        assert!(req.categories.is_empty());
    }

    #[test]
    fn search_args_clamps_limit() {
        let args = CrwSearchArgs {
            query: "test".to_string(),
            max_results: 50,
            language: None,
            time_range: None,
            safe_search: None,
            categories: None,
            page: None,
        };
        assert_eq!(args.to_request().limit, 10);
    }

    #[test]
    fn search_request_serializes_to_expected_json() {
        let args = CrwSearchArgs {
            query: "rust async reqwest timeout".to_string(),
            max_results: 5,
            language: None,
            time_range: None,
            safe_search: None,
            categories: None,
            page: None,
        };
        let json = serde_json::to_value(args.to_request()).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "query": "rust async reqwest timeout",
                "limit": 5,
                "sources": ["web"]
            })
        );
    }

    #[test]
    fn search_request_serializes_supported_options() {
        let args = CrwSearchArgs {
            query: "rust async reqwest timeout".to_string(),
            max_results: 5,
            language: Some(" en ".to_string()),
            time_range: Some("month".to_string()),
            safe_search: Some(1),
            categories: Some(serde_json::json!(["research", " github ", "research"])),
            page: Some(2),
        };
        let json = serde_json::to_value(args.to_request()).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "query": "rust async reqwest timeout",
                "limit": 5,
                "sources": ["web"],
                "lang": "en",
                "tbs": "qdr:m",
                "categories": ["github", "research"]
            })
        );
    }

    #[test]
    fn scrape_request_serializes_with_markdown_format() {
        let args = CrwScrapeArgs {
            url: "https://example.com/page".to_string(),
        };
        let json = serde_json::to_value(args.to_request()).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({"url": "https://example.com/page", "formats": ["markdown"]})
        );
    }

    #[test]
    fn search_response_deserializes_firecrawl_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": [
                {"title": "Result 1", "url": "https://example.com/1", "description": "Snippet 1"},
                {"title": "Result 2", "url": "https://example.com/2", "content": "Content 2"}
            ]
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(resp.success);
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].content, "Snippet 1");
        assert_eq!(resp.data[1].content, "Content 2");
    }

    #[test]
    fn search_response_deserializes_crw_results_object_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "results": [
                    {"title": "Result 1", "url": "https://example.com/1", "snippet": "Snippet 1"},
                    {"title": "Result 2", "url": "https://example.com/2", "description": "Snippet 2"}
                ]
            }
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(resp.success);
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].content, "Snippet 1");
        assert_eq!(resp.data[1].content, "Snippet 2");
    }

    #[test]
    fn search_response_deserializes_grouped_results_object_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "results": {
                    "web": [
                        {"title": "Web", "url": "https://example.com/web", "snippet": "Web snippet"}
                    ],
                    "news": [
                        {"title": "News", "url": "https://example.com/news", "snippet": "News snippet"}
                    ]
                }
            }
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(resp.success);
        assert_eq!(resp.data.len(), 2);
        assert!(resp.data.iter().any(|result| result.title == "Web"));
        assert!(resp.data.iter().any(|result| result.title == "News"));
    }

    #[test]
    fn search_response_deserializes_direct_grouped_data_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "web": [
                    {"title": "Speedtest", "url": "https://example.com/speed", "snippet": "Montenegro speed"}
                ],
                "news": [
                    {"title": "Visa", "url": "https://example.com/visa", "snippet": "Digital nomad visa"}
                ]
            }
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(resp.data.len(), 2);
        assert!(resp.data.iter().any(|result| result.title == "Speedtest"));
        assert!(resp.data.iter().any(|result| result.title == "Visa"));
    }

    #[test]
    fn search_response_deserializes_nested_grouped_data_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "web": {
                    "results": [
                        {"title": "Nested", "url": "https://example.com/nested", "snippet": "Nested snippet"}
                    ]
                }
            }
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].title, "Nested");
        assert_eq!(resp.data[0].content, "Nested snippet");
    }

    #[test]
    fn search_response_keeps_grouped_results_with_null_fields() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "results": {
                    "web": [
                        {"title": "Wikipedia", "url": "https://www.wikipedia.org/", "description": null},
                        {"title": null, "url": "https://example.com/", "snippet": "Example"},
                        {"title": "Numeric", "url": "https://example.net/", "description": 123},
                        {"title": "Duplicate", "url": "https://example.org/", "description": "Description", "snippet": "Snippet"}
                    ]
                }
            }
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert_eq!(resp.data.len(), 4);
        assert_eq!(resp.data[0].title, "Wikipedia");
        assert_eq!(resp.data[0].content, "");
        assert_eq!(resp.data[1].title, "");
        assert_eq!(resp.data[1].content, "Example");
        assert_eq!(resp.data[2].content, "123");
        assert_eq!(resp.data[3].content, "Description");
    }

    #[test]
    fn scrape_response_deserializes_firecrawl_format() {
        let raw = serde_json::json!({
            "success": true,
            "data": {
                "markdown": "# Page Title\n\nContent here.",
                "metadata": {"url": "https://example.com", "statusCode": 200}
            }
        });
        let resp: CrwScrapeResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(resp.success);
        assert!(resp.data.markdown.contains("# Page Title"));
        assert_eq!(
            resp.data.metadata.url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(resp.data.metadata.status_code, Some(200));
    }

    #[test]
    fn search_response_tolerates_empty_data() {
        let raw = serde_json::json!({"success": true});
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(resp.data.is_empty());
    }

    #[test]
    fn search_response_preserves_success_false_error() {
        let raw = serde_json::json!({
            "success": false,
            "error": "Invalid API key"
        });
        let resp: CrwSearchResponse = serde_json::from_value(raw).expect("deserialize");
        assert!(!resp.success);
        assert!(resp.data.is_empty());
        assert_eq!(resp.error.as_deref(), Some("Invalid API key"));
    }

    #[test]
    fn search_args_deserializes_from_llm_payload() {
        let payload = r#"{"query":"hello world","max_results":3}"#;
        let args: CrwSearchArgs = serde_json::from_str(payload).expect("deserialize");
        assert_eq!(args.query, "hello world");
        assert_eq!(args.max_results, 3);
    }

    #[test]
    fn search_args_uses_default_max_results() {
        let payload = r#"{"query":"test"}"#;
        let args: CrwSearchArgs = serde_json::from_str(payload).expect("deserialize");
        assert_eq!(args.max_results, 5);
    }
}
