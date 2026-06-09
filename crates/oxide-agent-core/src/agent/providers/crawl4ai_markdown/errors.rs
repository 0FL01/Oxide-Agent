//! Error classification, failure payloads, and HTTP status helpers for Crawl4AI.

use reqwest::Url;
use serde_json::{Value, json};

use super::constants::{ERROR_MESSAGE_MAX_CHARS, RESPONSE_TAIL_MAX_CHARS, TOOL_CRAWL4AI_MARKDOWN};
use super::env_helpers::{response_tail, truncate_for_message};
use super::types::{Crawl4AiMarkdownArgs, Crawl4AiMarkdownConfig};

pub(crate) fn crawl4ai_failure_payload(
    args: Option<&Crawl4AiMarkdownArgs>,
    config: &Crawl4AiMarkdownConfig,
    error: &anyhow::Error,
) -> Value {
    let error_kind = crawl4ai_error_kind(error);
    json!({
        "provider": TOOL_CRAWL4AI_MARKDOWN,
        "error_kind": error_kind,
        "url": args.map(|args| args.url.as_str()),
        "host": args.and_then(|args| host_from_url(&args.url)),
        "crawl4ai_base_url_host": config.base_url.host_str(),
        "status_code": crawl4ai_http_status_code(error),
        "retryable": crawl4ai_error_retryable(error_kind, error),
        "provider_unavailable": error_kind == "crawl4ai_unavailable" || error_kind == "anti_bot",
        "message": crawl4ai_failure_message(args, Some(config), error),
        "response_tail": crawl4ai_response_tail(error)
    })
}

pub(crate) fn crawl4ai_failure_message(
    args: Option<&Crawl4AiMarkdownArgs>,
    _config: Option<&Crawl4AiMarkdownConfig>,
    error: &anyhow::Error,
) -> String {
    let error_kind = crawl4ai_error_kind(error);
    if error_kind == "anti_bot" {
        let error_message = format!("{error:#}");
        let detail = extract_anti_bot_detail(&error_message);
        let host = args.and_then(|a| host_from_url(&a.url));
        let location = host
            .as_deref()
            .map(|h| format!(" at {h}"))
            .unwrap_or_default();
        let suffix = "Do not retry this host in this task; use another source.";
        if let Some(d) = detail {
            return format!(
                "crawl4ai blocked by anti-bot protection ({d}){location}; crawl4ai could not bypass the challenge. {suffix}"
            );
        }
        return format!(
            "crawl4ai blocked by anti-bot protection{location}; crawl4ai could not bypass the challenge. {suffix}"
        );
    }
    truncate_for_message(&format!("{error:#}"), ERROR_MESSAGE_MAX_CHARS)
}

/// Extracts a short anti-bot detail (e.g. "Cloudflare JS challenge") from the
/// Crawl4AI response tail embedded in the error message.
fn extract_anti_bot_detail(error_message: &str) -> Option<&str> {
    let tail = error_message.split("response_tail: ").nth(1)?;
    // tail looks like: {"detail":"Blocked by anti-bot protection: Cloudflare JS challenge"}
    let lower = tail.to_ascii_lowercase();
    if !lower.contains("anti-bot") && !lower.contains("anti_bot") {
        return None;
    }
    // Extract the string value of "detail" from JSON
    let detail_value = extract_json_string_value(tail, "detail")?;
    // Strip common prefixes like "Blocked by anti-bot protection: "
    if let Some(after) = detail_value.split(": ").nth(1) {
        return Some(after);
    }
    Some(detail_value)
}

/// Naive extraction of a JSON string value for a given key from a JSON fragment.
fn extract_json_string_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\"");
    let idx = json.find(&pattern)?;
    let rest = &json[idx + pattern.len()..];
    // Skip whitespace and colon
    let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ':');
    // Expect opening quote
    let rest = rest.strip_prefix('"')?;
    // Find closing quote (unescaped)
    let mut end = 0;
    let bytes = rest.as_bytes();
    while end < bytes.len() {
        if bytes[end] == b'\\' {
            end += 2; // skip escaped char
        } else if bytes[end] == b'"' {
            return Some(&rest[..end]);
        } else {
            end += 1;
        }
    }
    None
}

pub(crate) fn crawl4ai_error_kind(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("invalid crawl4ai_markdown arguments") {
        "invalid_arguments"
    } else if message.contains("cancelled") {
        "cancelled"
    } else if message.contains("unsupported url scheme") || message.contains("not direct media/pdf")
    {
        "unsupported_url"
    } else if message.contains("refusing to crawl") {
        "ssrf_blocked"
    } else if message.contains("dns preflight failed")
        || message.contains("dns preflight returned no records")
    {
        "dns_failed"
    } else if message.contains("health") || message.contains("base url") {
        "crawl4ai_unavailable"
    } else if message.contains("crawl4ai auth failed") {
        "crawl4ai_auth_failed"
    } else if message.contains("crawl4ai returned non-success status")
        && is_anti_bot_response(&message)
    {
        "anti_bot"
    } else if message.contains("crawl4ai returned non-success status") {
        "crawl4ai_http_status"
    } else if message.contains("blocked/noise page detected") {
        "blocked_or_noise"
    } else if message.contains("reddit rss") {
        "reddit_rss_failed"
    } else if message.contains("crawl4ai crawl failed") {
        "crawl_failed"
    } else if message.contains("unexpected result count") {
        "unexpected_result_count"
    } else if message.contains("parse error")
        || message.contains("unsupported markdown shape")
        || message.contains("empty markdown")
    {
        "parse_error"
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("response too large") {
        "response_too_large"
    } else if message.contains("final_url blocked") {
        "final_url_blocked"
    } else if message.contains("request failed")
        || message.contains("failed to read crawl4ai response chunk")
    {
        "network"
    } else {
        "internal"
    }
}

pub(crate) fn crawl4ai_error_retryable(error_kind: &str, _error: &anyhow::Error) -> bool {
    match error_kind {
        "crawl4ai_unavailable" | "timeout" | "network" => true,
        "crawl4ai_http_status" => crawl4ai_http_status_code(_error)
            .is_some_and(|status| status == 429 || (500..=599).contains(&status)),
        _ => false,
    }
}

/// Check whether the response tail embedded in the error message indicates an
/// anti-bot challenge (Cloudflare, Datadome, PerimeterX, etc.).
fn is_anti_bot_response(lower_message: &str) -> bool {
    // The response tail is embedded after "response_tail: "
    let Some(tail) = lower_message.split("response_tail: ").nth(1) else {
        return false;
    };
    for marker in [
        "anti-bot protection",
        "anti_bot protection",
        "cloudflare",
        "challenge",
        "captcha",
        "datadome",
        "perimeterx",
        "kasada",
        "akamai bot manager",
    ] {
        if tail.contains(marker) {
            return true;
        }
    }
    false
}

pub(crate) fn crawl4ai_http_status_error(status: u16, body: &[u8]) -> anyhow::Error {
    let tail = response_tail(body, RESPONSE_TAIL_MAX_CHARS);
    if status == 401 || status == 403 {
        anyhow::anyhow!("crawl4ai auth failed with status: {status}; response_tail: {tail}")
    } else {
        anyhow::anyhow!("crawl4ai returned non-success status: {status}; response_tail: {tail}")
    }
}

pub(crate) fn crawl4ai_http_status_code(error: &anyhow::Error) -> Option<u16> {
    let message = format!("{error:#}");
    for marker in [
        "crawl4ai returned non-success status: ",
        "crawl4ai auth failed with status: ",
        "crawl4ai health returned non-success status: ",
    ] {
        if let Some(status) = message.split(marker).nth(1) {
            return status
                .split(|ch: char| !ch.is_ascii_digit())
                .next()?
                .parse()
                .ok();
        }
    }
    None
}

pub(crate) fn crawl4ai_response_tail(error: &anyhow::Error) -> Option<String> {
    let message = format!("{error:#}");
    message
        .split("response_tail: ")
        .nth(1)
        .map(|tail| truncate_for_message(tail, RESPONSE_TAIL_MAX_CHARS))
}

pub(crate) fn host_from_url(raw_url: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()?
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}
