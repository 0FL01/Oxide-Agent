//! Passive research observation runtime.
//!
//! This module records typed tool-output observations without making policy
//! decisions. It intentionally stays in-process and records only when an
//! execution context supplies a [`ResearchRuntime`].

use crate::agent::tool_runtime::{ToolOutput, ToolOutputStatus};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Mutex;

mod verifier;

pub use verifier::{
    AnswerVerificationDecision, AnswerVerificationError, AnswerVerificationRequest,
    AnswerVerifierConfidence, AnswerVerifierVerdict, ResearchVerifierConfig, StrictAnswerVerifier,
    VerifierAllowedClaim, VerifierContradiction, VerifierUnsupportedClaim, parse_verifier_decision,
};

const MAX_EVIDENCE_EXCERPT_CHARS: usize = 12_000;

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

/// Bounded proof-grade document captured from a fetched source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceDocument {
    /// Tool that fetched the source text.
    pub tool_name: String,
    /// Provider field from structured payload, when present.
    pub provider: Option<String>,
    /// Requested URL.
    pub url: String,
    /// Final URL after redirects, when provided.
    pub final_url: Option<String>,
    /// HTTP status code, when provided.
    pub status_code: Option<u16>,
    /// Source priority derived from the tool name.
    pub source_priority: ResearchSourcePriority,
    /// Bounded source excerpt used by the strict verifier.
    pub excerpt: String,
    /// SHA-256 of the bounded excerpt.
    pub excerpt_sha256: String,
    /// SHA-256 of the full fetched Markdown before local evidence bounding.
    pub content_sha256: String,
    /// Full fetched Markdown character count before local evidence bounding.
    pub content_chars: usize,
    /// Bounded excerpt character count.
    pub excerpt_chars: usize,
    /// Whether provider/runtime/local evidence bounding truncated source text.
    pub truncated: bool,
    /// Provider/source kind metadata when present.
    pub source_kind: Option<String>,
    /// Provider fetch timestamp when present.
    pub fetched_at: Option<String>,
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

/// Final-answer guard decision captured for research audit/debug output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchGuardDecision {
    /// Final guard outcome, for example `allow`, `force_iteration`, or `skip_limit`.
    pub decision: String,
    /// Human-readable reason for the decision.
    pub reason: String,
    /// Whether the final answer contained a high-impact/current marker.
    pub high_impact_detected: bool,
    /// Whether adequate fetched evidence was available when the guard ran.
    pub adequate_fetched_evidence: bool,
    /// Unsupported claim category when the guard forced another iteration.
    pub unsupported_claim: Option<String>,
    /// Continuation count observed by the hook.
    pub continuation_count: usize,
    /// Continuation limit observed by the hook.
    pub continuation_limit: usize,
}

/// Strict verifier decision captured for audit/debug output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchVerifierTrace {
    /// Verifier verdict when a valid verifier decision was returned.
    pub verdict: Option<String>,
    /// Runner outcome derived from the verifier decision.
    pub outcome: String,
    /// Short verifier summary or fail-closed error summary.
    pub summary: String,
    /// Fail-closed verifier error, when no valid decision was available.
    pub error: Option<String>,
    /// Verifier round number sent to the sidecar.
    pub round: usize,
    /// Maximum verifier rounds configured for the run.
    pub max_rounds: usize,
    /// Whether the candidate final answer was a constrained no-proof report.
    pub proof_not_found_mode: bool,
    /// Number of bounded proof documents available to the verifier.
    pub evidence_document_count: usize,
    /// Exact unsupported claims reported by the verifier.
    pub unsupported_claims: Vec<String>,
    /// Contradicted claims reported by the verifier.
    pub contradictions: Vec<String>,
    /// Required next actions reported by the verifier.
    pub required_next_actions: Vec<String>,
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
    /// Bounded proof documents captured from fetched source text.
    pub evidence_documents: Vec<EvidenceDocument>,
    /// Failed research-relevant outputs.
    pub failures: Vec<ResearchFailure>,
    /// Hosts that produced anti-bot/access-blocking signals.
    pub anti_bot_hosts: Vec<String>,
    /// Final-answer guard decisions captured for audit/debug output.
    pub guard_decisions: Vec<ResearchGuardDecision>,
    /// Strict verifier decisions captured for audit/debug output.
    pub verifier_traces: Vec<ResearchVerifierTrace>,
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
            || error_kind.as_deref().is_some_and(looks_like_anti_bot);
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
            record_evidence_document(
                &mut state,
                tool_name,
                source_priority,
                output.success,
                truncated,
                payload,
            );
        }

        if !output.success {
            let host = payload.and_then(|value| string_field(value, "host"));
            if anti_bot
                && let Some(host) = host.as_deref()
                && !state.anti_bot_hosts.iter().any(|existing| existing == host)
            {
                state.anti_bot_hosts.push(host.to_string());
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

    /// Record one final-answer guard decision for audit/debug output.
    pub fn record_guard_decision(&self, decision: ResearchGuardDecision) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .guard_decisions
            .push(decision);
    }

    /// Record one strict verifier decision and return its compact payload.
    pub fn record_verifier_trace(&self, trace: ResearchVerifierTrace) -> Value {
        let payload = verifier_trace_payload(&trace);
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .verifier_traces
            .push(trace);
        payload
    }

    /// Return a cloneable point-in-time snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ResearchSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Return a compact JSON audit/debug payload for the current research state.
    #[must_use]
    pub fn audit_payload(&self) -> Value {
        audit_payload_from_snapshot(&self.snapshot())
    }
}

/// Return a compact JSON audit/debug payload for a research snapshot.
#[must_use]
pub fn audit_payload_from_snapshot(snapshot: &ResearchSnapshot) -> Value {
    json!({
        "task_kind": "unclassified",
        "mode": "evidence_guard",
        "providers_used": providers_used(snapshot),
        "queries": &snapshot.queries,
        "fetched_urls": snapshot
            .fetched_sources
            .iter()
            .map(|source| source.final_url.as_deref().unwrap_or(source.url.as_str()))
            .collect::<Vec<_>>(),
        "evidence_documents": snapshot
            .evidence_documents
            .iter()
            .map(evidence_document_payload)
            .collect::<Vec<_>>(),
        "evidence_document_count": snapshot.evidence_documents.len(),
        "evidence_observations": snapshot
            .observations
            .iter()
            .map(observation_payload)
            .collect::<Vec<_>>(),
        "failures": snapshot
            .failures
            .iter()
            .map(failure_payload)
            .collect::<Vec<_>>(),
        "anti_bot_hosts": &snapshot.anti_bot_hosts,
        "unsupported_claims": snapshot
            .guard_decisions
            .iter()
            .filter_map(|decision| decision.unsupported_claim.as_deref())
            .collect::<Vec<_>>(),
        "verifier_unsupported_claims": snapshot
            .verifier_traces
            .iter()
            .flat_map(|trace| trace.unsupported_claims.iter().map(String::as_str))
            .collect::<Vec<_>>(),
        "final_guard_decision": snapshot
            .guard_decisions
            .last()
            .map(guard_decision_payload),
        "guard_decisions": snapshot
            .guard_decisions
            .iter()
            .map(guard_decision_payload)
            .collect::<Vec<_>>(),
        "final_verifier_trace": snapshot
            .verifier_traces
            .last()
            .map(verifier_trace_payload),
        "verifier_traces": snapshot
            .verifier_traces
            .iter()
            .map(verifier_trace_payload)
            .collect::<Vec<_>>(),
        "verifier_trace_count": snapshot.verifier_traces.len(),
    })
}

fn evidence_document_payload(document: &EvidenceDocument) -> Value {
    json!({
        "tool_name": &document.tool_name,
        "provider": &document.provider,
        "url": &document.url,
        "final_url": &document.final_url,
        "status_code": document.status_code,
        "source_priority": source_priority_label(document.source_priority),
        "excerpt": &document.excerpt,
        "excerpt_sha256": &document.excerpt_sha256,
        "content_sha256": &document.content_sha256,
        "content_chars": document.content_chars,
        "excerpt_chars": document.excerpt_chars,
        "truncated": document.truncated,
        "source_kind": &document.source_kind,
        "fetched_at": &document.fetched_at,
    })
}

fn source_priority(tool_name: &str) -> Option<ResearchSourcePriority> {
    match tool_name {
        "searxng_search" | "crawl4ai_markdown" => Some(ResearchSourcePriority::Primary),
        "web_search" | "web_extract" | "web_markdown" | "brave_search" | "duckduckgo_search"
        | "duckduckgo_news" => Some(ResearchSourcePriority::Fallback),
        _ => None,
    }
}

fn providers_used(snapshot: &ResearchSnapshot) -> Vec<String> {
    let mut providers = Vec::new();
    for observation in &snapshot.observations {
        let provider = observation
            .provider
            .as_deref()
            .unwrap_or(observation.tool_name.as_str());
        if !providers.iter().any(|existing| existing == provider) {
            providers.push(provider.to_string());
        }
    }
    providers
}

fn observation_payload(observation: &ResearchObservation) -> Value {
    json!({
        "tool_name": &observation.tool_name,
        "status": format!("{:?}", observation.status),
        "success": observation.success,
        "provider": &observation.provider,
        "kind": &observation.kind,
        "query": &observation.query,
        "url": &observation.url,
        "source_priority": source_priority_label(observation.source_priority),
        "snippet_only": observation.snippet_only,
        "truncated": observation.truncated,
        "error_kind": &observation.error_kind,
        "anti_bot": observation.anti_bot,
    })
}

fn failure_payload(failure: &ResearchFailure) -> Value {
    json!({
        "tool_name": &failure.tool_name,
        "status": format!("{:?}", failure.status),
        "error_kind": &failure.error_kind,
        "message": &failure.message,
        "url": &failure.url,
        "host": &failure.host,
        "provider_unavailable": failure.provider_unavailable,
        "retryable": failure.retryable,
        "anti_bot": failure.anti_bot,
    })
}

fn guard_decision_payload(decision: &ResearchGuardDecision) -> Value {
    json!({
        "decision": &decision.decision,
        "reason": &decision.reason,
        "high_impact_detected": decision.high_impact_detected,
        "adequate_fetched_evidence": decision.adequate_fetched_evidence,
        "unsupported_claim": &decision.unsupported_claim,
        "continuation_count": decision.continuation_count,
        "continuation_limit": decision.continuation_limit,
    })
}

fn verifier_trace_payload(trace: &ResearchVerifierTrace) -> Value {
    json!({
        "verdict": &trace.verdict,
        "outcome": &trace.outcome,
        "summary": &trace.summary,
        "error": &trace.error,
        "round": trace.round,
        "max_rounds": trace.max_rounds,
        "proof_not_found_mode": trace.proof_not_found_mode,
        "evidence_document_count": trace.evidence_document_count,
        "unsupported_claim_count": trace.unsupported_claims.len(),
        "unsupported_claims": &trace.unsupported_claims,
        "contradiction_count": trace.contradictions.len(),
        "contradictions": &trace.contradictions,
        "required_next_actions": &trace.required_next_actions,
    })
}

pub(super) const fn source_priority_label(priority: ResearchSourcePriority) -> &'static str {
    match priority {
        ResearchSourcePriority::Primary => "primary",
        ResearchSourcePriority::Fallback => "fallback",
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

fn record_evidence_document(
    state: &mut ResearchSnapshot,
    tool_name: &str,
    source_priority: ResearchSourcePriority,
    success: bool,
    provider_truncated: bool,
    payload: &Value,
) {
    if !success {
        return;
    }
    if tool_name != "crawl4ai_markdown" {
        return;
    }
    let Some(markdown) = string_field(payload, "markdown") else {
        return;
    };
    let Some(url) = string_field(payload, "url") else {
        return;
    };
    if markdown.trim().is_empty() {
        return;
    }

    let content_chars = markdown.chars().count();
    let (excerpt, locally_truncated) = bounded_excerpt(&markdown, MAX_EVIDENCE_EXCERPT_CHARS);
    let excerpt_chars = excerpt.chars().count();
    state.evidence_documents.push(EvidenceDocument {
        tool_name: tool_name.to_string(),
        provider: string_field(payload, "provider"),
        final_url: string_field(payload, "final_url"),
        status_code: u16_field(payload, "status_code"),
        url,
        source_priority,
        excerpt_sha256: sha256_hex(excerpt.as_bytes()),
        content_sha256: sha256_hex(markdown.as_bytes()),
        excerpt,
        content_chars,
        excerpt_chars,
        truncated: provider_truncated || locally_truncated,
        source_kind: string_field(payload, "source_kind"),
        fetched_at: string_field(payload, "fetched_at"),
    });
}

fn bounded_excerpt(text: &str, max_chars: usize) -> (String, bool) {
    let mut excerpt = String::new();
    let mut chars = text.chars();
    for _ in 0..max_chars {
        let Some(ch) = chars.next() else {
            return (excerpt, false);
        };
        excerpt.push(ch);
    }
    (excerpt, chars.next().is_some())
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
            execution_context: ToolExecutionContext::new(config.artifact_dir),
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
    fn records_crawl4ai_evidence_document_with_hash_and_bounds() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let markdown = "А".repeat(MAX_EVIDENCE_EXCERPT_CHARS + 8);
        let mut output = normalizer.success(&invocation("crawl4ai_markdown"), &markdown, "");
        output.structured_payload = Some(json!({
            "provider": "crawl4ai_markdown",
            "kind": "fetch",
            "url": "https://huggingface.co/example/model",
            "final_url": "https://huggingface.co/example/model",
            "status_code": 200,
            "source_kind": "model_card",
            "markdown": markdown,
            "truncated": false,
            "fetched_at": "2026-06-10T15:00:00Z"
        }));

        let runtime = ResearchRuntime::new();
        runtime.record_tool_output(&output);
        let snapshot = runtime.snapshot();

        assert_eq!(snapshot.evidence_documents.len(), 1);
        let document = &snapshot.evidence_documents[0];
        assert_eq!(document.tool_name, "crawl4ai_markdown");
        assert_eq!(document.url, "https://huggingface.co/example/model");
        assert_eq!(document.status_code, Some(200));
        assert_eq!(document.source_kind.as_deref(), Some("model_card"));
        assert_eq!(document.content_chars, MAX_EVIDENCE_EXCERPT_CHARS + 8);
        assert_eq!(document.excerpt_chars, MAX_EVIDENCE_EXCERPT_CHARS);
        assert!(document.truncated);
        assert_eq!(document.excerpt.chars().count(), MAX_EVIDENCE_EXCERPT_CHARS);
        assert_eq!(
            document.excerpt_sha256,
            sha256_hex(document.excerpt.as_bytes())
        );
        assert_eq!(document.content_sha256, sha256_hex(markdown.as_bytes()));
    }

    #[test]
    fn search_snippets_and_fallback_fetches_do_not_become_proof_documents() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut search_output = normalizer.success(&invocation("searxng_search"), "markdown", "");
        search_output.structured_payload = Some(json!({
            "provider": "searxng_search",
            "kind": "search",
            "query": "russian pii model",
            "snippet_only": true,
            "results": [
                { "title": "Model", "url": "https://huggingface.co/example/model", "snippet": "claims" }
            ]
        }));
        let mut fallback_output =
            normalizer.success(&invocation("web_markdown"), "fallback markdown", "");
        fallback_output.structured_payload = Some(json!({
            "provider": "web_markdown",
            "kind": "fetch",
            "url": "https://example.test/page",
            "final_url": "https://example.test/page",
            "status_code": 200,
            "markdown": "fallback markdown",
            "truncated": false
        }));

        let runtime = ResearchRuntime::new();
        runtime.record_tool_output(&search_output);
        runtime.record_tool_output(&fallback_output);
        let snapshot = runtime.snapshot();

        assert_eq!(snapshot.search_leads.len(), 1);
        assert_eq!(snapshot.fetched_sources.len(), 1);
        assert!(snapshot.evidence_documents.is_empty());
    }

    #[test]
    fn ignores_non_research_tools() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let output = normalizer.success(&invocation("write_todos"), "ok", "");
        let runtime = ResearchRuntime::new();

        runtime.record_tool_output(&output);

        assert!(runtime.snapshot().observations.is_empty());
    }

    #[test]
    fn audit_payload_summarizes_research_state_and_guard_decision() {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig::default());
        let mut output = normalizer.success(&invocation("web_markdown"), "markdown", "");
        output.structured_payload = Some(json!({
            "provider": "web_markdown",
            "kind": "fetch",
            "url": "https://example.test/page",
            "final_url": "https://example.test/page?ok=1",
            "status_code": 200,
            "truncated": false
        }));
        let runtime = ResearchRuntime::new();
        runtime.record_tool_output(&output);
        runtime.record_guard_decision(ResearchGuardDecision {
            decision: "force_iteration".to_string(),
            reason: "unsupported current/high-impact claim without fetched source evidence"
                .to_string(),
            high_impact_detected: true,
            adequate_fetched_evidence: false,
            unsupported_claim: Some("current/high-impact claim".to_string()),
            continuation_count: 1,
            continuation_limit: 4,
        });
        runtime.record_verifier_trace(ResearchVerifierTrace {
            verdict: Some("need_more_evidence".to_string()),
            outcome: "continue".to_string(),
            summary: "metric was not found in evidence".to_string(),
            error: None,
            round: 2,
            max_rounds: 10,
            proof_not_found_mode: false,
            evidence_document_count: 0,
            unsupported_claims: vec!["Model X has 97% F1".to_string()],
            contradictions: vec![],
            required_next_actions: vec!["fetch model card with crawl4ai_markdown".to_string()],
        });

        let audit = runtime.audit_payload();

        assert_eq!(audit["mode"], "evidence_guard");
        assert_eq!(audit["providers_used"][0], "web_markdown");
        assert_eq!(audit["fetched_urls"][0], "https://example.test/page?ok=1");
        assert_eq!(audit["evidence_document_count"], 0);
        assert_eq!(audit["unsupported_claims"][0], "current/high-impact claim");
        assert_eq!(audit["verifier_trace_count"], 1);
        assert_eq!(
            audit["final_verifier_trace"]["verdict"],
            "need_more_evidence"
        );
        assert_eq!(audit["final_verifier_trace"]["unsupported_claim_count"], 1);
        assert_eq!(
            audit["verifier_unsupported_claims"][0],
            "Model X has 97% F1"
        );
        assert_eq!(
            audit["final_verifier_trace"]["required_next_actions"][0],
            "fetch model card with crawl4ai_markdown"
        );
        assert_eq!(audit["final_guard_decision"]["decision"], "force_iteration");
    }
}
