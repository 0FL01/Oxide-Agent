use super::client::SearxngClient;
use super::format::format_search_results;
use super::types::{SearxngSearchArgs, TOOL_NAME};
use crate::agent::provider::ToolProvider;
use crate::config::get_searxng_timeout;
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
use tracing::debug;

#[derive(Debug, Clone)]
/// Tool provider for self-hosted SearXNG web search.
pub struct SearxngProvider {
    client: SearxngClient,
}

impl SearxngProvider {
    /// Create a provider using the configured default timeout.
    pub fn new(base_url: &str) -> Result<Self> {
        Self::new_with_timeout(base_url, Duration::from_secs(get_searxng_timeout()))
    }

    /// Create a provider with an explicit HTTP timeout.
    pub fn new_with_timeout(base_url: &str, timeout: Duration) -> Result<Self> {
        Ok(Self {
            client: SearxngClient::new(base_url, timeout)?,
        })
    }
}

#[async_trait]
impl ToolProvider for SearxngProvider {
    fn name(&self) -> &'static str {
        "searxng"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: TOOL_NAME.to_string(),
            description: concat!(
                "Search the public web using a self-hosted SearXNG instance. ",
                "Best for fast web discovery, current facts, documentation leads, and finding URLs before deeper crawling."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of search results to return (1-10, default: 5)"
                    },
                    "language": {
                        "type": "string",
                        "description": "Preferred search language, for example 'en', 'ru', or 'all'"
                    },
                    "time_range": {
                        "type": "string",
                        "enum": ["day", "month", "year"],
                        "description": "Optional recency filter"
                    },
                    "safe_search": {
                        "type": "integer",
                        "enum": [0, 1, 2],
                        "description": "Safe search level: 0 off, 1 moderate, 2 strict"
                    },
                    "categories": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional SearXNG categories such as general, news, images, science"
                    },
                    "engines": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Optional search engines to restrict the query to"
                    },
                    "page": {
                        "type": "integer",
                        "description": "Result page number starting from 1"
                    }
                },
                "required": ["query"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == TOOL_NAME
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        debug!(tool = tool_name, "Executing SearXNG tool");

        match tool_name {
            TOOL_NAME => {
                let args: SearxngSearchArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => return Ok(format!("Invalid arguments: {error}")),
                };

                debug!(
                    query = %args.query,
                    max_results = args.normalized_max_results(),
                    language = ?args.language,
                    time_range = ?args.time_range,
                    safe_search = ?args.normalized_safe_search(),
                    "SearXNG search"
                );

                match self.client.search(&args).await {
                    Ok(response) => Ok(format_search_results(
                        &args.query,
                        &response,
                        args.normalized_max_results(),
                    )),
                    Err(error) => Ok(format!("SearXNG search error: {error}")),
                }
            }
            _ => anyhow::bail!("Unknown SearXNG tool: {tool_name}"),
        }
    }
}
