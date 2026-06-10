use super::backoff::MAX_RETRIES;
use super::client::SearxngClient;
use super::format::format_search_results;
use super::types::{SearxngSearchArgs, TOOL_NAME};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::config::{get_searxng_bearer_token, get_searxng_rotation_engines, get_searxng_timeout};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;
use tracing::error;

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
        Self::new_with_timeout_and_bearer_token(base_url, timeout, get_searxng_bearer_token())
    }

    /// Create a provider with an explicit HTTP timeout and optional Bearer token.
    pub fn new_with_timeout_and_bearer_token(
        base_url: &str,
        timeout: Duration,
        bearer_token: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            client: SearxngClient::new(
                base_url,
                timeout,
                get_searxng_rotation_engines(),
                bearer_token,
            )?,
        })
    }

    /// Build native typed runtime executors for SearXNG tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(SearxngToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
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
                        "enum": ["day", "week", "month", "year"],
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
        }
    }

    async fn execute_tool(&self, tool_name: &str, arguments: &str) -> Result<SearxngToolResult> {
        debug!(tool = tool_name, "Executing SearXNG tool");

        match tool_name {
            TOOL_NAME => {
                let args: SearxngSearchArgs = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Ok(SearxngToolResult::failure(
                            "Invalid search arguments".to_string(),
                            json!({
                                "provider": TOOL_NAME,
                                "kind": "search",
                                "query": Value::Null,
                                "error_kind": "invalid_arguments",
                                "message": format!("invalid SearXNG search arguments: {error}"),
                                "provider_unavailable": false,
                                "retryable": false,
                                "results": [],
                                "snippet_only": true,
                            }),
                        ));
                    }
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
                    Ok(response) => {
                        let (markdown, payload) = format_search_results(
                            &args.query,
                            &response,
                            args.normalized_max_results(),
                        );
                        Ok(SearxngToolResult::success(markdown, payload))
                    }
                    Err(error) => {
                        error!(
                            query = %args.query,
                            error = %error,
                            "SearXNG search failed after {} attempts",
                            MAX_RETRIES + 1,
                        );
                        Ok(SearxngToolResult::failure(
                            error.agent_message(),
                            error.failure_payload(&args.query),
                        ))
                    }
                }
            }
            _ => anyhow::bail!("Unknown SearXNG tool: {tool_name}"),
        }
    }
}

struct SearxngToolResult {
    markdown: String,
    payload: Value,
    success: bool,
}

impl SearxngToolResult {
    fn success(markdown: String, payload: Value) -> Self {
        Self {
            markdown,
            payload,
            success: true,
        }
    }

    fn failure(markdown: String, payload: Value) -> Self {
        Self {
            markdown,
            payload,
            success: false,
        }
    }
}

struct SearxngToolExecutor {
    provider: Arc<SearxngProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for SearxngToolExecutor {
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
                    normalizer
                        .failure(&invocation, &result.markdown)
                        .with_streams(
                            normalizer.stdout_preview(&result.markdown),
                            normalizer.stderr_preview(""),
                        )
                };
                output.structured_payload = Some(result.payload);
                output
            })
            .map_err(|error| ToolRuntimeError::Failure(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolRuntimeConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let config = ToolRuntimeConfig::default();
        ToolInvocation {
            session_id: SessionId::from(1),
            turn_id: TurnId::from("turn-searxng"),
            batch_id: ToolBatchId::from("batch-searxng"),
            batch_index: 0,
            invocation_id: InvocationId::new("invoke-searxng"),
            tool_call_id: ToolCallId::from("call-searxng"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(TOOL_NAME),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::from_str(raw_arguments).unwrap_or_else(|_| json!({})),
            cancellation_token: CancellationToken::new(),
            timeout: config.timeout,
            execution_context: ToolExecutionContext::new(config.artifact_dir.clone()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
        }
    }

    #[tokio::test]
    async fn empty_query_returns_structured_failure_status() {
        let provider = Arc::new(
            SearxngProvider::new_with_timeout("http://127.0.0.1:9", Duration::from_secs(1))
                .expect("provider"),
        );
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("executor");

        let output = executor
            .execute(runtime_invocation(r#"{"query":"   "}"#))
            .await
            .expect("typed searxng output");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert!(!output.success);
        assert!(
            output
                .stdout
                .text
                .as_deref()
                .unwrap_or_default()
                .contains("Search query cannot be empty")
        );
        let payload = output
            .structured_payload
            .expect("structured failure payload");
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["error_kind"], "empty_query");
        assert_eq!(payload["retryable"], false);
        assert_eq!(payload["results"], json!([]));
        assert!(payload["fetched_at"].is_string());
    }
}
