//! Crawl4AI provider - deep crawling, markdown extraction, and PDF export.
//!
//! Provides `deep_crawl`, `web_markdown`, and `web_pdf` tools via a Crawl4AI sidecar.

mod response;

#[cfg(test)]
mod tests;

use crate::agent::provider::ToolProvider;
use crate::config::get_crawl4ai_timeout;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tracing::debug;

use response::{
    build_crawl_body, format_crawl_output, format_http_error, format_markdown_output,
    format_pdf_output, is_json_response, is_pdf_response, ResponsePayload,
};

/// Provider for Crawl4AI tools.
pub struct Crawl4aiProvider {
    base_url: String,
    client: reqwest::Client,
    timeout: Duration,
}

impl Crawl4aiProvider {
    /// Create a new Crawl4AI provider with the default timeout.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        let timeout = Duration::from_secs(get_crawl4ai_timeout());
        Self::with_timeout(base_url, timeout)
    }

    fn with_timeout(base_url: &str, timeout: Duration) -> Self {
        let client = match reqwest::Client::builder().timeout(timeout).build() {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            timeout,
        }
    }

    fn endpoint_url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    async fn post(&self, path: &str, body: Value) -> Result<ResponsePayload> {
        let url = self.endpoint_url(path);
        debug!(url = %url, timeout_secs = self.timeout.as_secs(), "Crawl4AI request");

        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| anyhow!("Crawl4AI request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!(format_http_error(status, &text)));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow!("Crawl4AI response read failed: {e}"))?;

        if is_pdf_response(&content_type, bytes.as_ref()) {
            return Ok(ResponsePayload::Pdf(bytes.to_vec()));
        }

        let text = String::from_utf8_lossy(bytes.as_ref()).to_string();
        if is_json_response(&content_type, bytes.as_ref()) {
            match serde_json::from_slice::<Value>(bytes.as_ref()) {
                Ok(value) => Ok(ResponsePayload::Json(value)),
                Err(_) => Ok(ResponsePayload::Text(text)),
            }
        } else {
            Ok(ResponsePayload::Text(text))
        }
    }
}

/// Arguments for `deep_crawl` tool.
#[derive(Debug, Deserialize)]
struct DeepCrawlArgs {
    urls: Vec<String>,
    max_depth: Option<u8>,
}

/// Arguments for `web_markdown` tool.
#[derive(Debug, Deserialize)]
struct WebMarkdownArgs {
    url: String,
}

/// Arguments for `web_pdf` tool.
#[derive(Debug, Deserialize)]
struct WebPdfArgs {
    url: String,
}

#[async_trait]
impl ToolProvider for Crawl4aiProvider {
    fn name(&self) -> &'static str {
        "crawl4ai"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "deep_crawl".to_string(),
                description: "Deep crawl website with JS rendering. Use for dynamic sites or multi-page discovery.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "List of URLs to crawl"
                        },
                        "max_depth": {
                            "type": "integer",
                            "description": "Optional crawl depth limit"
                        }
                    },
                    "required": ["urls"]
                }),
            },
            ToolDefinition {
                name: "web_markdown".to_string(),
                description: "Extract markdown from a single URL. Use for fast page reading.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL to extract"
                        }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "web_pdf".to_string(),
                description: "Export webpage to PDF. Returns base64 PDF when possible.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "URL to export"
                        }
                    },
                    "required": ["url"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, "deep_crawl" | "web_markdown" | "web_pdf")
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if let Some(token) = cancellation_token {
            if token.is_cancelled() {
                return Err(anyhow!("Crawl4AI request cancelled"));
            }
        }

        match tool_name {
            "deep_crawl" => {
                let args: DeepCrawlArgs = serde_json::from_str(arguments)?;
                if args.urls.is_empty() {
                    return Err(anyhow!("deep_crawl requires at least one URL"));
                }

                let body = build_crawl_body(args.urls, args.max_depth);
                let payload = self.post("/crawl", body).await?;
                Ok(format_crawl_output(payload))
            }
            "web_markdown" => {
                let args: WebMarkdownArgs = serde_json::from_str(arguments)?;
                if args.url.trim().is_empty() {
                    return Err(anyhow!("web_markdown requires a URL"));
                }

                let body = json!({
                    "url": args.url,
                    "f": "fit"
                });
                let payload = self.post("/md", body).await?;
                Ok(format_markdown_output(payload))
            }
            "web_pdf" => {
                let args: WebPdfArgs = serde_json::from_str(arguments)?;
                if args.url.trim().is_empty() {
                    return Err(anyhow!("web_pdf requires a URL"));
                }

                let body = json!({ "url": args.url });
                let payload = self.post("/pdf", body).await?;
                format_pdf_output(payload)
            }
            _ => Err(anyhow!("Unknown Crawl4AI tool: {tool_name}")),
        }
    }
}
