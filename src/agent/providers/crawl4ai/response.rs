use anyhow::{anyhow, Result};
use base64::Engine as _;
use reqwest::StatusCode;
use serde_json::{json, Map, Value};

const MAX_PDF_BYTES: usize = 8 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 20_000;

pub(super) enum ResponsePayload {
    Json(Value),
    Text(String),
    Pdf(Vec<u8>),
}

pub(super) fn build_crawl_body(urls: Vec<String>, max_depth: Option<u8>) -> Value {
    let mut params = Map::new();
    params.insert("cache_mode".to_string(), json!("bypass"));
    if let Some(depth) = max_depth {
        params.insert("max_depth".to_string(), json!(depth));
    }

    json!({
        "urls": urls,
        "crawler_config": {
            "type": "CrawlerRunConfig",
            "params": params
        }
    })
}

pub(super) fn is_pdf_response(content_type: &str, bytes: &[u8]) -> bool {
    content_type.contains("application/pdf") || bytes.starts_with(b"%PDF-")
}

pub(super) fn is_json_response(content_type: &str, bytes: &[u8]) -> bool {
    if content_type.contains("application/json") {
        return true;
    }
    let trimmed = bytes.iter().copied().find(|b| !b.is_ascii_whitespace());
    matches!(trimmed, Some(b'{') | Some(b'['))
}

pub(super) fn format_http_error(status: StatusCode, body: &str) -> String {
    let trimmed = body.trim_start();
    let is_html = trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML");

    if is_html {
        return format!("Crawl4AI error: {status} (HTML error page)");
    }

    let mut message = trimmed.to_string();
    if message.len() > 500 {
        message.truncate(500);
        message.push_str("... (truncated)");
    }

    if message.is_empty() {
        format!("Crawl4AI error: {status}")
    } else {
        format!("Crawl4AI error: {status} - {message}")
    }
}

pub(super) fn format_crawl_output(payload: ResponsePayload) -> String {
    match payload {
        ResponsePayload::Json(value) => format_crawl_results(&value),
        ResponsePayload::Text(text) => truncate_output(text),
        ResponsePayload::Pdf(_) => "Crawl response unexpectedly returned PDF bytes.".to_string(),
    }
}

pub(super) fn format_markdown_output(payload: ResponsePayload) -> String {
    match payload {
        ResponsePayload::Json(value) => extract_markdown(&value).unwrap_or_else(|| {
            let formatted = format_json(&value);
            truncate_output(formatted)
        }),
        ResponsePayload::Text(text) => truncate_output(text),
        ResponsePayload::Pdf(_) => "Markdown response unexpectedly returned PDF bytes.".to_string(),
    }
}

pub(super) fn format_pdf_output(payload: ResponsePayload) -> Result<String> {
    match payload {
        ResponsePayload::Pdf(bytes) => format_pdf_bytes(&bytes),
        ResponsePayload::Json(value) => Ok(format_pdf_json(&value)),
        ResponsePayload::Text(text) => Ok(truncate_output(text)),
    }
}

fn format_pdf_bytes(bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(anyhow!(
            "PDF response too large ({} bytes, limit {} bytes)",
            bytes.len(),
            MAX_PDF_BYTES
        ));
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("PDF (base64, {} bytes):\n{}", bytes.len(), encoded))
}

fn format_pdf_json(value: &Value) -> String {
    if let Some(url) = value.get("url").and_then(|v| v.as_str()) {
        return format!("PDF URL: {url}");
    }
    if let Some(url) = value.get("download_url").and_then(|v| v.as_str()) {
        return format!("PDF URL: {url}");
    }
    if let Some(data) = value.get("pdf").and_then(|v| v.as_str()) {
        return truncate_output(format!("PDF (base64):\n{data}"));
    }
    if let Some(data) = value.get("data").and_then(|v| v.as_str()) {
        return truncate_output(format!("PDF (base64):\n{data}"));
    }

    truncate_output(format_json(value))
}

fn format_crawl_results(value: &Value) -> String {
    let results = match value.get("results").and_then(|v| v.as_array()) {
        Some(results) => results,
        None => return truncate_output(format_json(value)),
    };

    if results.is_empty() {
        return "Crawl completed but returned no results.".to_string();
    }

    let mut output = String::from("## Crawl results\n\n");
    for (index, item) in results.iter().enumerate() {
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown URL)");
        let content = item
            .get("markdown")
            .or_else(|| item.get("content"))
            .or_else(|| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("(no content)");
        output.push_str(&format!(
            "### {}. {}\n\n{}\n\n---\n\n",
            index + 1,
            url,
            content
        ));
    }

    truncate_output(output)
}

fn extract_markdown(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }

    for key in ["markdown", "md", "content", "text", "result"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }

    value
        .get("data")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn format_json(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn truncate_output(text: String) -> String {
    if text.len() <= MAX_OUTPUT_CHARS {
        return text;
    }

    let mut truncated = text.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    truncated.push_str("\n\n... (truncated)");
    truncated
}
