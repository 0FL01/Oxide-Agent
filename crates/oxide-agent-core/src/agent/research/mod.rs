//! Passive research observation runtime.
//!
//! This module records typed tool-output observations without making policy
//! decisions. It intentionally stays in-process and disabled unless an
//! execution context supplies a [`ResearchRuntime`].

use crate::agent::tool_runtime::{ToolOutput, ToolOutputStatus};
use serde_json::Value;
use std::sync::Mutex;

/// Source priority derived from the tool that produced an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchSourcePriority {
    /// Primary research stack: SearXNG discovery and Crawl4AI fetches.
    Primary,
    /// Optional compatibility/fallback search and fetch providers.
    Fallback,
}

/// Passive observation for one research-relevant typed tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchObservation {
    /// Exact tool name that produced the output.
    pub tool_name: String,
    /// Terminal status of the typed tool output.
    pub status: ToolOutputStatus,
    /// Success flag copied from the typed output.
    pub success: bool,
    /// Provider field from structured payload, when present.
    pub provider: Option<String>,
    /// Payload kind, usually `search`, `news`, or `fetch`.
    pub kind: Option<String>,
    /// Query that produced search results, when exposed by payload.
    pub query: Option<String>,
    /// URL/final URL for a fetched source, when exposed by payload.
    pub url: Option<String>,
    /// Source priority derived from the tool name.
    pub source_priority: ResearchSourcePriority,
    /// Whether the observation is only a snippet/search lead, not full source evidence.
    pub snippet_only: bool,
    /// Whether the typed output or payload indicates truncation.
    pub truncated: bool,
    /// Classified error kind from structured payload, when present.
    pub error_kind: Option<String>,
    /// True when the payload/error looks like anti-bot or access-blocking behavior.
    pub anti_bot: bool,
}

/// Search lead discovered from a typed search payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLead {
    /// Tool that produced the search lead.
    pub tool_name: String,
    /// Query associated with the result list.
    pub query: Option<String>,
    /// Result title when provided.
    pub title: Option<String>,
    /// Result URL.
    pub url: String,
    /// Snippet/description/summary for the lead.
    pub snippet: Option<String>,
    /// Primary/fallback priority derived from the tool.
    pub source_priority: ResearchSourcePriority,
}

/// Source fetched by a typed fetch/extract payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedSource {
    /// Tool that fetched or extracted the source.
    pub tool_name: String,
    /// Requested URL.
    pub url: String,
    /// Final URL after redirects, when provided.
    pub final_url: Option<String>,
    /// HTTP status code, when provided.
    pub status_code: Option<u16>,
    /// Primary/fallback priority derived from the tool.
    pub source_priority: ResearchSourcePriority,
    /// Whether the fetched source was truncated.
    pub truncated: bool,
}

/// Failed research-relevant typed tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchFailure {
    /// Tool that failed.
    pub tool_name: String,
    /// Terminal status reported by the typed runtime.
    pub status: ToolOutputStatus,
    /// Provider-specific classified error kind.
    pub error_kind: Option<String>,
    /// Human-readable error message.
    pub message: Option<String>,
    /// URL associated with the failure, when present.
    pub url: Option<String>,
    /// Host associated with the failure, when present.
    pub host: Option<String>,
    /// Whether the provider marked itself unavailable.
    pub provider_unavailable: bool,
    /// Provider retryability hint, when present.
    pub retryable: Option<bool>,
    /// Whether the failure looks like anti-bot/access blocking.
    pub anti_bot: bool,
}

/// Snapshot of passive research state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResearchSnapshot {
    /// Unique queries seen in structured search payloads.
    pub queries: Vec<String>,
    /// One passive observation per research-relevant tool output.
    pub observations: Vec<ResearchObservation>,
    /// Search result leads discovered from structured payloads.
    pub search_leads: Vec<SearchLead>,
    /// Fetched/extracted sources discovered from structured payloads.
    pub fetched_sources: Vec<FetchedSource>,
    /// Failed research-relevant outputs.
    pub failures: Vec<ResearchFailure>,
    /// Hosts that produced anti-bot/access-blocking signals.
    pub anti_bot_hosts: Vec<String>,
}

/// In-process passive research runtime.
#[derive(Debug, Default)]
pub struct ResearchRuntime {
    state: Mutex<ResearchSnapshot>,
}

impl ResearchRuntime {
    /// Create an empty passive runtime.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one typed tool output if it is research-relevant.
    pub fn record_tool_output(&self, output: &ToolOutput) {
        let tool_name = output.tool_name.as_str();
        let Some(source_priority) = source_priority(tool_name) else {
            return;
        };

        let payload = output.structured_payload.as_ref();
        let query = payload.and_then(|value| string_field(value, "query"));
        let url = payload.and_then(primary_url);
        let kind = payload.and_then(|value| string_field(value, "kind"));
        let provider = payload.and_then(|value| string_field(value, "provider"));
        let error_kind = payload.and_then(|value| string_field(value, "error_kind"));
        let anti_bot = payload.is_some_and(payload_indicates_anti_bot)
            || error_kind
                .as_deref()
                .is_some_and(|kind| looks_like_anti_bot(kind));
        let truncated = output.truncation.content_truncated
            || output.truncation.stdout_truncated
            || payload.is_some_and(payload_indicates_truncation);
        let snippet_only = is_search_tool(tool_name)
            || payload
                .and_then(|value| bool_field(value, "snippet_only"))
                .unwrap_or(false);

        let observation = ResearchObservation {
            tool_name: tool_name.to_string(),
            status: output.status,
            success: output.success,
            provider,
            kind,
            query: query.clone(),
            url: url.clone(),
            source_priority,
            snippet_only,
            truncated,
            error_kind: error_kind.clone(),
            anti_bot,
        };

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.observations.push(observation);

        if let Some(query) = query.clone()
            && !state.queries.contains(&query)
        {
            state.queries.push(query);
        }

        if let Some(payload) = payload {
            record_search_leads(
                &mut state,
                tool_name,
                query.as_deref(),
                source_priority,
                payload,
            );
            record_fetched_source(&mut state, tool_name, source_priority, truncated, payload);
        }

        if !output.success {
            let host = payload.and_then(|value| string_field(value, "host"));
            if anti_bot
                && let Some(host) = host.clone()
                && !state.anti_bot_hosts.contains(&host)
            {
                state.anti_bot_hosts.push(host.clone());
            }
            state.failures.push(ResearchFailure {
                tool_name: tool_name.to_string(),
                status: output.status,
                error_kind,
                message: output.error_message.clone(),
                url,
                host,
                provider_unavailable: payload
                    .and_then(|value| bool_field(value, "provider_unavailable"))
                    .unwrap_or(false),
                retryable: payload.and_then(|value| bool_field(value, "retryable")),
                anti_bot,
            });
        }
    }

    /// Return a cloneable point-in-time snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ResearchSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn source_priority(tool_name: &str) -> Option<ResearchSourcePriority> {
    match tool_name {
        "searxng_search" | "crawl4ai_markdown" => Some(ResearchSourcePriority::Primary),
        "web_search" | "web_extract" | "web_markdown" | "brave_search" | "duckduckgo_search"
        | "duckduckgo_news" => Some(ResearchSourcePriority::Fallback),
        _ => None,
    }
}

fn is_search_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "searxng_search" | "web_search" | "brave_search" | "duckduckgo_search" | "duckduckgo_news"
    )
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field)?.as_str().map(str::to_string)
}

fn bool_field(value: &Value, field: &str) -> Option<bool> {
    value.get(field)?.as_bool()
}

fn u16_field(value: &Value, field: &str) -> Option<u16> {
    let number = value.get(field)?.as_u64()?;
    u16::try_from(number).ok()
}

fn primary_url(value: &Value) -> Option<String> {
    string_field(value, "final_url").or_else(|| string_field(value, "url"))
}

fn payload_indicates_truncation(value: &Value) -> bool {
    bool_field(value, "truncated").unwrap_or(false)
        || bool_field(value, "content_truncated").unwrap_or(false)
        || bool_field(value, "markdown_truncated").unwrap_or(false)
}

fn payload_indicates_anti_bot(value: &Value) -> bool {
    bool_field(value, "anti_bot").unwrap_or(false)
        || string_field(value, "error_kind")
            .as_deref()
            .is_some_and(looks_like_anti_bot)
}

fn looks_like_anti_bot(kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    lower.contains("anti_bot")
        || lower.contains("captcha")
        || lower.contains("cloudflare")
        || lower.contains("forbidden")
        || lower.contains("blocked")
}

fn record_search_leads(
    state: &mut ResearchSnapshot,
    tool_name: &str,
    query: Option<&str>,
    source_priority: ResearchSourcePriority,
    payload: &Value,
) {
    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return;
    };
    for result in results {
        let Some(url) = string_field(result, "url") else {
            continue;
        };
        state.search_leads.push(SearchLead {
            tool_name: tool_name.to_string(),
            query: query.map(str::to_string),
            title: string_field(result, "title"),
            url,
            snippet: string_field(result, "snippet")
                .or_else(|| string_field(result, "content"))
                .or_else(|| string_field(result, "description"))
                .or_else(|| string_field(result, "summary")),
            source_priority,
        });
    }
}

fn record_fetched_source(
    state: &mut ResearchSnapshot,
    tool_name: &str,
    source_priority: ResearchSourcePriority,
    truncated: bool,
    payload: &Value,
) {
    if is_search_tool(tool_name) {
        return;
    }
    let Some(url) = string_field(payload, "url") else {
        return;
    };
    state.fetched_sources.push(FetchedSource {
        tool_name: tool_name.to_string(),
        final_url: string_field(payload, "final_url"),
        status_code: u16_field(payload, "status_code"),
        url,
        source_priority,
        truncated,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool_runtime::{
        OutputNormalizer, ProviderMetadata, ToolBatchId, ToolExecutionContext, ToolInvocation,
        ToolName, ToolRuntimeConfig, TurnId,
    };
    use crate::agent::{identity::SessionId, tool_runtime::ToolCallId};
    use crate::llm::InvocationId;
    use chrono::Utc;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    fn invocation(tool_name: &str) -> ToolInvocation {
        let config = ToolRuntimeConfig::default();
        ToolInvocation {
            session_id: SessionId::from(1),
            turn_id: TurnId::from("turn"),
            batch_id: ToolBatchId::from("batch"),
            batch_index: 0,
            invocation_id: InvocationId::new("call-runtime"),
            tool_call_id: ToolCallId::from("call-provider"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: json!({}).to_string(),
            normalized_arguments: json!({}),
            cancellation_token: CancellationToken::new(),
            timeout: config.timeout,
            execution_context: ToolExecutionContext::new(config.artifact_dir.clone()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: crate::agent::tool_runtime::ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
        }
    }

    #[test]
    fn records_search_payload_leads_and_query() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut output = normalizer.success(&invocation("searxng_search"), "markdown", "");
        output.structured_payload = Some(json!({
            "provider": "searxng",
            "kind": "search",
            "query": "rust 2026",
            "results": [
                { "title": "Rust", "url": "https://example.test/rust", "content": "snippet" }
            ]
        }));

        let runtime = ResearchRuntime::new();
        runtime.record_tool_output(&output);
        let snapshot = runtime.snapshot();

        assert_eq!(snapshot.queries, vec!["rust 2026"]);
        assert_eq!(snapshot.observations.len(), 1);
        assert_eq!(
            snapshot.observations[0].source_priority,
            ResearchSourcePriority::Primary
        );
        assert!(snapshot.observations[0].snippet_only);
        assert_eq!(snapshot.search_leads.len(), 1);
        assert_eq!(snapshot.search_leads[0].url, "https://example.test/rust");
        assert_eq!(snapshot.search_leads[0].snippet.as_deref(), Some("snippet"));
    }

    #[test]
    fn records_fetch_payload_truncation_and_failure_signals() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut output = normalizer.failure(
            &invocation("crawl4ai_markdown"),
            "blocked by anti-bot protection",
        );
        output.structured_payload = Some(json!({
            "provider": "crawl4ai_markdown",
            "kind": "fetch",
            "url": "https://example.test/page",
            "final_url": "https://example.test/page?ok=1",
            "host": "example.test",
            "status_code": 403,
            "error_kind": "anti_bot_blocked",
            "retryable": false,
            "provider_unavailable": false,
            "truncated": true
        }));

        let runtime = ResearchRuntime::new();
        runtime.record_tool_output(&output);
        let snapshot = runtime.snapshot();

        assert_eq!(snapshot.observations.len(), 1);
        assert!(snapshot.observations[0].anti_bot);
        assert!(snapshot.observations[0].truncated);
        assert_eq!(snapshot.fetched_sources.len(), 1);
        assert_eq!(snapshot.fetched_sources[0].status_code, Some(403));
        assert_eq!(snapshot.failures.len(), 1);
        assert_eq!(snapshot.failures[0].host.as_deref(), Some("example.test"));
        assert_eq!(snapshot.anti_bot_hosts, vec!["example.test"]);
    }

    #[test]
    fn ignores_non_research_tools() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let output = normalizer.success(&invocation("write_todos"), "ok", "");
        let runtime = ResearchRuntime::new();

        runtime.record_tool_output(&output);

        assert!(runtime.snapshot().observations.is_empty());
    }
}
