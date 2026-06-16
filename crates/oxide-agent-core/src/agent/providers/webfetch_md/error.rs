use anyhow::{Result, bail};
use reqwest::Url;
use reqwest::header::{HeaderMap, SERVER};
use serde_json::json;

use super::WebMarkdownArgs;

// --- Content-type helpers ---

pub(super) fn is_html_content_type(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml+xml")
}

pub(super) fn display_content_type(content_type: &str) -> &str {
    if content_type.trim().is_empty() {
        "(unknown)"
    } else {
        content_type
    }
}

// --- Anti-bot detection ---

pub(super) fn reject_anti_bot_challenge(headers: &HeaderMap, body: &str) -> Result<()> {
    if header_contains(headers, "cf-mitigated", "challenge") {
        bail!(super::ANTI_BOT_ERROR);
    }

    if server_header_contains_cloudflare(headers) && body_has_cloudflare_challenge_marker(body) {
        bail!(super::ANTI_BOT_ERROR);
    }

    if body_has_anti_bot_marker(body) {
        bail!(super::ANTI_BOT_ERROR);
    }

    Ok(())
}

fn header_contains(headers: &HeaderMap, name: &'static str, needle: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains(needle))
}

fn server_header_contains_cloudflare(headers: &HeaderMap) -> bool {
    headers
        .get(SERVER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().contains("cloudflare"))
}

fn body_has_cloudflare_challenge_marker(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("challenge") || lower.contains("cf-chl-") || lower.contains("just a moment")
}

fn body_has_anti_bot_marker(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();

    lower.contains("just a moment")
        || lower.contains("making sure you're not a bot")
        || lower.contains("checking your browser")
        || lower.contains("enable javascript and cookies")
        || lower.contains("requires the use of modern javascript")
        || lower.contains("anubis uses a proof-of-work scheme")
        || lower.contains("set up anubis to protect the server")
        || lower.contains("cf-chl-")
        || lower.contains("captcha challenge")
        || lower.contains("captcha verification")
        || lower.contains("please complete the captcha")
        || lower.contains("g-recaptcha-response")
}

// --- Failure reporting ---

pub(super) fn webfetch_failure_payload(
    args: Option<&WebMarkdownArgs>,
    error: &anyhow::Error,
) -> serde_json::Value {
    let error_kind = webfetch_error_kind(error);
    let host = args.and_then(|args| webfetch_host_from_url(&args.url));
    let retryable = webfetch_error_retryable(error_kind, error);

    json!({
        "provider": "web_markdown",
        "kind": "fetch",
        "url": args.map(|args| args.url.as_str()),
        "host": host,
        "error_kind": error_kind,
        "status_code": webfetch_http_status_code(error),
        "error": format!("{error:#}"),
        "retryable": retryable,
        "provider_unavailable": error_kind == "anti_bot"
    })
}

pub(super) fn webfetch_failure_message(
    args: Option<&WebMarkdownArgs>,
    error: &anyhow::Error,
) -> String {
    let error_kind = webfetch_error_kind(error);
    if error_kind == "anti_bot" {
        if let Some(host) = args.and_then(|args| webfetch_host_from_url(&args.url)) {
            return format!(
                "web_markdown blocked by anti-bot protection at {host}; this lightweight fetcher cannot solve JS/CAPTCHA/PoW challenges. Do not retry this host in this task; use another source."
            );
        }
        return concat!(
            "web_markdown blocked by anti-bot protection; this lightweight fetcher cannot solve JS/CAPTCHA/PoW challenges. ",
            "Do not retry this host in this task; use another source."
        )
        .to_string();
    }

    format!("{error:#}")
}

pub(super) fn webfetch_error_kind(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();

    if message.contains("anti-bot protection") {
        "anti_bot"
    } else if message.contains("cancelled") {
        "cancelled"
    } else if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("non-success status") {
        "http_status"
    } else if message.contains("response too large") {
        "too_large"
    } else if message.contains("unsafe redirect target")
        || message.contains("unsupported url scheme")
        || message.contains("refusing to fetch")
        || message.contains("not direct media/pdf urls")
    {
        "unsupported_url"
    } else if message.contains("request failed")
        || message.contains("failed to read response chunk")
    {
        "network"
    } else {
        "fetch_failed"
    }
}

fn webfetch_error_retryable(error_kind: &str, error: &anyhow::Error) -> bool {
    match error_kind {
        "timeout" => true,
        "network" => {
            let message = format!("{error:#}").to_ascii_lowercase();
            message.contains("connection")
                || message.contains("reset")
                || message.contains("refused")
                || message.contains("broken pipe")
                || message.contains("eof")
                || message.contains("dns")
        }
        "http_status" => {
            let message = format!("{error:#}").to_ascii_lowercase();
            message.contains(" 500")
                || message.contains(" 502")
                || message.contains(" 503")
                || message.contains(" 504")
                || message.contains(" 429")
        }
        _ => false,
    }
}

pub(super) fn webfetch_host_from_url(raw_url: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()?
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

pub(super) fn webfetch_http_status_code(error: &anyhow::Error) -> Option<u16> {
    let message = format!("{error:#}");
    let marker = "non-success status: ";
    let status = message.split(marker).nth(1)?;
    status.split_whitespace().next()?.parse().ok()
}
