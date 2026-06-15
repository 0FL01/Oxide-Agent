use super::client::CrwClient;
use super::format::format_search_results;
use super::types::{CrwSearchArgs, TOOL_WEB_SEARCH};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error};

#[derive(Debug, Clone)]
/// Tool provider for CRW-backed web search and scrape.
pub struct CrwProvider {
    client: CrwClient,
}

impl CrwProvider {
    /// Create a provider using config defaults.
    pub fn new() -> Result<Self> {
        Self::new_with_timeout(Duration::from_secs(crate::config::get_crw_timeout_secs()))
    }

    /// Create a provider with an explicit HTTP timeout.
    pub fn new_with_timeout(timeout: Duration) -> Result<Self> {
        Self::new_full(
            &crate::config::get_crw_base_url(),
            timeout,
            crate::config::get_crw_api_token(),
        )
    }

    /// Create a provider with full configuration.
    pub fn new_full(base_url: &str, timeout: Duration, api_token: Option<String>) -> Result<Self> {
        Ok(Self {
            client: CrwClient::new(base_url, timeout, api_token)?,
        })
    }

    /// Direct access to the scrape client (used by `web_crawler` fallback).
    pub fn client(&self) -> &CrwClient {
        &self.client
    }

    /// Build native typed runtime executors for CRW tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(CrwSearchToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_WEB_SEARCH.to_string(),
            description: concat!(
                "Search the public web for current information, facts, documentation leads, and URLs. ",
                "Returns titles, URLs, and content snippets."
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
                        "description": "Maximum number of results to return (1-10, default: 5)"
                    },
                    "language": {
                        "type": "string",
                        "description": "Preferred search language code, for example 'en', 'ru', or 'all'"
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
                        "description": "Optional search categories (string or array)"
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

    async fn execute_search(&self, arguments: &str) -> Result<String> {
        debug!("Executing CRW web_search");

        let args: CrwSearchArgs = match serde_json::from_str(arguments) {
            Ok(args) => args,
            Err(_) => return Ok("Invalid search arguments".to_string()),
        };

        debug!(
            query = %args.query,
            max_results = args.normalized_max_results(),
            "CRW web_search"
        );

        match self.client.search(&args).await {
            Ok(response) => Ok(format_search_results(
                &args.query,
                &response,
                args.normalized_max_results(),
            )),
            Err(err) => {
                error!(
                    query = %args.query,
                    error = %err,
                    "CRW web_search failed after retries"
                );
                Ok(err.agent_message())
            }
        }
    }
}

struct CrwSearchToolExecutor {
    provider: Arc<CrwProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for CrwSearchToolExecutor {
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
            .execute_search(&invocation.raw_arguments)
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(|err| ToolRuntimeError::Failure(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definition_has_web_search_name() {
        let spec = CrwProvider::tool_definition();
        assert_eq!(spec.name, "web_search");
    }

    #[test]
    fn tool_definition_requires_query() {
        let spec = CrwProvider::tool_definition();
        let required = spec.parameters["required"]
            .as_array()
            .expect("required array");
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "query");
    }
}
