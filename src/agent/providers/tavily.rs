//! Tavily Provider - web search and content extraction
//!
//! Provides `web_search` and `web_extract` tools using native Tavily Rust SDK.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use html_escape;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tavily::Tavily;
use tracing::debug;

/// Provider for Tavily web search tools
pub struct TavilyProvider {
    client: Tavily,
    api_key: String,
}

impl TavilyProvider {
    /// Create a new Tavily provider with the given API key
    ///
    /// # Errors
    ///
    /// Returns an error if the Tavily client cannot be created.
    pub fn new(api_key: &str) -> Result<Self> {
        let client = Tavily::builder(api_key)
            .timeout(Duration::from_secs(30))
            .max_retries(2)
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create Tavily client: {e}"))?;

        Ok(Self {
            client,
            api_key: api_key.to_string(),
        })
    }
}

/// Arguments for `web_search` tool
#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: u8,
}

const fn default_max_results() -> u8 {
    5
}

/// Arguments for `web_extract` tool
#[derive(Debug, Deserialize)]
struct WebExtractArgs {
    urls: Vec<String>,
}

#[async_trait]
impl ToolProvider for TavilyProvider {
    fn name(&self) -> &'static str {
        "tavily"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "web_search".to_string(),
                description: "Search the web for current information. Use for news, facts, documentation, real-time data. Returns relevant search results with titles, URLs, and content snippets.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "The search query"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Maximum number of results (1-10, default: 5)"
                        }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "web_extract".to_string(),
                description: "Extract and read content from web pages. Use to read articles, documentation, blog posts. Returns the full text content of the pages.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "urls": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "List of URLs to extract content from (max 5)"
                        }
                    },
                    "required": ["urls"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, "web_search" | "web_extract")
    }

    async fn execute(&self, tool_name: &str, arguments: &str) -> Result<String> {
        use std::fmt::Write;
        debug!(tool = tool_name, "Executing Tavily tool");

        match tool_name {
            "web_search" => {
                let args: WebSearchArgs = serde_json::from_str(arguments)?;
                let max_results = args.max_results.clamp(1, 10);

                debug!(query = %args.query, max_results = max_results, "Tavily web search");

                let request = tavily::SearchRequest::new(&self.api_key, &args.query)
                    .max_results(i32::from(max_results))
                    .search_depth("basic");

                match self.client.call(&request).await {
                    Ok(response) => {
                        let mut output = format!("## Результаты поиска: {}\n\n", args.query);

                        if response.results.is_empty() {
                            output.push_str("Ничего не найдено по данному запросу.\n");
                        } else {
                            for (i, result) in response.results.iter().enumerate() {
                                let _ = write!(
                                    output,
                                    "### {}. {}\n**URL**: {}\n\n{}\n\n---\n\n",
                                    i + 1,
                                    html_escape::encode_text(&result.title),
                                    result.url,
                                    html_escape::encode_text(&result.content)
                                );
                            }
                        }

                        Ok(output)
                    }
                    Err(e) => Ok(format!("Ошибка поиска: {e}")),
                }
            }
            "web_extract" => {
                let args: WebExtractArgs = serde_json::from_str(arguments)?;

                // Limit to 5 URLs
                let urls: Vec<&str> = args.urls.iter().take(5).map(String::as_str).collect();

                debug!(urls = ?urls, "Tavily extract");

                match self.client.extract(urls).await {
                    Ok(response) => {
                        let mut output = String::new();

                        if response.results.is_empty() {
                            output.push_str("Не удалось извлечь контент из указанных URL.\n");
                        } else {
                            for result in response.results {
                                let _ = write!(
                                    output,
                                    "## {}\n\n{}\n\n---\n\n",
                                    result.url,
                                    html_escape::encode_text(&result.raw_content)
                                );
                            }
                        }

                        Ok(output)
                    }
                    Err(e) => Ok(format!("Ошибка извлечения контента: {e}")),
                }
            }
            _ => anyhow::bail!("Unknown Tavily tool: {tool_name}"),
        }
    }
}
