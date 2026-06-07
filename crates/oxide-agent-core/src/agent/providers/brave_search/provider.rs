use super::client::BraveSearchClient;
use super::error::BraveSearchError;
use super::format::{format_search_failure, format_search_results};
use super::types::{BraveSearchArgs, NormalizedBraveSearchArgs, TOOL_NAME};
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::config::{
    get_brave_search_api_key, get_brave_search_country, get_brave_search_lang,
    get_brave_search_max_concurrent, get_brave_search_min_delay_ms, get_brave_search_safesearch,
    get_brave_search_timeout, get_brave_search_ui_lang,
};
use crate::llm::ToolDefinition;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error};

#[derive(Debug, Clone)]
/// Tool provider skeleton for Brave Web Search.
pub struct BraveSearchProvider {
    client: BraveSearchClient,
    default_country: String,
    default_search_lang: String,
    default_ui_lang: String,
    default_safesearch: String,
}

/// Brave Search provider construction defaults.
#[derive(Debug, Clone)]
pub struct BraveSearchProviderConfig {
    /// HTTP request timeout.
    pub timeout: Duration,
    /// Default Brave `country` query parameter.
    pub default_country: String,
    /// Default Brave `search_lang` query parameter.
    pub default_search_lang: String,
    /// Default Brave `ui_lang` query parameter.
    pub default_ui_lang: String,
    /// Default Brave `safesearch` query parameter.
    pub default_safesearch: String,
    /// Maximum concurrent Brave requests.
    pub max_concurrent: usize,
    /// Minimum delay between Brave request starts.
    pub min_delay: Duration,
}

impl BraveSearchProvider {
    /// Create a provider from global configuration.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when the Brave API key is not configured.
    pub fn new_from_config() -> Result<Self, BraveSearchError> {
        Self::new(
            get_brave_search_api_key().unwrap_or_default(),
            config_from_env(),
        )
    }

    /// Create a provider with explicit defaults.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when `api_key` is empty.
    pub fn new(
        api_key: impl Into<String>,
        config: BraveSearchProviderConfig,
    ) -> Result<Self, BraveSearchError> {
        Ok(Self {
            client: BraveSearchClient::new(
                api_key,
                config.timeout,
                config.max_concurrent,
                config.min_delay,
            )?,
            default_country: config.default_country,
            default_search_lang: config.default_search_lang,
            default_ui_lang: config.default_ui_lang,
            default_safesearch: config.default_safesearch,
        })
    }

    /// Return the underlying client skeleton.
    #[must_use]
    pub const fn client(&self) -> &BraveSearchClient {
        &self.client
    }

    /// Return the default Brave `country` query parameter.
    #[must_use]
    pub fn default_country(&self) -> &str {
        &self.default_country
    }

    /// Return the default Brave `search_lang` query parameter.
    #[must_use]
    pub fn default_search_lang(&self) -> &str {
        &self.default_search_lang
    }

    /// Return the default Brave `ui_lang` query parameter.
    #[must_use]
    pub fn default_ui_lang(&self) -> &str {
        &self.default_ui_lang
    }

    /// Return the default Brave `safesearch` query parameter.
    #[must_use]
    pub fn default_safesearch(&self) -> &str {
        &self.default_safesearch
    }

    /// Build native typed runtime executors for Brave Search tools.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(BraveSearchToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_NAME.to_string(),
            description: concat!(
                "Search the public web using Brave Search API. Use this to discover URLs and snippets. ",
                "Open only selected result URLs with crawl4ai_markdown; do not crawl every result. ",
                "If Brave is unavailable, use searxng_search as fallback."
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
                    "country": {
                        "type": "string",
                        "description": "Optional Brave country code, for example 'US'"
                    },
                    "search_lang": {
                        "type": "string",
                        "description": "Optional Brave search language, for example 'en'"
                    },
                    "ui_lang": {
                        "type": "string",
                        "description": "Optional Brave UI language, for example 'en-US'"
                    },
                    "freshness": {
                        "type": "string",
                        "description": "Optional recency filter: pd, pw, pm, py, or a Brave custom date range string"
                    },
                    "safesearch": {
                        "type": "string",
                        "enum": ["off", "moderate", "strict"],
                        "description": "Safe search level (default from configuration)"
                    },
                    "extra_snippets": {
                        "type": "boolean",
                        "description": "Whether to request additional Brave snippets (default: false)"
                    },
                    "page": {
                        "type": "integer",
                        "description": "Result page number starting from 1 (1-10, default: 1)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &str,
    ) -> Result<BraveSearchToolResult> {
        debug!(tool = tool_name, "Executing Brave Search tool");

        match tool_name {
            TOOL_NAME => {
                let args: BraveSearchArgs = serde_json::from_str(arguments)?;
                let normalized = match args.normalized(&self.default_safesearch) {
                    Ok(normalized) => self.apply_defaults(normalized),
                    Err(error) => {
                        let (markdown, payload) = format_search_failure(&args.query, &error);
                        return Ok(BraveSearchToolResult {
                            markdown,
                            payload,
                            success: false,
                        });
                    }
                };

                debug!(
                    query = %normalized.query,
                    max_results = normalized.max_results,
                    offset = normalized.offset,
                    country = ?normalized.country,
                    search_lang = ?normalized.search_lang,
                    freshness = ?normalized.freshness,
                    safesearch = %normalized.safesearch,
                    "Brave Search request"
                );

                match self.client.search(&normalized).await {
                    Ok(response) => {
                        let (markdown, payload) = format_search_results(&normalized, &response);
                        Ok(BraveSearchToolResult {
                            markdown,
                            payload,
                            success: true,
                        })
                    }
                    Err(error) => {
                        error!(query = %normalized.query, error = %error, "Brave Search failed");
                        let (markdown, payload) = format_search_failure(&normalized.query, &error);
                        Ok(BraveSearchToolResult {
                            markdown,
                            payload,
                            success: false,
                        })
                    }
                }
            }
            _ => anyhow::bail!("Unknown Brave Search tool: {tool_name}"),
        }
    }

    fn apply_defaults(&self, mut args: NormalizedBraveSearchArgs) -> NormalizedBraveSearchArgs {
        if args.country.is_none() {
            args.country = non_empty_default(&self.default_country);
        }
        if args.search_lang.is_none() {
            args.search_lang = non_empty_default(&self.default_search_lang);
        }
        if args.ui_lang.is_none() {
            args.ui_lang = non_empty_default(&self.default_ui_lang);
        }
        args
    }
}

struct BraveSearchToolResult {
    markdown: String,
    payload: Value,
    success: bool,
}

struct BraveSearchToolExecutor {
    provider: Arc<BraveSearchProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for BraveSearchToolExecutor {
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

fn non_empty_default(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn config_from_env() -> BraveSearchProviderConfig {
    BraveSearchProviderConfig {
        timeout: Duration::from_secs(get_brave_search_timeout()),
        default_country: get_brave_search_country(),
        default_search_lang: get_brave_search_lang(),
        default_ui_lang: get_brave_search_ui_lang(),
        default_safesearch: get_brave_search_safesearch(),
        max_concurrent: get_brave_search_max_concurrent(),
        min_delay: Duration::from_millis(get_brave_search_min_delay_ms()),
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

    #[test]
    fn provider_constructs_with_explicit_config() {
        let provider = BraveSearchProvider::new("test-key", test_config()).expect("provider");

        assert_eq!(provider.default_country(), "US");
        assert_eq!(provider.default_search_lang(), "en");
        assert_eq!(provider.default_ui_lang(), "en-US");
        assert_eq!(provider.default_safesearch(), "moderate");
        assert_eq!(provider.client().api_key(), "test-key");
    }

    #[test]
    fn typed_runtime_executors_register_only_brave_search() {
        let provider =
            Arc::new(BraveSearchProvider::new("test-key", test_config()).expect("provider"));
        let executors = provider.tool_runtime_executors();

        assert_eq!(executors.len(), 1);
        assert_eq!(executors[0].name().as_str(), TOOL_NAME);

        let spec = executors[0].spec();
        assert_eq!(spec.name, TOOL_NAME);
        assert!(spec.description.contains("Brave Search API"));
        assert_eq!(spec.parameters["required"][0], "query");
        assert!(
            spec.parameters["properties"]
                .get("extra_snippets")
                .is_some()
        );
        assert!(spec.parameters["properties"].get("page").is_some());
    }

    #[test]
    fn apply_defaults_fills_configured_targeting() {
        let provider = BraveSearchProvider::new("test-key", test_config()).expect("provider");
        let normalized = BraveSearchArgs {
            query: "rust".to_string(),
            max_results: 5,
            country: None,
            search_lang: None,
            ui_lang: None,
            freshness: None,
            safesearch: None,
            extra_snippets: false,
            page: 1,
        }
        .normalized(provider.default_safesearch())
        .expect("normalized args");

        let normalized = provider.apply_defaults(normalized);

        assert_eq!(normalized.country.as_deref(), Some("US"));
        assert_eq!(normalized.search_lang.as_deref(), Some("en"));
        assert_eq!(normalized.ui_lang.as_deref(), Some("en-US"));
        assert_eq!(normalized.safesearch, "moderate");
    }

    #[tokio::test]
    async fn typed_runtime_executor_reports_invalid_json_without_network() {
        let provider =
            Arc::new(BraveSearchProvider::new("test-key", test_config()).expect("provider"));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("brave_search executor registered");

        let error = executor
            .execute(runtime_invocation("{"))
            .await
            .expect_err("invalid JSON must fail before network call");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn typed_runtime_executor_returns_failure_payload_for_empty_query() {
        let provider =
            Arc::new(BraveSearchProvider::new("test-key", test_config()).expect("provider"));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("brave_search executor registered");

        let output = executor
            .execute(runtime_invocation(r#"{"query":"   "}"#))
            .await
            .expect("empty query is a tool-level failure");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        let payload = output.structured_payload.expect("structured payload");
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["error_kind"], "empty_query");
    }

    fn test_config() -> BraveSearchProviderConfig {
        BraveSearchProviderConfig {
            timeout: Duration::from_secs(1),
            default_country: "US".to_string(),
            default_search_lang: "en".to_string(),
            default_ui_lang: "en-US".to_string(),
            default_safesearch: "moderate".to_string(),
            max_concurrent: 1,
            min_delay: Duration::from_millis(0),
        }
    }

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-brave"),
            batch_id: ToolBatchId::from("batch-brave"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-brave"),
            tool_call_id: ToolCallId::from("call-brave"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(TOOL_NAME),
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
}
