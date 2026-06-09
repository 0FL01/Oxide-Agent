//! Deterministic summaries for noisy dead-end tool failures.

use crate::agent::compaction::{AgentMessageKind, count_tokens_cached};
use crate::agent::memory::{AgentMessage, MessageRole, PrunedArtifact};
use lazy_regex::lazy_regex;
use serde_json::{Value, json};

static RE_ANTI_BOT_HOST: lazy_regex::Lazy<regex::Regex> =
    lazy_regex!(r"anti-bot protection at ([A-Za-z0-9.-]+)");
static RE_HTTP_STATUS: lazy_regex::Lazy<regex::Regex> =
    lazy_regex!(r"non-success status:\s*(\d{3})");
static RE_URL: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r#"https?://[^\s"\\)]+"#);

/// A compact replacement for a raw tool failure payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolFailureSummary {
    /// Compact JSON content to keep in hot memory and send to the next LLM call.
    pub content: String,
    /// Metadata marking the raw payload as intentionally pruned.
    pub pruned_artifact: PrunedArtifact,
}

/// Outcome of rewriting an existing memory slice.
#[derive(Debug, Clone)]
pub(crate) struct ToolFailureRewrite {
    /// Replacement messages.
    pub messages: Vec<AgentMessage>,
    /// Number of tool result payloads rewritten.
    pub rewritten_count: usize,
}

struct FailureSignal {
    failure_kind: String,
    dead_end_scope: &'static str,
    target: String,
    summary: String,
    guidance: String,
}

/// Summarize a noisy tool failure payload when it is a known dead end.
#[must_use]
pub(crate) fn summarize_tool_failure_content(
    tool_name: &str,
    content: &str,
) -> Option<ToolFailureSummary> {
    let signal = classify_failure(tool_name, content)?;
    let model_content = json!({
        "status": "failure",
        "tool_name": tool_name,
        "dead_end": true,
        "dead_end_scope": signal.dead_end_scope,
        "failure_kind": signal.failure_kind,
        "target": signal.target,
        "summary": signal.summary,
        "guidance": signal.guidance,
        "tool_failure_summary_version": 1,
    });
    let model_content = serde_json::to_string(&model_content).ok()?;
    Some(ToolFailureSummary {
        pruned_artifact: PrunedArtifact {
            estimated_tokens: count_tokens_cached(content),
            original_chars: content.chars().count(),
            preview: signal.summary,
            archive_ref: None,
        },
        content: model_content,
    })
}

/// Rewrite already persisted raw tool failure messages in-place.
#[must_use]
pub(crate) fn rewrite_tool_failure_messages(
    messages: &[AgentMessage],
) -> Option<ToolFailureRewrite> {
    let mut rewritten_count = 0usize;
    let rewritten = messages
        .iter()
        .map(|message| {
            if message.role != MessageRole::Tool
                || message.resolved_kind() != AgentMessageKind::ToolResult
                || message.is_pruned()
            {
                return message.clone();
            }

            let Some(tool_name) = message.tool_name.as_deref() else {
                return message.clone();
            };
            let Some(summary) = summarize_tool_failure_content(tool_name, &message.content) else {
                return message.clone();
            };

            rewritten_count = rewritten_count.saturating_add(1);
            let mut replacement = message.clone();
            replacement.content = summary.content;
            replacement.pruned_artifact = Some(summary.pruned_artifact);
            replacement
        })
        .collect::<Vec<_>>();

    (rewritten_count > 0).then_some(ToolFailureRewrite {
        messages: rewritten,
        rewritten_count,
    })
}

fn classify_failure(tool_name: &str, content: &str) -> Option<FailureSignal> {
    let parsed = serde_json::from_str::<Value>(content).ok();
    if parsed
        .as_ref()
        .and_then(|value| value.get("tool_failure_summary_version"))
        .is_some()
    {
        return None;
    }

    if let Some(value) = parsed.as_ref() {
        if value.get("success").and_then(Value::as_bool) == Some(true)
            || value.get("status").and_then(Value::as_str) == Some("success")
        {
            return None;
        }
        if let Some(signal) = classify_structured_failure(tool_name, value) {
            return Some(signal);
        }
    }

    classify_text_failure(tool_name, content)
}

fn classify_structured_failure(tool_name: &str, value: &Value) -> Option<FailureSignal> {
    let payload = value
        .get("structured_payload")
        .filter(|payload| payload.is_object())
        .unwrap_or(value);
    let effective_tool_name = value
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or(tool_name);
    let provider = string_field(payload, "provider");
    let error_kind = string_field(payload, "error_kind");

    if is_duckduckgo(provider, effective_tool_name)
        && matches!(error_kind, Some("blocked" | "rate_limited"))
    {
        return Some(duckduckgo_dead_end(
            effective_tool_name,
            error_kind.unwrap_or("blocked"),
            string_field(payload, "query"),
        ));
    }

    if is_brave_search(provider, effective_tool_name)
        && matches!(
            error_kind,
            Some("rate_limited" | "auth" | "missing_api_key" | "server" | "network" | "timeout")
        )
    {
        return Some(brave_search_dead_end(
            error_kind.unwrap_or("provider_unavailable"),
            string_field(payload, "query"),
        ));
    }

    if is_web_markdown(provider, effective_tool_name) {
        if error_kind == Some("anti_bot") {
            return Some(web_markdown_host_dead_end(
                string_field(payload, "host"),
                string_field(payload, "url"),
            ));
        }

        if error_kind == Some("http_status")
            && let Some(status_code) = payload.get("status_code").and_then(Value::as_u64)
            && matches!(status_code, 404 | 410)
            && payload.get("retryable").and_then(Value::as_bool) != Some(true)
        {
            return Some(web_markdown_exact_url_dead_end(
                status_code,
                string_field(payload, "url"),
            ));
        }
    }

    let provider_unavailable = payload
        .get("provider_unavailable")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let retryable = payload.get("retryable").and_then(Value::as_bool);
    let error_message = string_field(value, "error_message")
        .or_else(|| string_field(payload, "error"))
        .unwrap_or_default();
    if provider_unavailable
        && retryable == Some(false)
        && error_message.to_ascii_lowercase().contains("do not retry")
    {
        return Some(provider_dead_end(
            effective_tool_name,
            provider.unwrap_or(effective_tool_name),
            error_kind.unwrap_or("provider_unavailable"),
        ));
    }

    None
}

fn classify_text_failure(tool_name: &str, content: &str) -> Option<FailureSignal> {
    if is_web_markdown(None, tool_name) {
        if let Some(captures) = RE_ANTI_BOT_HOST.captures(content) {
            return Some(web_markdown_host_dead_end(
                captures.get(1).map(|value| value.as_str()),
                first_url(content).as_deref(),
            ));
        }

        if let Some(captures) = RE_HTTP_STATUS.captures(content)
            && let Some(status_code) = captures
                .get(1)
                .and_then(|value| value.as_str().parse::<u64>().ok())
            && matches!(status_code, 404 | 410)
        {
            return Some(web_markdown_exact_url_dead_end(
                status_code,
                first_url(content).as_deref(),
            ));
        }
    }

    None
}

fn duckduckgo_dead_end(tool_name: &str, error_kind: &str, query: Option<&str>) -> FailureSignal {
    let query = query.map(trim_for_prompt);
    let summary = query.as_ref().map_or_else(
        || format!("DuckDuckGo {error_kind}"),
        |query| format!("DuckDuckGo {error_kind} query: {query}"),
    );
    FailureSignal {
        failure_kind: error_kind.to_string(),
        dead_end_scope: "provider",
        target: "duckduckgo".to_string(),
        summary,
        guidance: format!(
            "Do not retry {tool_name} with rewritten queries in this task; use existing results or another available source."
        ),
    }
}

fn brave_search_dead_end(error_kind: &str, query: Option<&str>) -> FailureSignal {
    let query = query.map(trim_for_prompt);
    let summary = query.as_ref().map_or_else(
        || format!("Brave Search {error_kind}"),
        |query| format!("Brave Search {error_kind} query: {query}"),
    );
    FailureSignal {
        failure_kind: error_kind.to_string(),
        dead_end_scope: "provider",
        target: "brave_search".to_string(),
        summary,
        guidance: "Do not retry brave_search in this task; use searxng_search or synthesize from existing results.".to_string(),
    }
}

fn web_markdown_host_dead_end(host: Option<&str>, url: Option<&str>) -> FailureSignal {
    let target = host
        .map(str::to_string)
        .or_else(|| url.map(trim_for_prompt))
        .unwrap_or_else(|| "web_markdown host".to_string());
    FailureSignal {
        failure_kind: "anti_bot".to_string(),
        dead_end_scope: "host",
        target: target.clone(),
        summary: format!("anti_bot at {target}"),
        guidance: format!(
            "Do not retry web_markdown for {target} in this task; use another source."
        ),
    }
}

fn web_markdown_exact_url_dead_end(status_code: u64, url: Option<&str>) -> FailureSignal {
    let target = url
        .map(trim_for_prompt)
        .unwrap_or_else(|| "this exact URL".to_string());
    let status_label = http_status_label(status_code);
    FailureSignal {
        failure_kind: format!("http_{status_code}"),
        dead_end_scope: "exact_url",
        target: target.clone(),
        summary: format!("{status_label} for {target}"),
        guidance: "Do not retry this exact URL in this task. The host and web_markdown are not marked unavailable; find a canonical page, docs URL, sitemap, or another source.".to_string(),
    }
}

fn provider_dead_end(tool_name: &str, provider: &str, error_kind: &str) -> FailureSignal {
    let target = trim_for_prompt(provider);
    FailureSignal {
        failure_kind: error_kind.to_string(),
        dead_end_scope: "provider",
        target: target.clone(),
        summary: format!("{target} is unavailable for {tool_name}"),
        guidance: format!(
            "Do not retry {tool_name} for this provider in this task; use another available source."
        ),
    }
}

fn is_web_markdown(provider: Option<&str>, tool_name: &str) -> bool {
    provider == Some("web_markdown") || tool_name == "web_markdown"
}

fn is_duckduckgo(provider: Option<&str>, tool_name: &str) -> bool {
    provider == Some("duckduckgo") || matches!(tool_name, "duckduckgo_search" | "duckduckgo_news")
}

fn is_brave_search(provider: Option<&str>, tool_name: &str) -> bool {
    provider == Some("brave_search") || tool_name == "brave_search"
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn first_url(content: &str) -> Option<String> {
    RE_URL.find(content).map(|value| value.as_str().to_string())
}

fn trim_for_prompt(value: &str) -> String {
    const MAX_CHARS: usize = 180;
    let mut trimmed = value.trim().chars().take(MAX_CHARS).collect::<String>();
    if value.trim().chars().count() > MAX_CHARS {
        trimmed.push_str("...");
    }
    trimmed
}

fn http_status_label(status_code: u64) -> String {
    match status_code {
        404 => "404 Not Found".to_string(),
        410 => "410 Gone".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCallCorrelation;

    fn parsed_summary(content: &str) -> Value {
        serde_json::from_str(content).expect("summary content is valid json")
    }

    #[test]
    fn summarizes_web_markdown_anti_bot_as_host_dead_end() {
        let raw = serde_json::to_string(&json!({
            "status": "failure",
            "success": false,
            "tool_name": "web_markdown",
            "error_message": "web_markdown blocked by anti-bot protection at platform.kimi.ai; do not retry",
            "structured_payload": {
                "provider": "web_markdown",
                "kind": "fetch",
                "error_kind": "anti_bot",
                "host": "platform.kimi.ai",
                "url": "https://platform.kimi.ai/pricing/limits",
                "retryable": false,
                "provider_unavailable": true
            }
        }))
        .expect("raw json");

        let summary = summarize_tool_failure_content("web_markdown", &raw)
            .expect("anti-bot failure should be summarized");
        let value = parsed_summary(&summary.content);

        assert_eq!(value["dead_end"], true);
        assert_eq!(value["dead_end_scope"], "host");
        assert_eq!(value["failure_kind"], "anti_bot");
        assert_eq!(value["target"], "platform.kimi.ai");
        assert!(
            value["guidance"]
                .as_str()
                .expect("guidance")
                .contains("Do not retry web_markdown for platform.kimi.ai")
        );
        assert!(summary.pruned_artifact.original_chars > summary.content.len());
    }

    #[test]
    fn summarizes_duckduckgo_block_as_provider_dead_end() {
        let raw = serde_json::to_string(&json!({
            "status": "failure",
            "success": false,
            "tool_name": "duckduckgo_search",
            "error_message": "DuckDuckGo is temporarily blocking or rate-limiting requests",
            "structured_payload": {
                "provider": "duckduckgo",
                "kind": "search",
                "query": "kimi code subscription $20 per month agent API",
                "region": "wt-wt",
                "error_kind": "blocked",
                "provider_unavailable": true,
                "results": []
            }
        }))
        .expect("raw json");

        let summary = summarize_tool_failure_content("duckduckgo_search", &raw)
            .expect("blocked DuckDuckGo failure should be summarized");
        let value = parsed_summary(&summary.content);

        assert_eq!(value["dead_end_scope"], "provider");
        assert_eq!(value["failure_kind"], "blocked");
        assert_eq!(value["target"], "duckduckgo");
        assert!(
            value["summary"]
                .as_str()
                .expect("summary")
                .contains("kimi code subscription")
        );
        assert!(
            value["guidance"]
                .as_str()
                .expect("guidance")
                .contains("Do not retry duckduckgo_search")
        );
    }

    #[test]
    fn summarizes_brave_rate_limit_as_provider_dead_end() {
        let raw = serde_json::to_string(&json!({
            "provider": "brave_search",
            "kind": "search",
            "query": "rust async runtime comparison",
            "error_kind": "rate_limited",
            "error": "Brave Search is temporarily rate-limited",
            "provider_unavailable": true,
            "retryable": false,
            "fallback": "searxng_search",
            "results": []
        }))
        .expect("raw json");

        let summary = summarize_tool_failure_content("brave_search", &raw)
            .expect("rate-limited Brave failure should be summarized");
        let value = parsed_summary(&summary.content);

        assert_eq!(value["dead_end_scope"], "provider");
        assert_eq!(value["failure_kind"], "rate_limited");
        assert_eq!(value["target"], "brave_search");
        assert!(
            value["summary"]
                .as_str()
                .expect("summary")
                .contains("Brave Search rate_limited query: rust async runtime comparison")
        );
        assert!(
            value["guidance"]
                .as_str()
                .expect("guidance")
                .contains("Do not retry brave_search in this task; use searxng_search")
        );
    }

    #[test]
    fn summarizes_web_markdown_404_as_exact_url_dead_end() {
        let raw = serde_json::to_string(&json!({
            "status": "failure",
            "success": false,
            "tool_name": "web_markdown",
            "error_message": "web_markdown fetch failed: server returned non-success status: 404 Not Found",
            "structured_payload": {
                "provider": "web_markdown",
                "kind": "fetch",
                "error_kind": "http_status",
                "host": "kimi.com",
                "url": "https://kimi.com/api/pricing",
                "retryable": false,
                "provider_unavailable": false,
                "status_code": 404
            }
        }))
        .expect("raw json");

        let summary = summarize_tool_failure_content("web_markdown", &raw)
            .expect("404 failure should be summarized");
        let value = parsed_summary(&summary.content);

        assert_eq!(value["dead_end_scope"], "exact_url");
        assert_eq!(value["failure_kind"], "http_404");
        assert_eq!(value["target"], "https://kimi.com/api/pricing");
        assert!(
            value["guidance"]
                .as_str()
                .expect("guidance")
                .contains("host and web_markdown are not marked unavailable")
        );
    }

    #[test]
    fn does_not_summarize_success_or_retryable_500() {
        let success = serde_json::to_string(&json!({
            "status": "success",
            "success": true,
            "tool_name": "web_markdown"
        }))
        .expect("success json");
        assert!(summarize_tool_failure_content("web_markdown", &success).is_none());

        let retryable = serde_json::to_string(&json!({
            "status": "failure",
            "success": false,
            "tool_name": "web_markdown",
            "structured_payload": {
                "provider": "web_markdown",
                "error_kind": "http_status",
                "status_code": 500,
                "retryable": true
            }
        }))
        .expect("retryable json");
        assert!(summarize_tool_failure_content("web_markdown", &retryable).is_none());
    }

    #[test]
    fn rewrite_preserves_tool_identity_and_marks_pruned() {
        let raw = serde_json::to_string(&json!({
            "status": "failure",
            "success": false,
            "tool_name": "web_markdown",
            "structured_payload": {
                "provider": "web_markdown",
                "error_kind": "anti_bot",
                "host": "platform.kimi.ai",
                "retryable": false,
                "provider_unavailable": true
            }
        }))
        .expect("raw json");
        let correlation =
            ToolCallCorrelation::new("invoke-web").with_provider_tool_call_id("provider-web");
        let message =
            AgentMessage::tool_with_correlation("invoke-web", correlation, "web_markdown", &raw);

        let rewrite =
            rewrite_tool_failure_messages(&[message]).expect("message should be rewritten");
        assert_eq!(rewrite.rewritten_count, 1);
        let rewritten = rewrite.messages.first().expect("rewritten message");

        assert_eq!(rewritten.tool_call_id.as_deref(), Some("invoke-web"));
        assert_eq!(rewritten.tool_name.as_deref(), Some("web_markdown"));
        assert!(rewritten.is_pruned());
        assert!(
            rewritten
                .content
                .contains("\"tool_failure_summary_version\":1")
        );
    }
}
