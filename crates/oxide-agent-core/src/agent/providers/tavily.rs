//! Tavily Provider - web search and content extraction
//!
//! Provides `web_search` and `web_extract` tools using native Tavily Rust SDK.

use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tavily::Tavily;
use tracing::debug;

const TOOL_WEB_SEARCH: &str = "web_search";
const TOOL_WEB_EXTRACT: &str = "web_extract";

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

    /// Build native typed runtime executors for Tavily tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(TavilyToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_WEB_SEARCH.to_string(),
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
                name: TOOL_WEB_EXTRACT.to_string(),
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

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<String> {
        use std::fmt::Write;
        debug!(tool = tool_name, "Executing Tavily tool");

        match tool_name {
            TOOL_WEB_SEARCH => {
                let args: WebSearchArgs = serde_json::from_str(arguments)?;
                let max_results = args.max_results.clamp(1, 10);

                debug!(query = %args.query, max_results = max_results, "Tavily web search");

                let request = tavily::SearchRequest::new(&self.api_key, &args.query)
                    .max_results(i32::from(max_results))
                    .search_depth("basic");

                match self.client.call(&request).await {
                    Ok(response) => {
                        let mut output = format!("## Search results for: {}\n\n", args.query);

                        if response.results.is_empty() {
                            output.push_str("No results found for this query.\n");
                        } else {
                            for (i, result) in response.results.iter().enumerate() {
                                let _ = write!(
                                    output,
                                    "### {}. {}\n**URL**: {}\n\n{}\n\n---\n\n",
                                    i + 1,
                                    crate::utils::clean_html(&result.title),
                                    result.url,
                                    crate::utils::clean_html(&result.content)
                                );
                            }
                        }

                        Ok(output)
                    }
                    Err(e) => Ok(format!("Search error: {e}")),
                }
            }
            TOOL_WEB_EXTRACT => {
                let args: WebExtractArgs = serde_json::from_str(arguments)?;

                // Limit to 5 URLs.
                let urls: Vec<&str> = args.urls.iter().take(5).map(String::as_str).collect();

                debug!(urls = ?urls, "Tavily extract");

                match self.client.extract(urls).await {
                    Ok(response) => {
                        let mut output = String::new();

                        if response.results.is_empty() {
                            output.push_str("Failed to extract content from the specified URLs.\n");
                        } else {
                            for result in response.results {
                                let _ = write!(
                                    output,
                                    "## {}\n\n{}\n\n---\n\n",
                                    result.url,
                                    crate::utils::clean_html(&result.raw_content)
                                );
                            }
                        }

                        Ok(output)
                    }
                    Err(e) => Ok(format!("Content extraction error: {e}")),
                }
            }
            _ => anyhow::bail!("Unknown Tavily tool: {tool_name}"),
        }
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
        Self::tool_definitions()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(tool_name, TOOL_WEB_SEARCH | TOOL_WEB_EXTRACT)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        _cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        self.execute_tool(tool_name, arguments).await
    }
}

struct TavilyToolExecutor {
    provider: Arc<TavilyProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for TavilyToolExecutor {
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
            .map(|output| normalizer.success(&invocation, &output, ""))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-tavily"),
            batch_id: ToolBatchId::from("batch-tavily"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[test]
    fn typed_runtime_executors_register_search_and_extract() {
        let provider = Arc::new(TavilyProvider::new("dummy-key").expect("provider constructs"));
        let names = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().as_str().to_string())
            .collect::<Vec<_>>();

        assert_eq!(names, vec![TOOL_WEB_SEARCH, TOOL_WEB_EXTRACT]);
    }

    #[tokio::test]
    async fn typed_runtime_executor_reports_invalid_arguments_without_network() {
        let provider = Arc::new(TavilyProvider::new("dummy-key").expect("provider constructs"));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WEB_SEARCH)
            .expect("web_search executor registered");

        let error = executor
            .execute(runtime_invocation(TOOL_WEB_SEARCH, r#"{"max_results":3}"#))
            .await
            .expect_err("missing query must fail before network call");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));

        let output = OutputNormalizer::new(ToolRuntimeConfig::default()).executor_error(
            &runtime_invocation(TOOL_WEB_SEARCH, r#"{"max_results":3}"#),
            error,
        );
        assert_eq!(output.status, ToolOutputStatus::InvalidArguments);
    }
}
