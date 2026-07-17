//! Unified indexed web search provider.
//!
//! Exposes one LLM-facing `web_search` tool and keeps CRW, Tavily, and Brave
//! as private indexed-search backends selected by runtime configuration.

use crate::agent::tool_runtime::arguments::deserialize_unsigned;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::config::{
    get_brave_backend_api_key, get_brave_backend_country, get_brave_backend_lang,
    get_brave_backend_max_concurrent, get_brave_backend_min_delay_ms, get_brave_backend_safesearch,
    get_brave_backend_timeout, get_brave_backend_ui_lang, get_crw_api_token, get_crw_base_url,
    get_crw_timeout_secs, get_tavily_api_key,
};
use crate::llm::ToolDefinition;
use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore};
use url::Url;

const TOOL_WEB_SEARCH: &str = "web_search";
const TAVILY_API_BASE: &str = "https://api.tavily.com";
const BRAVE_WEB_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const BRAVE_SUBSCRIPTION_TOKEN_HEADER: &str = "X-Subscription-Token";
const DEFAULT_MAX_RESULTS: u8 = 5;
const MAX_RESULTS_LIMIT: u8 = 10;
const MAX_OUTPUT_CHARS: usize = 20_000;
const CRW_MAX_RETRIES: usize = 3;
const BRAVE_MAX_RETRIES: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum WebSearchBackendKind {
    Crw,
    Tavily,
    Brave,
}

impl WebSearchBackendKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Crw => "crw",
            Self::Tavily => "tavily",
            Self::Brave => "brave",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "crw" => Some(Self::Crw),
            "tavily" => Some(Self::Tavily),
            "brave" => Some(Self::Brave),
            _ => None,
        }
    }
}

impl fmt::Display for WebSearchBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
struct WebSearchResult {
    title: String,
    url: String,
    snippet: String,
    source_provider: &'static str,
}

#[derive(Debug, Clone)]
struct NormalizedWebSearchRequest {
    query: String,
    provider: Option<WebSearchBackendKind>,
    max_results: u8,
    language: Option<String>,
    time_range: Option<String>,
    safe_search: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(
        default = "default_max_results",
        deserialize_with = "deserialize_unsigned"
    )]
    max_results: u8,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    time_range: Option<String>,
    #[serde(default)]
    safe_search: Option<String>,
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

impl WebSearchArgs {
    fn normalize(self) -> Result<NormalizedWebSearchRequest, WebSearchError> {
        let query = self.query.trim();
        if query.is_empty() {
            return Err(WebSearchError::InvalidArguments(
                "query must be a non-empty string".to_string(),
            ));
        }

        let provider = match normalize_optional_string(self.provider.as_deref()).as_deref() {
            None | Some("auto") => None,
            Some(value) => Some(WebSearchBackendKind::parse(value).ok_or_else(|| {
                WebSearchError::InvalidArguments(format!(
                    "invalid provider '{value}' (expected auto, crw, tavily, or brave)"
                ))
            })?),
        };

        let time_range = normalize_optional_string(self.time_range.as_deref())
            .map(|value| normalize_time_range(&value))
            .transpose()?;
        let safe_search = normalize_optional_string(self.safe_search.as_deref())
            .map(|value| normalize_safe_search(&value))
            .transpose()?;

        Ok(NormalizedWebSearchRequest {
            query: query.to_string(),
            provider,
            max_results: self.max_results.clamp(1, MAX_RESULTS_LIMIT),
            language: normalize_optional_string(self.language.as_deref()),
            time_range,
            safe_search,
        })
    }
}

fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_time_range(value: &str) -> Result<String, WebSearchError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "day" | "week" | "month" | "year" => Ok(value.trim().to_ascii_lowercase()),
        other => Err(WebSearchError::InvalidArguments(format!(
            "invalid time_range '{other}' (expected day, week, month, or year)"
        ))),
    }
}

fn normalize_safe_search(value: &str) -> Result<String, WebSearchError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "moderate" | "strict" => Ok(value.trim().to_ascii_lowercase()),
        other => Err(WebSearchError::InvalidArguments(format!(
            "invalid safe_search '{other}' (expected off, moderate, or strict)"
        ))),
    }
}

#[derive(Debug, Clone, Serialize)]
struct WebSearchAttempt {
    provider: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_unavailable: Option<bool>,
}

impl WebSearchAttempt {
    fn success(provider: WebSearchBackendKind) -> Self {
        Self {
            provider: provider.as_str(),
            status: "success",
            error_kind: None,
            error: None,
            retryable: None,
            provider_unavailable: None,
        }
    }

    fn failure(provider: WebSearchBackendKind, error: &BackendError) -> Self {
        Self {
            provider: provider.as_str(),
            status: "failure",
            error_kind: Some(error.kind.clone()),
            error: Some(error.message.clone()),
            retryable: Some(error.retryable),
            provider_unavailable: Some(error.provider_unavailable),
        }
    }

    fn not_configured(provider: WebSearchBackendKind) -> Self {
        Self {
            provider: provider.as_str(),
            status: "failure",
            error_kind: Some("not_configured".to_string()),
            error: Some(format!("{} backend is not configured", provider.as_str())),
            retryable: Some(false),
            provider_unavailable: Some(true),
        }
    }
}

#[derive(Debug, Clone)]
struct BackendError {
    kind: String,
    message: String,
    retryable: bool,
    provider_unavailable: bool,
}

impl BackendError {
    fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        provider_unavailable: bool,
    ) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            retryable,
            provider_unavailable,
        }
    }

    fn empty_query() -> Self {
        Self::new("empty_query", "query must be non-empty", false, false)
    }

    fn api_failure(message: impl Into<String>) -> Self {
        Self::new("api_failure", message, false, true)
    }

    fn invalid_response(message: impl Into<String>) -> Self {
        Self::new("invalid_response", message, false, true)
    }

    fn from_http(provider: &str, status: StatusCode, body: String) -> Self {
        let kind = match status.as_u16() {
            401 | 403 => "auth_failed",
            408 => "timeout",
            429 => "rate_limited",
            500 | 502 | 503 | 504 => "server",
            _ => "http_status",
        };
        let retryable = matches!(status.as_u16(), 408 | 500 | 502 | 503 | 504);
        Self::new(
            kind,
            format!("{provider} HTTP {status}: {}", truncate_for_error(body)),
            retryable,
            true,
        )
    }

    fn from_reqwest(provider: &str, error: reqwest::Error) -> Self {
        if error.is_timeout() {
            return Self::new(
                "timeout",
                format!("{provider} request timed out"),
                true,
                true,
            );
        }
        if error.is_connect() {
            return Self::new(
                "network",
                format!("{provider} connection failed: {error}"),
                true,
                true,
            );
        }
        if error.is_decode() {
            return Self::invalid_response(format!("{provider} invalid response: {error}"));
        }
        Self::new(
            "request_failed",
            format!("{provider} request failed: {error}"),
            false,
            true,
        )
    }
}

#[derive(Debug, Error)]
pub(crate) enum WebSearchError {
    #[error("invalid web_search arguments: {0}")]
    InvalidArguments(String),
    #[error("web_search provider initialization failed: {0}")]
    ProviderInit(String),
}

#[async_trait]
trait SearchBackend: Send + Sync {
    fn kind(&self) -> WebSearchBackendKind;

    async fn search(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<Vec<WebSearchResult>, BackendError>;
}

/// Provider for the single indexed `web_search` tool.
pub struct WebSearchProvider {
    backends: Vec<Arc<dyn SearchBackend>>,
}

impl WebSearchProvider {
    /// Build a provider from runtime env. Returns `Ok(None)` when no indexed
    /// search backend is configured, so `web_search` is not registered.
    pub(crate) fn new_from_env() -> Result<Option<Self>, WebSearchError> {
        let mut backends: Vec<Arc<dyn SearchBackend>> = Vec::new();

        if let Some(backend) = CrwSearchBackend::from_env()? {
            backends.push(Arc::new(backend));
        }
        if let Some(backend) = TavilySearchBackend::from_env()? {
            backends.push(Arc::new(backend));
        }
        if let Some(backend) = BraveSearchBackend::from_env()? {
            backends.push(Arc::new(backend));
        }

        if backends.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Self { backends }))
        }
    }

    #[cfg(test)]
    fn new_for_test(backends: Vec<Arc<dyn SearchBackend>>) -> Self {
        Self { backends }
    }

    /// Build native typed runtime executors for `web_search`.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = self.tool_definition();
        vec![Arc::new(WebSearchToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition(&self) -> ToolDefinition {
        let mut provider_enum = vec![json!("auto")];
        provider_enum.extend(
            self.backends
                .iter()
                .map(|backend| json!(backend.kind().as_str())),
        );

        ToolDefinition {
            name: TOOL_WEB_SEARCH.to_string(),
            description: concat!(
                "Search indexed public web results by query. ",
                "The provider field selects a preferred backend; if that backend fails, ",
                "web_search automatically falls back to another configured backend and reports the attempt chain."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "provider": {
                        "type": "string",
                        "enum": provider_enum,
                        "description": "Preferred indexed-search backend. auto uses configured backends in deterministic order."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of search results to return (1-10, default: 5)"
                    },
                    "language": {
                        "type": "string",
                        "description": "Preferred search language code, for example 'en' or 'ru'"
                    },
                    "time_range": {
                        "type": "string",
                        "enum": ["day", "week", "month", "year"],
                        "description": "Optional recency filter"
                    },
                    "safe_search": {
                        "type": "string",
                        "enum": ["off", "moderate", "strict"],
                        "description": "Safe search preference when supported by the selected backend"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute_search(&self, arguments: &str) -> Result<WebSearchToolResult, WebSearchError> {
        let args: WebSearchArgs = serde_json::from_str(arguments)
            .map_err(|error| WebSearchError::InvalidArguments(error.to_string()))?;
        let request = args.normalize()?;
        Ok(self.execute_normalized(&request).await)
    }

    async fn execute_normalized(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> WebSearchToolResult {
        let mut attempts = Vec::new();
        let provider_order = self.provider_order(request.provider);

        for provider in provider_order {
            let Some(backend) = self.backend(provider) else {
                attempts.push(WebSearchAttempt::not_configured(provider));
                continue;
            };

            match backend.search(request).await {
                Ok(results) => {
                    attempts.push(WebSearchAttempt::success(provider));
                    return WebSearchToolResult::success(request, provider, attempts, results);
                }
                Err(error) => attempts.push(WebSearchAttempt::failure(provider, &error)),
            }
        }

        WebSearchToolResult::failure(request, attempts)
    }

    fn provider_order(&self, requested: Option<WebSearchBackendKind>) -> Vec<WebSearchBackendKind> {
        let configured = self
            .backends
            .iter()
            .map(|backend| backend.kind())
            .collect::<Vec<_>>();

        if let Some(requested) = requested {
            let mut order = vec![requested];
            order.extend(configured.into_iter().filter(|kind| *kind != requested));
            return order;
        }

        configured
    }

    fn backend(&self, kind: WebSearchBackendKind) -> Option<&Arc<dyn SearchBackend>> {
        self.backends.iter().find(|backend| backend.kind() == kind)
    }
}

struct WebSearchToolResult {
    markdown: String,
    payload: Value,
    success: bool,
}

impl WebSearchToolResult {
    fn success(
        request: &NormalizedWebSearchRequest,
        provider_used: WebSearchBackendKind,
        attempts: Vec<WebSearchAttempt>,
        results: Vec<WebSearchResult>,
    ) -> Self {
        let markdown = format_success_markdown(request, provider_used, &attempts, &results);
        let payload = json!({
            "provider": TOOL_WEB_SEARCH,
            "kind": "search",
            "query": request.query,
            "requested_provider": request.provider.map(|provider| provider.as_str()).unwrap_or("auto"),
            "provider_used": provider_used.as_str(),
            "attempts": attempts,
            "results": results,
        });
        Self {
            markdown,
            payload,
            success: true,
        }
    }

    fn failure(request: &NormalizedWebSearchRequest, attempts: Vec<WebSearchAttempt>) -> Self {
        let markdown = format_failure_markdown(request, &attempts);
        let retryable = attempts
            .iter()
            .any(|attempt| attempt.retryable.unwrap_or(false));
        let payload = json!({
            "provider": TOOL_WEB_SEARCH,
            "kind": "search",
            "query": request.query,
            "requested_provider": request.provider.map(|provider| provider.as_str()).unwrap_or("auto"),
            "provider_used": Value::Null,
            "attempts": attempts,
            "results": [],
            "error_kind": "all_backends_failed",
            "error": "all configured web_search backends failed",
            "provider_unavailable": true,
            "retryable": retryable,
        });
        Self {
            markdown,
            payload,
            success: false,
        }
    }
}

fn format_success_markdown(
    request: &NormalizedWebSearchRequest,
    provider_used: WebSearchBackendKind,
    attempts: &[WebSearchAttempt],
    results: &[WebSearchResult],
) -> String {
    let mut output = format!("## Web search results for: {}\n\n", request.query);
    output.push_str(&format!("Provider used: {}\n", provider_used.as_str()));
    append_fallback_notes(&mut output, attempts);
    output.push('\n');

    if results.is_empty() {
        output.push_str("Search returned no results for this query.\n");
    } else {
        output.push_str("### Results\n\n");
        for (index, result) in results.iter().enumerate() {
            append_result(&mut output, index + 1, result);
        }
    }

    truncate_output(output)
}

fn format_failure_markdown(
    request: &NormalizedWebSearchRequest,
    attempts: &[WebSearchAttempt],
) -> String {
    let mut output = format!("web_search failed for query: {}\n\n", request.query);
    append_fallback_notes(&mut output, attempts);
    if attempts.is_empty() {
        output.push_str("No indexed search backend is configured.\n");
    }
    truncate_output(output)
}

fn append_fallback_notes(output: &mut String, attempts: &[WebSearchAttempt]) {
    let failures = attempts
        .iter()
        .filter(|attempt| attempt.status == "failure")
        .collect::<Vec<_>>();
    if failures.is_empty() {
        return;
    }

    output.push_str("Fallback attempts:\n");
    for attempt in failures {
        let kind = attempt.error_kind.as_deref().unwrap_or("unknown");
        let message = attempt.error.as_deref().unwrap_or("backend failed");
        output.push_str(&format!("- {}: {} ({})\n", attempt.provider, kind, message));
    }
}

fn append_result(output: &mut String, index: usize, result: &WebSearchResult) {
    use std::fmt::Write as _;

    let title = crate::utils::clean_html(&result.title);
    let snippet = crate::utils::clean_html(&result.snippet);
    let _ = writeln!(output, "{index}. **{title}**");
    let _ = writeln!(output, "   URL: {}", result.url);
    if !snippet.trim().is_empty() {
        let _ = writeln!(output, "   Snippet: {snippet}");
    }
    let _ = writeln!(output, "   Source: {}", result.source_provider);
    output.push('\n');
}

fn truncate_output(mut output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }

    let truncated = output.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    output.clear();
    output.push_str(&truncated);
    output.push_str("\n\n[truncated]\n");
    output
}

fn truncate_for_error(body: String) -> String {
    const LIMIT: usize = 500;
    if body.chars().count() <= LIMIT {
        return body;
    }
    let mut truncated = body.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}

struct WebSearchToolExecutor {
    provider: Arc<WebSearchProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for WebSearchToolExecutor {
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

fn search_runtime_error(error: WebSearchError) -> ToolRuntimeError {
    match error {
        WebSearchError::InvalidArguments(message) => ToolRuntimeError::InvalidArguments(message),
        WebSearchError::ProviderInit(message) => ToolRuntimeError::Failure(message),
    }
}

struct TavilySearchBackend {
    client: reqwest::Client,
    api_key: String,
}

impl TavilySearchBackend {
    fn from_env() -> Result<Option<Self>, WebSearchError> {
        let Some(api_key) = get_tavily_api_key() else {
            return Ok(None);
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| WebSearchError::ProviderInit(error.to_string()))?;
        Ok(Some(Self { client, api_key }))
    }
}

#[async_trait]
impl SearchBackend for TavilySearchBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Tavily
    }

    async fn search(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<Vec<WebSearchResult>, BackendError> {
        let body = TavilySearchRequest {
            api_key: &self.api_key,
            query: &request.query,
            search_depth: "basic",
            max_results: request.max_results,
        };
        let url = format!("{TAVILY_API_BASE}/search");
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|error| BackendError::from_reqwest("tavily", error))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(BackendError::from_http("tavily", status, body));
        }

        let parsed = response
            .json::<TavilySearchResponse>()
            .await
            .map_err(|error| BackendError::from_reqwest("tavily", error))?;
        Ok(parsed
            .results
            .into_iter()
            .take(usize::from(request.max_results))
            .map(|result| WebSearchResult {
                title: result.title,
                url: result.url,
                snippet: result.content,
                source_provider: WebSearchBackendKind::Tavily.as_str(),
            })
            .collect())
    }
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
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

struct CrwSearchBackend {
    base_url: String,
    client: reqwest::Client,
    api_token: String,
}

impl CrwSearchBackend {
    fn from_env() -> Result<Option<Self>, WebSearchError> {
        let (Some(base_url), Some(api_token)) = (get_crw_base_url(), get_crw_api_token()) else {
            return Ok(None);
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(get_crw_timeout_secs()))
            .build()
            .map_err(|error| WebSearchError::ProviderInit(error.to_string()))?;
        Ok(Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            api_token,
        }))
    }

    async fn search_once(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<CrwSearchResponse, BackendError> {
        let endpoint = format!("{}/v1/search", self.base_url);
        let body = CrwSearchRequest {
            query: request.query.clone(),
            limit: request.max_results,
            sources: vec!["web".to_string()],
            lang: request.language.clone(),
            tbs: request.time_range.as_deref().and_then(time_range_to_tbs),
        };

        let response = self
            .client
            .post(endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.api_token))
            .json(&body)
            .send()
            .await
            .map_err(|error| BackendError::from_reqwest("crw", error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(BackendError::from_http("crw", status, body));
        }

        let parsed = response
            .json::<CrwSearchResponse>()
            .await
            .map_err(|error| BackendError::from_reqwest("crw", error))?;
        if !parsed.success {
            return Err(BackendError::api_failure(
                parsed
                    .error
                    .unwrap_or_else(|| "CRW returned success=false".to_string()),
            ));
        }
        Ok(parsed)
    }
}

#[async_trait]
impl SearchBackend for CrwSearchBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Crw
    }

    async fn search(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<Vec<WebSearchResult>, BackendError> {
        if request.query.trim().is_empty() {
            return Err(BackendError::empty_query());
        }

        for attempt in 0..=CRW_MAX_RETRIES {
            match self.search_once(request).await {
                Ok(response) => {
                    return Ok(response
                        .data
                        .into_iter()
                        .take(usize::from(request.max_results))
                        .map(|result| WebSearchResult {
                            title: result.title,
                            url: result.url,
                            snippet: result.content,
                            source_provider: WebSearchBackendKind::Crw.as_str(),
                        })
                        .collect());
                }
                Err(error) if error.retryable && attempt < CRW_MAX_RETRIES => {
                    tokio::time::sleep(retry_delay(attempt + 1)).await;
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("CRW retry loop ran at least once")
    }
}

#[derive(Debug, Clone, Serialize)]
struct CrwSearchRequest {
    query: String,
    limit: u8,
    sources: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lang: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tbs: Option<String>,
}

#[derive(Debug)]
struct CrwSearchResponse {
    success: bool,
    data: Vec<CrwSearchResult>,
    error: Option<String>,
}

impl<'de> Deserialize<'de> for CrwSearchResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawResponse {
            #[serde(default)]
            success: Option<bool>,
            #[serde(default)]
            data: Value,
            #[serde(default)]
            results: Value,
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            message: Option<String>,
        }

        let raw = RawResponse::deserialize(deserializer)?;
        let data = search_results_from_value(&raw.data)
            .or_else(|| search_results_from_value(&raw.results))
            .unwrap_or_default();

        Ok(Self {
            success: raw.success.unwrap_or(true),
            data,
            error: raw.error.or(raw.message),
        })
    }
}

#[derive(Debug)]
struct CrwSearchResult {
    title: String,
    url: String,
    content: String,
}

impl CrwSearchResult {
    fn from_json_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            title: first_non_empty_string(object, &["title", "name"]),
            url: first_non_empty_string(object, &["url", "link", "href"]),
            content: first_non_empty_string(object, &["content", "description", "snippet"]),
        })
    }
}

fn search_results_from_value(value: &Value) -> Option<Vec<CrwSearchResult>> {
    if value.is_null() {
        return Some(Vec::new());
    }
    if value.is_array() {
        return Some(parse_search_result_array(value));
    }

    let object = value.as_object()?;
    if let Some(results) = object.get("results") {
        if results.is_array() {
            return Some(parse_search_result_array(results));
        }
        if let Some(grouped) = results.as_object() {
            return Some(flatten_search_result_groups(grouped));
        }
    }

    let flattened = flatten_search_result_groups(object);
    if flattened.is_empty() {
        None
    } else {
        Some(flattened)
    }
}

fn flatten_search_result_groups(grouped: &serde_json::Map<String, Value>) -> Vec<CrwSearchResult> {
    let mut flattened = Vec::new();
    for entries in grouped.values() {
        if entries.is_array() {
            flattened.append(&mut parse_search_result_array(entries));
            continue;
        }
        if let Some(nested_results) = entries.get("results")
            && nested_results.is_array()
        {
            flattened.append(&mut parse_search_result_array(nested_results));
        }
    }
    flattened
}

fn parse_search_result_array(value: &Value) -> Vec<CrwSearchResult> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(CrwSearchResult::from_json_value)
                .collect()
        })
        .unwrap_or_default()
}

fn first_non_empty_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        let text = json_value_to_string(value);
        if !text.trim().is_empty() {
            return text;
        }
    }
    String::new()
}

fn json_value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn time_range_to_tbs(value: &str) -> Option<String> {
    match value {
        "day" => Some("qdr:d".to_string()),
        "week" => Some("qdr:w".to_string()),
        "month" => Some("qdr:m".to_string()),
        "year" => Some("qdr:y".to_string()),
        _ => None,
    }
}

struct BraveSearchBackend {
    api_key: String,
    endpoint: String,
    client: reqwest::Client,
    limiter: Arc<BraveRateLimiter>,
    default_country: String,
    default_search_lang: String,
    default_ui_lang: String,
    default_safesearch: String,
}

struct BraveRateLimiter {
    semaphore: Semaphore,
    min_delay: Duration,
    last_started: Mutex<Option<tokio::time::Instant>>,
}

impl BraveSearchBackend {
    fn from_env() -> Result<Option<Self>, WebSearchError> {
        let Some(api_key) = get_brave_backend_api_key() else {
            return Ok(None);
        };
        let timeout = Duration::from_secs(get_brave_backend_timeout());
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| WebSearchError::ProviderInit(error.to_string()))?;
        Ok(Some(Self {
            api_key,
            endpoint: BRAVE_WEB_SEARCH_ENDPOINT.to_string(),
            client,
            limiter: Arc::new(BraveRateLimiter {
                semaphore: Semaphore::new(get_brave_backend_max_concurrent().max(1)),
                min_delay: Duration::from_millis(get_brave_backend_min_delay_ms()),
                last_started: Mutex::new(None),
            }),
            default_country: get_brave_backend_country(),
            default_search_lang: get_brave_backend_lang(),
            default_ui_lang: get_brave_backend_ui_lang(),
            default_safesearch: get_brave_backend_safesearch(),
        }))
    }

    async fn wait_turn(&self) {
        let mut last_started = self.limiter.last_started.lock().await;
        if let Some(last) = *last_started {
            let elapsed = last.elapsed();
            if elapsed < self.limiter.min_delay {
                tokio::time::sleep(self.limiter.min_delay - elapsed).await;
            }
        }
        *last_started = Some(tokio::time::Instant::now());
    }

    async fn search_once(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<Vec<WebSearchResult>, BackendError> {
        let _permit =
            self.limiter.semaphore.acquire().await.map_err(|error| {
                BackendError::new("request_failed", error.to_string(), true, true)
            })?;
        self.wait_turn().await;
        let url = self.request_url(request)?;
        let response = self
            .client
            .get(url)
            .header(ACCEPT, "application/json")
            .header(BRAVE_SUBSCRIPTION_TOKEN_HEADER, self.api_key.as_str())
            .send()
            .await
            .map_err(|error| BackendError::from_reqwest("brave", error))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(BackendError::from_http("brave", status, body));
        }

        let parsed = response
            .json::<BraveSearchResponse>()
            .await
            .map_err(|error| BackendError::from_reqwest("brave", error))?;
        Ok(parsed
            .web
            .map(|web| web.results)
            .unwrap_or_default()
            .into_iter()
            .take(usize::from(request.max_results))
            .map(|result| WebSearchResult {
                title: result.title,
                url: result.url,
                snippet: result.description,
                source_provider: WebSearchBackendKind::Brave.as_str(),
            })
            .collect())
    }

    fn request_url(&self, request: &NormalizedWebSearchRequest) -> Result<Url, BackendError> {
        let mut url = Url::parse(&self.endpoint)
            .map_err(|error| BackendError::new("request_failed", error.to_string(), false, true))?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("q", &request.query);
            pairs.append_pair("count", &request.max_results.to_string());
            pairs.append_pair("offset", "0");
            pairs.append_pair("safesearch", &self.safe_search(request));
            pairs.append_pair("extra_snippets", "false");
            push_optional_param(&mut pairs, "country", Some(self.default_country.as_str()));
            push_optional_param(
                &mut pairs,
                "search_lang",
                request
                    .language
                    .as_deref()
                    .or(Some(self.default_search_lang.as_str())),
            );
            push_optional_param(&mut pairs, "ui_lang", Some(self.default_ui_lang.as_str()));
            push_optional_param(
                &mut pairs,
                "freshness",
                request.time_range.as_deref().and_then(time_range_to_brave),
            );
        }
        Ok(url)
    }

    fn safe_search(&self, request: &NormalizedWebSearchRequest) -> String {
        request
            .safe_search
            .as_deref()
            .and_then(normalize_brave_safe_search)
            .or_else(|| normalize_brave_safe_search(&self.default_safesearch))
            .unwrap_or("moderate")
            .to_string()
    }
}

#[async_trait]
impl SearchBackend for BraveSearchBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Brave
    }

    async fn search(
        &self,
        request: &NormalizedWebSearchRequest,
    ) -> Result<Vec<WebSearchResult>, BackendError> {
        let mut last_error = None;
        for attempt in 0..=BRAVE_MAX_RETRIES {
            match self.search_once(request).await {
                Ok(results) => return Ok(results),
                Err(error) if error.retryable && attempt < BRAVE_MAX_RETRIES => {
                    tokio::time::sleep(retry_delay(attempt + 1)).await;
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.expect("Brave retry loop ran at least once"))
    }
}

fn push_optional_param<T: url::form_urlencoded::Target>(
    pairs: &mut url::form_urlencoded::Serializer<'_, T>,
    key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        pairs.append_pair(key, value);
    }
}

fn time_range_to_brave(value: &str) -> Option<&'static str> {
    match value {
        "day" => Some("pd"),
        "week" => Some("pw"),
        "month" => Some("pm"),
        "year" => Some("py"),
        _ => None,
    }
}

fn normalize_brave_safe_search(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Some("off"),
        "moderate" => Some("moderate"),
        "strict" => Some("strict"),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    #[serde(default)]
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    #[serde(default)]
    results: Vec<BraveWebResult>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
}

fn retry_delay(attempt: usize) -> Duration {
    let base_ms = 500_u64;
    let max_ms = 10_000;
    let delay = base_ms * 2u64.pow(attempt.saturating_sub(1) as u32);
    let capped = delay.min(max_ms);
    let jitter_ms = u64::from(fastrand::u16(0..50));
    Duration::from_millis(capped + jitter_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_search_accepts_stringified_max_results() {
        let args: WebSearchArgs =
            serde_json::from_str(r#"{"query":"rust async","max_results":"7"}"#)
                .expect("stringified max_results should parse");

        assert_eq!(args.max_results, 7);
    }

    #[test]
    fn web_search_rejects_out_of_range_stringified_max_results() {
        assert!(
            serde_json::from_str::<WebSearchArgs>(r#"{"query":"rust async","max_results":"256"}"#)
                .is_err()
        );
    }

    struct StaticBackend {
        kind: WebSearchBackendKind,
        result: Result<Vec<WebSearchResult>, BackendError>,
    }

    #[async_trait]
    impl SearchBackend for StaticBackend {
        fn kind(&self) -> WebSearchBackendKind {
            self.kind
        }

        async fn search(
            &self,
            _request: &NormalizedWebSearchRequest,
        ) -> Result<Vec<WebSearchResult>, BackendError> {
            self.result.clone()
        }
    }

    fn request(provider: Option<WebSearchBackendKind>) -> NormalizedWebSearchRequest {
        NormalizedWebSearchRequest {
            query: "rust async".to_string(),
            provider,
            max_results: 5,
            language: None,
            time_range: None,
            safe_search: None,
        }
    }

    fn result(provider: WebSearchBackendKind) -> WebSearchResult {
        WebSearchResult {
            title: "Rust".to_string(),
            url: "https://www.rust-lang.org/".to_string(),
            snippet: "A language empowering everyone".to_string(),
            source_provider: provider.as_str(),
        }
    }

    #[tokio::test]
    async fn requested_unconfigured_provider_falls_back_to_configured_backend() {
        let provider = WebSearchProvider::new_for_test(vec![Arc::new(StaticBackend {
            kind: WebSearchBackendKind::Tavily,
            result: Ok(vec![result(WebSearchBackendKind::Tavily)]),
        })]);

        let output = provider
            .execute_normalized(&request(Some(WebSearchBackendKind::Brave)))
            .await;

        assert!(output.success);
        assert_eq!(output.payload["provider_used"], "tavily");
        assert_eq!(output.payload["attempts"][0]["provider"], "brave");
        assert_eq!(
            output.payload["attempts"][0]["error_kind"],
            "not_configured"
        );
        assert!(output.markdown.contains("Fallback attempts"));
    }

    #[tokio::test]
    async fn backend_failure_falls_back_to_next_backend() {
        let provider = WebSearchProvider::new_for_test(vec![
            Arc::new(StaticBackend {
                kind: WebSearchBackendKind::Crw,
                result: Err(BackendError::new(
                    "auth_failed",
                    "crw auth failed",
                    false,
                    true,
                )),
            }),
            Arc::new(StaticBackend {
                kind: WebSearchBackendKind::Tavily,
                result: Ok(vec![result(WebSearchBackendKind::Tavily)]),
            }),
        ]);

        let output = provider.execute_normalized(&request(None)).await;

        assert!(output.success);
        assert_eq!(output.payload["provider_used"], "tavily");
        assert_eq!(
            output.payload["attempts"]
                .as_array()
                .expect("attempts payload must be an array")
                .len(),
            2
        );
        assert_eq!(output.payload["attempts"][0]["error_kind"], "auth_failed");
    }

    #[test]
    fn tool_definition_provider_enum_contains_only_available_backends() {
        let provider = WebSearchProvider::new_for_test(vec![Arc::new(StaticBackend {
            kind: WebSearchBackendKind::Brave,
            result: Ok(vec![result(WebSearchBackendKind::Brave)]),
        })]);

        let spec = provider.tool_definition();
        let values = spec.parameters["properties"]["provider"]["enum"]
            .as_array()
            .expect("provider enum");

        assert_eq!(values, &vec![json!("auto"), json!("brave")]);
    }
}
