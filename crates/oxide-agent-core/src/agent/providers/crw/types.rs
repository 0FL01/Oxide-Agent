use serde::{Deserialize, Serialize};

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
}

// --- Search response (Firecrawl-compatible) ---

/// CRW search API response.
#[derive(Debug, Deserialize)]
pub struct CrwSearchResponse {
    /// Whether the request succeeded.
    #[serde(default)]
    pub success: bool,
    /// Search result entries.
    #[serde(default)]
    pub data: Vec<CrwSearchResult>,
}

/// Single search result entry.
#[derive(Debug, Deserialize)]
pub struct CrwSearchResult {
    /// Result title.
    #[serde(default)]
    pub title: String,
    /// Result URL.
    #[serde(default)]
    pub url: String,
    /// Snippet or content excerpt.
    #[serde(default, alias = "description")]
    pub content: String,
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
        }
    }
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
            serde_json::json!({"query": "rust async reqwest timeout", "limit": 5})
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
