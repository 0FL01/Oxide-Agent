use super::client::DuckDuckGoClient;
use super::error::DuckDuckGoError;
use super::format::{format_news_results, format_search_results};
use super::types::{
    DuckDuckGoNewsArgs, DuckDuckGoSearchArgs, TOOL_DUCKDUCKGO_NEWS, TOOL_DUCKDUCKGO_SEARCH,
};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::{debug, error};

/// Tool provider for DuckDuckGo web and news search.
pub struct DuckDuckGoProvider {
    client: DuckDuckGoClient,
}

impl DuckDuckGoProvider {
    /// Create a DuckDuckGo provider from environment configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying DuckDuckGo browser client cannot be initialized.
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: DuckDuckGoClient::from_config()?,
        })
    }

    /// Build native typed runtime executors for DuckDuckGo tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(DuckDuckGoToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![search_definition(), news_definition()]
    }

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<DuckDuckGoToolResult> {
        debug!(tool = tool_name, "Executing DuckDuckGo tool");

        match tool_name {
            TOOL_DUCKDUCKGO_SEARCH => {
                let args: DuckDuckGoSearchArgs = serde_json::from_str(arguments)?;
                let region = args.normalized_region().to_string();
                let max_results = args.normalized_max_results();
                debug!(query = %args.query, max_results, region = %region, "DuckDuckGo search");

                match self.client.lite_search(&args).await {
                    Ok(results) => {
                        let (markdown, payload) =
                            format_search_results(&args.query, &region, &results, max_results);
                        Ok(DuckDuckGoToolResult {
                            markdown,
                            payload,
                            success: true,
                        })
                    }
                    Err(error) => {
                        error!(query = %args.query, error = %error, "DuckDuckGo search failed");
                        Ok(error_tool_result("search", &args.query, &region, &error))
                    }
                }
            }
            TOOL_DUCKDUCKGO_NEWS => {
                let args: DuckDuckGoNewsArgs = serde_json::from_str(arguments)?;
                let region = args.normalized_region().to_string();
                let max_results = args.normalized_max_results();
                debug!(
                    query = %args.query,
                    max_results,
                    region = %region,
                    safe_search = args.safe_search,
                    "DuckDuckGo news"
                );

                match self.client.news(&args).await {
                    Ok(results) => {
                        let (markdown, payload) =
                            format_news_results(&args.query, &region, &results, max_results);
                        Ok(DuckDuckGoToolResult {
                            markdown,
                            payload,
                            success: true,
                        })
                    }
                    Err(error) => {
                        error!(query = %args.query, error = %error, "DuckDuckGo news failed");
                        Ok(error_tool_result("news", &args.query, &region, &error))
                    }
                }
            }
            _ => anyhow::bail!("Unknown DuckDuckGo tool: {tool_name}"),
        }
    }
}

struct DuckDuckGoToolResult {
    markdown: String,
    payload: Value,
    success: bool,
}

fn error_tool_result(
    kind: &'static str,
    query: &str,
    region: &str,
    error: &DuckDuckGoError,
) -> DuckDuckGoToolResult {
    DuckDuckGoToolResult {
        markdown: error.agent_message(),
        payload: json!({
            "provider": "duckduckgo",
            "kind": kind,
            "query": query,
            "region": region,
            "error_kind": error.code(),
            "error": error.to_string(),
            "provider_unavailable": matches!(
                error,
                DuckDuckGoError::Blocked(_) | DuckDuckGoError::RateLimited
            ),
            "results": [],
        }),
        success: false,
    }
}

fn search_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_DUCKDUCKGO_SEARCH.to_string(),
        description: concat!(
            "Search public web using DuckDuckGo HTML/Lite. Use this to discover URLs. ",
            "Use web_markdown to fetch selected result pages; do not fetch every result automatically."
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
                "region": {
                    "type": "string",
                    "description": "DuckDuckGo region code, for example 'wt-wt' or 'us-en' (default: wt-wt)"
                }
            },
            "required": ["query"]
        }),
    }
}

fn news_definition() -> ToolDefinition {
    ToolDefinition {
        name: TOOL_DUCKDUCKGO_NEWS.to_string(),
        description: concat!(
            "Search recent news using DuckDuckGo News. Returns article title, source, date, URL, and excerpt. ",
            "Use web_markdown to fetch selected full articles."
        )
        .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "News search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of news results to return (1-10, default: 5)"
                },
                "region": {
                    "type": "string",
                    "description": "DuckDuckGo region code, for example 'wt-wt' or 'us-en' (default: wt-wt)"
                },
                "safe_search": {
                    "type": "boolean",
                    "description": "Whether DuckDuckGo safe search is enabled (default: true)"
                }
            },
            "required": ["query"]
        }),
    }
}

struct DuckDuckGoToolExecutor {
    provider: Arc<DuckDuckGoProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for DuckDuckGoToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(self.name.as_str(), &invocation.raw_arguments)
            .await
            .map(|result| {
                let mut output = if result.success {
                    normalizer.success(&invocation, &result.markdown, "")
                } else {
                    normalizer.failure(&invocation, result.markdown)
                };
                output.structured_payload = Some(result.payload);
                output
            })
            .map_err(search_runtime_error)
    }
}

fn search_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    if error.downcast_ref::<serde_json::Error>().is_some() {
        ToolRuntimeError::InvalidArguments(error.to_string())
    } else {
        ToolRuntimeError::Failure(error.to_string())
    }
}
