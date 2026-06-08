//! Crawl4AI response parsing: JSON extraction, markdown selection, HTML fallback.

use anyhow::{Result, anyhow, bail, Context};
use reqwest::Url;
use serde_json::Value;

use super::constants::ERROR_MESSAGE_MAX_CHARS;
use super::env_helpers::truncate_for_message;
use super::types::{CrawlResult, MarkdownSelection};
use super::url_validation::{dns_preflight_public, parse_public_http_url};

pub(crate) async fn parse_crawl_response(body: &[u8]) -> Result<CrawlResult> {
    let value: Value = serde_json::from_slice(body).context("crawl4ai response parse error")?;
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("crawl4ai response parse error: missing results array"))?;

    if results.len() != 1 {
        bail!("crawl4ai unexpected result count: {}", results.len());
    }

    let result = &results[0];
    if result.get("success").and_then(Value::as_bool) == Some(false) {
        let message = result
            .get("error_message")
            .and_then(Value::as_str)
            .unwrap_or("crawl4ai crawl failed");
        bail!(
            "crawl4ai crawl failed: {}",
            truncate_for_message(message, ERROR_MESSAGE_MAX_CHARS)
        );
    }

    let final_url = parse_final_url(result).await?;
    let markdown = select_markdown(result)?;
    reject_blocked_or_noise_markdown(&markdown.text)?;

    Ok(CrawlResult {
        final_url,
        status_code: result
            .get("status_code")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        markdown_kind: markdown.kind,
        content_mode: markdown.content_mode,
        source_kind: "web_page",
        markdown: markdown.text,
        raw_chars: markdown.raw_chars,
        selected_chars: markdown.selected_chars,
        elapsed_ms: result.get("elapsed_ms").and_then(Value::as_u64),
        entries_count: None,
        noise_filtered: markdown.noise_filtered,
    })
}

pub(crate) async fn parse_final_url(result: &Value) -> Result<Option<Url>> {
    let Some(raw_url) = result
        .get("url")
        .or_else(|| result.get("redirected_url"))
        .and_then(Value::as_str)
        .filter(|url| !url.trim().is_empty())
    else {
        return Ok(None);
    };

    let url = parse_public_http_url(raw_url).context("crawl4ai final_url blocked")?;
    dns_preflight_public(&url)
        .await
        .context("crawl4ai final_url blocked")?;
    Ok(Some(url))
}

pub(crate) fn select_markdown(result: &Value) -> Result<MarkdownSelection> {
    if let Some(markdown) = result.get("markdown")
        && let Some(selection) = select_crawl4ai_markdown(markdown)?
    {
        return Ok(selection);
    }

    if let Some(html) = result.get("html").and_then(Value::as_str) {
        let converted = html_to_markdown(html)?;
        if !converted.trim().is_empty() {
            return Ok(MarkdownSelection {
                kind: "html_fallback",
                content_mode: "html_fallback",
                raw_chars: html.chars().count(),
                selected_chars: converted.chars().count(),
                text: converted,
                noise_filtered: true,
            });
        }
    }

    bail!("crawl4ai response parse error: empty markdown and html fallback")
}

pub(crate) fn select_crawl4ai_markdown(markdown: &Value) -> Result<Option<MarkdownSelection>> {
    if let Some(text) = markdown.as_str() {
        let selected_chars = text.chars().count();
        return Ok((!text.trim().is_empty()).then(|| MarkdownSelection {
            kind: "raw_markdown",
            content_mode: "crawl4ai_raw_markdown",
            text: text.to_string(),
            raw_chars: selected_chars,
            selected_chars,
            noise_filtered: false,
        }));
    }

    let object = markdown
        .as_object()
        .ok_or_else(|| anyhow!("crawl4ai response parse error: unsupported markdown shape"))?;
    let raw_chars = object
        .get("raw_markdown")
        .and_then(Value::as_str)
        .map(|text| text.chars().count())
        .unwrap_or(0);
    for (kind, content_mode, field, noise_filtered) in [
        (
            "fit_markdown",
            "crawl4ai_fit_markdown",
            "fit_markdown",
            true,
        ),
        (
            "raw_markdown",
            "crawl4ai_raw_markdown",
            "raw_markdown",
            false,
        ),
        (
            "markdown_with_citations",
            "crawl4ai_citations",
            "markdown_with_citations",
            false,
        ),
    ] {
        if let Some(text) = object.get(field).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            let selected_chars = text.chars().count();
            return Ok(Some(MarkdownSelection {
                kind,
                content_mode,
                text: text.to_string(),
                raw_chars: raw_chars.max(selected_chars),
                selected_chars,
                noise_filtered,
            }));
        }
    }

    Ok(None)
}

pub(crate) fn reject_blocked_or_noise_markdown(markdown: &str) -> Result<()> {
    let trimmed = markdown.trim();
    if trimmed.chars().count() < 12 {
        bail!("crawl4ai blocked/noise page detected: near-empty successful response");
    }

    let lower = trimmed.to_ascii_lowercase();
    for marker in [
        "you've been blocked by network security",
        "blocked by anti-bot protection",
        "to continue, log in to your reddit account",
        "use your developer token",
    ] {
        if lower.contains(marker) {
            bail!("crawl4ai blocked/noise page detected: {marker}");
        }
    }
    Ok(())
}

pub(crate) fn html_to_markdown(html: &str) -> Result<String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "iframe", "object", "embed", "meta", "link", "nav",
            "footer", "aside", "form", "button", "svg", "canvas",
        ])
        .build()
        .convert(html)
        .map_err(|error| anyhow!("crawl4ai html fallback conversion failed: {error}"))
}
