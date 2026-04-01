use reqwest::StatusCode;
use serde_json::Value;

const MAX_OUTPUT_CHARS: usize = 20_000;

pub(super) enum ResponsePayload {
    Json(Value),
    Text(String),
}

pub(super) fn is_json_response(content_type: &str, bytes: &[u8]) -> bool {
    if content_type.contains("application/json") {
        return true;
    }

    let trimmed = bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    matches!(trimmed, Some(b'{') | Some(b'['))
}

pub(super) fn format_http_error(status: StatusCode, body: &str) -> String {
    let trimmed = body.trim_start();
    let is_html = trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<HTML");

    if is_html {
        return format!("Browser Use error: {status} (HTML error page)");
    }

    let mut message = trimmed.to_string();
    if message.len() > 500 {
        message.truncate(500);
        message.push_str("... (truncated)");
    }

    if message.is_empty() {
        format!("Browser Use error: {status}")
    } else {
        format!("Browser Use error: {status} - {message}")
    }
}

pub(super) fn format_tool_output(payload: ResponsePayload) -> String {
    match payload {
        ResponsePayload::Json(value) => truncate_output(format_json(&value)),
        ResponsePayload::Text(text) => truncate_output(text),
    }
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

pub(super) fn is_retryable_error(error: &str) -> bool {
    let error = error.to_lowercase();
    let transient_patterns = [
        "connection refused",
        "connection reset",
        "connection timed out",
        "timeout",
        "operation timed out",
        "temporary failure",
        "network",
        "connection closed",
        "error trying to connect",
    ];

    if transient_patterns
        .iter()
        .any(|pattern| error.contains(pattern))
    {
        return true;
    }

    error.contains("500") || error.contains("502") || error.contains("503") || error.contains("504")
}
