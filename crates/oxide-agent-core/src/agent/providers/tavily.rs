//! Tavily Provider - web search and content extraction
//!
//! Provides `web_search` and `web_extract` tools using Tavily's HTTP API.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

const TOOL_WEB_SEARCH: &str = "web_search";
const TOOL_WEB_EXTRACT: &str = "web_extract";
const TAVILY_API_BASE: &str = "https://api.tavily.com";

/// Provider for Tavily web search tools
pub struct TavilyProvider {
    client: reqwest::Client,
    api_key: String,
}

impl TavilyProvider {
    /// Create a new Tavily provider with the given API key
    ///
    /// # Errors
    ///
    /// Returns an error if the Tavily HTTP client cannot be created.
    pub fn new(api_key: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create Tavily HTTP client: {e}"))?;

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

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<TavilyToolResult> {
        use std::fmt::Write;
        debug!(tool = tool_name, "Executing Tavily tool");

        match tool_name {
            TOOL_WEB_SEARCH => {
                let args: WebSearchArgs = serde_json::from_str(arguments)?;
                if args.query.trim().is_empty() {
                    return Ok(tavily_failure_result(
                        TOOL_WEB_SEARCH,
                        "search",
                        Some(&args.query),
                        None,
                        "empty_query",
                        "Search query cannot be empty",
                    ));
                }
                let max_results = args.max_results.clamp(1, 10);

                debug!(query = %args.query, max_results = max_results, "Tavily web search");

                match self.search(&args.query, max_results).await {
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

                        Ok(tavily_search_success_result(
                            &args.query,
                            max_results,
                            &response,
                            output,
                        ))
                    }
                    Err(e) => Ok(tavily_failure_result(
                        TOOL_WEB_SEARCH,
                        "search",
                        Some(&args.query),
                        None,
                        tavily_error_kind(&e),
                        format!("Search error: {e}"),
                    )),
                }
            }
            TOOL_WEB_EXTRACT => {
                let args: WebExtractArgs = serde_json::from_str(arguments)?;
                if args.urls.is_empty() {
                    return Ok(tavily_failure_result(
                        TOOL_WEB_EXTRACT,
                        "fetch",
                        None,
                        None,
                        "empty_urls",
                        "Content extraction requires at least one URL",
                    ));
                }

                // Limit to 5 URLs.
                let urls: Vec<&str> = args.urls.iter().take(5).map(String::as_str).collect();

                debug!(urls = ?urls, "Tavily extract");

                match self.extract(&urls).await {
                    Ok(response) => {
                        let mut output = String::new();

                        if response.results.is_empty() {
                            Ok(tavily_failure_result(
                                TOOL_WEB_EXTRACT,
                                "fetch",
                                None,
                                args.urls.first().map(String::as_str),
                                "empty_results",
                                "Failed to extract content from the specified URLs.",
                            ))
                        } else {
                            for result in &response.results {
                                let _ = write!(
                                    output,
                                    "## {}\n\n{}\n\n---\n\n",
                                    result.url,
                                    crate::utils::clean_html(&result.raw_content)
                                );
                            }
                            Ok(tavily_extract_success_result(&args.urls, &response, output))
                        }
                    }
                    Err(e) => Ok(tavily_failure_result(
                        TOOL_WEB_EXTRACT,
                        "fetch",
                        None,
                        args.urls.first().map(String::as_str),
                        tavily_error_kind(&e),
                        format!("Content extraction error: {e}"),
                    )),
                }
            }
            _ => anyhow::bail!("Unknown Tavily tool: {tool_name}"),
        }
    }

    async fn search(&self, query: &str, max_results: u8) -> Result<TavilySearchResponse> {
        let request = TavilySearchRequest {
            api_key: &self.api_key,
            query,
            search_depth: "basic",
            max_results,
        };
        self.post_json("search", &request).await
    }

    async fn extract(&self, urls: &[&str]) -> Result<TavilyExtractResponse> {
        let request = TavilyExtractRequest {
            api_key: &self.api_key,
            urls,
        };
        self.post_json("extract", &request).await
    }

    async fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R>
    where
        T: Serialize + ?Sized,
        R: for<'de> Deserialize<'de>,
    {
        let url = format!("{TAVILY_API_BASE}/{path}");
        let response = self.client.post(url).json(body).send().await?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            anyhow::bail!("Tavily HTTP {status}: {}", truncate_for_error(body));
        }

        Ok(response.json::<R>().await?)
    }
}

struct TavilyToolResult {
    markdown: String,
    payload: Value,
    success: bool,
}

fn tavily_search_success_result(
    query: &str,
    max_results: u8,
    response: &TavilySearchResponse,
    markdown: String,
) -> TavilyToolResult {
    TavilyToolResult {
        markdown,
        payload: json!({
            "provider": "tavily",
            "tool": TOOL_WEB_SEARCH,
            "kind": "search",
            "query": query.trim(),
            "max_results": max_results,
            "results": response
                .results
                .iter()
                .enumerate()
                .map(|(index, result)| json!({
                    "rank": index + 1,
                    "title": result.title.as_str(),
                    "url": result.url.as_str(),
                    "snippet": result.content.as_str(),
                    "content": result.content.as_str(),
                }))
                .collect::<Vec<_>>(),
            "snippet_only": true,
            "fetched_at": Utc::now().to_rfc3339(),
        }),
        success: true,
    }
}

fn tavily_extract_success_result(
    urls: &[String],
    response: &TavilyExtractResponse,
    markdown: String,
) -> TavilyToolResult {
    let first_url = response
        .results
        .first()
        .map(|result| result.url.as_str())
        .or_else(|| urls.first().map(String::as_str));
    TavilyToolResult {
        markdown,
        payload: json!({
            "provider": "tavily",
            "tool": TOOL_WEB_EXTRACT,
            "kind": "fetch",
            "url": first_url,
            "final_url": first_url,
            "urls": urls,
            "results": response
                .results
                .iter()
                .map(|result| json!({
                    "url": result.url.as_str(),
                    "final_url": result.url.as_str(),
                    "raw_content": result.raw_content.as_str(),
                    "content": result.raw_content.as_str(),
                }))
                .collect::<Vec<_>>(),
            "snippet_only": false,
            "fetched_at": Utc::now().to_rfc3339(),
        }),
        success: true,
    }
}

fn tavily_failure_result(
    tool_name: &str,
    kind: &str,
    query: Option<&str>,
    url: Option<&str>,
    error_kind: &str,
    message: impl Into<String>,
) -> TavilyToolResult {
    let message = message.into();
    TavilyToolResult {
        markdown: message.clone(),
        payload: json!({
            "provider": "tavily",
            "tool": tool_name,
            "kind": kind,
            "query": query.map(str::trim),
            "url": url,
            "error_kind": error_kind,
            "error": message,
            "provider_unavailable": tavily_provider_unavailable(error_kind),
            "retryable": tavily_retryable(error_kind),
            "fallback": "searxng_search",
            "results": [],
            "fetched_at": Utc::now().to_rfc3339(),
        }),
        success: false,
    }
}

fn tavily_error_kind(error: &anyhow::Error) -> &'static str {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("timeout") {
        "timeout"
    } else if message.contains("401") || message.contains("403") || message.contains("api key") {
        "auth"
    } else if message.contains("429") || message.contains("rate") {
        "rate_limited"
    } else if message.contains("http") {
        "http_status"
    } else if message.contains("decode") || message.contains("json") {
        "invalid_response"
    } else {
        "request"
    }
}

fn tavily_provider_unavailable(error_kind: &str) -> bool {
    matches!(
        error_kind,
        "auth" | "rate_limited" | "timeout" | "http_status"
    )
}

fn tavily_retryable(error_kind: &str) -> bool {
    matches!(
        error_kind,
        "timeout" | "rate_limited" | "http_status" | "request"
    )
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

#[derive(Debug, Serialize)]
struct TavilySearchRequest<'a> {
    api_key: &'a str,
    query: &'a str,
    search_depth: &'a str,
    max_results: u8,
}

#[derive(Debug, Deserialize)]
struct TavilySearchResponse {
    #[serde(default)]
    results: Vec<TavilySearchResult>,
}

#[derive(Debug, Deserialize)]
struct TavilySearchResult {
    #[serde(default)]
    title: String,
    url: String,
    #[serde(default)]
    content: String,
}

#[derive(Debug, Serialize)]
struct TavilyExtractRequest<'a> {
    api_key: &'a str,
    urls: &'a [&'a str],
}

#[derive(Debug, Deserialize)]
struct TavilyExtractResponse {
    #[serde(default)]
    results: Vec<TavilyExtractResult>,
}

#[derive(Debug, Deserialize)]
struct TavilyExtractResult {
    url: String,
    #[serde(default)]
    raw_content: String,
}

fn truncate_for_error(body: String) -> String {
    const LIMIT: usize = 500;
    if body.chars().count() <= LIMIT {
        body
    } else {
        body.chars().take(LIMIT).collect::<String>()
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

    #[tokio::test]
    async fn web_search_empty_query_returns_structured_failure_without_network() {
        let provider = Arc::new(TavilyProvider::new("dummy-key").expect("provider constructs"));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WEB_SEARCH)
            .expect("web_search executor registered");

        let output = executor
            .execute(runtime_invocation(TOOL_WEB_SEARCH, r#"{"query":"   "}"#))
            .await
            .expect("empty query is a structured provider failure");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert!(!output.success);
        let payload = output.structured_payload.expect("failure payload");
        assert_eq!(payload["provider"], "tavily");
        assert_eq!(payload["tool"], TOOL_WEB_SEARCH);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["error_kind"], "empty_query");
        assert_eq!(payload["fallback"], "searxng_search");
        assert!(payload["results"].as_array().expect("array").is_empty());
    }

    #[tokio::test]
    async fn web_extract_empty_urls_returns_structured_failure_without_network() {
        let provider = Arc::new(TavilyProvider::new("dummy-key").expect("provider constructs"));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WEB_EXTRACT)
            .expect("web_extract executor registered");

        let output = executor
            .execute(runtime_invocation(TOOL_WEB_EXTRACT, r#"{"urls":[]}"#))
            .await
            .expect("empty urls is a structured provider failure");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        let payload = output.structured_payload.expect("failure payload");
        assert_eq!(payload["provider"], "tavily");
        assert_eq!(payload["tool"], TOOL_WEB_EXTRACT);
        assert_eq!(payload["kind"], "fetch");
        assert_eq!(payload["error_kind"], "empty_urls");
        assert_eq!(payload["retryable"], false);
    }
}
