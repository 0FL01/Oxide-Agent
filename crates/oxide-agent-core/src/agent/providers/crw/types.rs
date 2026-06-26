use serde::{Deserialize, Serialize};

/// CRW `POST /v1/scrape` request body.
#[derive(Debug, Clone, Serialize)]
pub struct CrwScrapeRequest {
    /// URL to scrape.
    pub url: String,
    /// Output formats (always `["markdown"]`).
    pub formats: Vec<String>,
    /// Whether to render JavaScript. `false` = HTTP-only.
    #[serde(skip_serializing_if = "Option::is_none", rename = "renderJs")]
    pub render_js: Option<bool>,
    /// Pin to a specific renderer: `lightpanda` or `playwright`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renderer: Option<String>,
    /// Milliseconds to wait after JS rendering for late content.
    #[serde(skip_serializing_if = "Option::is_none", rename = "waitFor")]
    pub wait_for: Option<u64>,
}

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
    /// Which renderer CRW actually used (e.g. `http`, `lightpanda`, `playwright`).
    #[serde(default, alias = "renderedWith")]
    pub rendered_with: Option<String>,
}

/// Arguments for CRW scrape (used by `web_crawler` rendered modes).
#[derive(Debug, Clone)]
pub struct CrwScrapeArgs {
    /// URL to scrape.
    pub url: String,
    /// Renderer to use: `lightpanda` or `playwright`.
    pub renderer: String,
    /// Milliseconds to wait after JS rendering for late content.
    pub wait_for_ms: u64,
}

impl CrwScrapeArgs {
    /// Build the CRW scrape request body.
    #[must_use]
    pub fn to_request(&self) -> CrwScrapeRequest {
        CrwScrapeRequest {
            url: self.url.trim().to_string(),
            formats: vec!["markdown".to_string()],
            render_js: Some(true),
            renderer: Some(self.renderer.clone()),
            wait_for: Some(self.wait_for_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrape_request_serializes_rendered_markdown_contract() {
        let args = CrwScrapeArgs {
            url: " https://example.com/page ".to_string(),
            renderer: "lightpanda".to_string(),
            wait_for_ms: 750,
        };

        let json = serde_json::to_value(args.to_request()).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "url": "https://example.com/page",
                "formats": ["markdown"],
                "renderJs": true,
                "renderer": "lightpanda",
                "waitFor": 750
            })
        );
    }
}
