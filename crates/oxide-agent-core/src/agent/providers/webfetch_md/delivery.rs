use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::convert::{OutputWindow, WindowedOutput};
use super::fetch::{FetchedMarkdownDocument, window_markdown_document};
use crate::agent::tool_runtime::ToolRuntimeError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownReadMode {
    Auto,
    Next,
}

#[derive(Debug, Clone)]
pub(crate) struct MarkdownDeliveryResult {
    pub(crate) requested_url: String,
    pub(crate) document: FetchedMarkdownDocument,
    pub(crate) output_window: OutputWindow,
    pub(crate) windowed: WindowedOutput,
}

#[derive(Debug, Default)]
pub(crate) struct MarkdownDeliveryCache {
    inner: Mutex<MarkdownDeliveryState>,
}

#[derive(Debug, Default)]
struct MarkdownDeliveryState {
    documents: HashMap<MarkdownDocumentKey, CachedMarkdownDocument>,
    last_by_session: HashMap<i64, MarkdownDocumentKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MarkdownDocumentKey {
    session_id: i64,
    requested_url: String,
}

#[derive(Debug, Clone)]
struct CachedMarkdownDocument {
    document: FetchedMarkdownDocument,
    next_offset_chars: Option<usize>,
}

impl MarkdownDeliveryCache {
    pub(crate) async fn store_document_window(
        &self,
        session_id: i64,
        requested_url: String,
        document: FetchedMarkdownDocument,
        output_window: OutputWindow,
    ) -> MarkdownDeliveryResult {
        let windowed = window_markdown_document(&document, output_window);
        let key = MarkdownDocumentKey {
            session_id,
            requested_url: requested_url.clone(),
        };
        let mut state = self.inner.lock().await;
        state.documents.insert(
            key.clone(),
            CachedMarkdownDocument {
                document: document.clone(),
                next_offset_chars: windowed.next_offset_chars,
            },
        );
        state.last_by_session.insert(session_id, key);
        MarkdownDeliveryResult {
            requested_url,
            document,
            output_window,
            windowed,
        }
    }

    pub(crate) async fn next_document_window(
        &self,
        session_id: i64,
        requested_url: Option<&str>,
        mut output_window: OutputWindow,
    ) -> Option<MarkdownDeliveryResult> {
        let mut state = self.inner.lock().await;
        let key = if let Some(requested_url) = requested_url
            .map(str::trim)
            .filter(|requested_url| !requested_url.is_empty())
        {
            MarkdownDocumentKey {
                session_id,
                requested_url: requested_url.to_string(),
            }
        } else {
            state.last_by_session.get(&session_id)?.clone()
        };
        let cached = state.documents.get_mut(&key)?;
        output_window.offset_chars = cached
            .next_offset_chars
            .unwrap_or_else(|| cached.document.markdown.chars().count());
        let windowed = window_markdown_document(&cached.document, output_window);
        cached.next_offset_chars = windowed.next_offset_chars;
        let document = cached.document.clone();
        state.last_by_session.insert(session_id, key.clone());
        Some(MarkdownDeliveryResult {
            requested_url: key.requested_url,
            document,
            output_window,
            windowed,
        })
    }
}

pub(crate) fn document_metadata(document: &FetchedMarkdownDocument, key: &str) -> Option<String> {
    document
        .metadata
        .iter()
        .find_map(|(metadata_key, value)| (metadata_key == key).then(|| value.clone()))
}

// --- Unified delivery helpers (shared by web_markdown and web_crawler) ---

/// Require a non-empty `url` argument, parameterised by tool name for errors.
pub(crate) fn require_url<'a>(
    url: Option<&'a str>,
    tool_name: &str,
) -> std::result::Result<&'a str, ToolRuntimeError> {
    url.map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            ToolRuntimeError::InvalidArguments(format!(
                "{tool_name} requires url unless read is \"next\""
            ))
        })
}

/// Parse `read` argument into `MarkdownReadMode`, parameterised by tool name for errors.
pub(crate) fn parse_read_mode(
    read: Option<&str>,
    tool_name: &str,
) -> std::result::Result<MarkdownReadMode, ToolRuntimeError> {
    match read.map(str::trim).filter(|read| !read.is_empty()) {
        None | Some("auto") => Ok(MarkdownReadMode::Auto),
        Some("next") => Ok(MarkdownReadMode::Next),
        Some(other) => Err(ToolRuntimeError::InvalidArguments(format!(
            "invalid {tool_name} read mode '{other}'; expected 'auto' or 'next'"
        ))),
    }
}

/// Resolve output window from optional `max_chars` with tool-specific defaults/clamps.
/// `offset_chars` is always 0 — the delivery cursor manages continuation offsets.
pub(crate) fn resolve_output_window(
    max_chars: Option<usize>,
    default: usize,
    min: usize,
    max: usize,
) -> OutputWindow {
    OutputWindow {
        max_chars: max_chars.unwrap_or(default).clamp(min, max),
        offset_chars: 0,
    }
}

/// Optional fields for stdout rendering (web_crawler adds backend/render).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeliveryStdoutExtra<'a> {
    pub backend: Option<&'a str>,
    pub render: Option<&'a str>,
}

/// Unified stdout renderer for a windowed delivery result.
///
/// Produces a single format used by both `web_markdown` and `web_crawler`:
/// ```text
/// ## <tool_name>
///
/// [Backend: ...]
/// [Render: ...]
/// <metadata entries>
/// [Fetched-Bytes: ...]
/// Range-Chars: <start>..<end>
/// Markdown-Chars: <total>
/// Returned-Chars: <returned>
/// Remaining-Chars: <remaining>
/// Next-Offset-Chars: <offset|none>
/// Truncated: <yes|no>
///
/// ---
///
/// <windowed text>
/// ```
pub(crate) fn render_delivery_stdout(
    tool_name: &str,
    delivery: &MarkdownDeliveryResult,
    extra: Option<&DeliveryStdoutExtra<'_>>,
) -> String {
    let mut output = String::with_capacity(256 + delivery.windowed.text.len());
    output.push_str("## ");
    output.push_str(tool_name);
    output.push_str("\n\n");

    if let Some(extra) = extra {
        if let Some(backend) = extra.backend {
            output.push_str("Backend: ");
            output.push_str(backend);
            output.push('\n');
        }
        if let Some(render) = extra.render {
            output.push_str("Render: ");
            output.push_str(render);
            output.push('\n');
        }
    }

    for (key, value) in &delivery.document.metadata {
        output.push_str(key);
        output.push_str(": ");
        output.push_str(value);
        output.push('\n');
    }

    if let Some(bytes) = delivery.document.fetched_bytes {
        output.push_str("Fetched-Bytes: ");
        output.push_str(&bytes.to_string());
        output.push('\n');
    }

    let start = delivery.output_window.offset_chars;
    let end = start + delivery.windowed.returned_chars;
    output.push_str("Range-Chars: ");
    output.push_str(&start.to_string());
    output.push_str("..");
    output.push_str(&end.to_string());
    output.push('\n');
    output.push_str("Markdown-Chars: ");
    output.push_str(&delivery.windowed.markdown_chars.to_string());
    output.push('\n');
    output.push_str("Returned-Chars: ");
    output.push_str(&delivery.windowed.returned_chars.to_string());
    output.push('\n');
    output.push_str("Remaining-Chars: ");
    output.push_str(&delivery.windowed.remaining_chars.to_string());
    output.push('\n');
    output.push_str("Next-Offset-Chars: ");
    match delivery.windowed.next_offset_chars {
        Some(offset) => output.push_str(&offset.to_string()),
        None => output.push_str("none"),
    }
    output.push('\n');
    output.push_str("Truncated: ");
    output.push_str(if delivery.windowed.was_truncated {
        "yes"
    } else {
        "no"
    });
    output.push_str("\n\n---\n\n");
    output.push_str(&delivery.windowed.text);
    output
}

/// Optional fields for structured payload (web_crawler adds backend/render/status_code/raw_payload).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DeliveryPayloadExtra<'a> {
    pub backend: Option<&'a str>,
    pub render: Option<&'a str>,
    pub rendered_with: Option<&'a str>,
    pub status_code: Option<u64>,
    pub raw_payload: Option<&'a Value>,
}

/// Unified structured payload for a windowed delivery success.
pub(crate) fn delivery_success_payload(
    tool_name: &str,
    delivery: &MarkdownDeliveryResult,
    extra: Option<&DeliveryPayloadExtra<'_>>,
) -> Value {
    let start_chars = delivery.output_window.offset_chars;
    let end_chars = start_chars + delivery.windowed.returned_chars;
    let has_more = delivery.windowed.was_truncated;
    let continue_with = has_more.then(|| {
        json!({
            "tool": tool_name,
            "args": { "read": "next" }
        })
    });

    let (backend, render, rendered_with, status_code, raw_payload) = match extra {
        Some(extra) => (
            extra.backend.map(Value::from),
            extra.render.map(Value::from),
            extra.rendered_with.map(Value::from),
            extra.status_code.map(Value::from),
            extra.raw_payload.cloned(),
        ),
        None => (None, None, None, None, None),
    };

    json!({
        "provider": tool_name,
        "backend": backend,
        "render": render,
        "rendered_with": rendered_with,
        "kind": "fetch",
        "url": delivery.requested_url,
        "final_url": document_metadata(&delivery.document, "URL"),
        "status_code": status_code,
        "markdown": delivery.windowed.text,
        "chars": delivery.windowed.markdown_chars,
        "markdown_chars": delivery.windowed.markdown_chars,
        "returned_chars": delivery.windowed.returned_chars,
        "remaining_chars": delivery.windowed.remaining_chars,
        "next_offset_chars": delivery.windowed.next_offset_chars,
        "truncated": has_more,
        "complete": start_chars == 0 && !has_more,
        "range": {
            "start_chars": start_chars,
            "end_chars": end_chars,
            "total_chars": delivery.windowed.markdown_chars,
            "has_more": has_more
        },
        "continue_with": continue_with,
        "raw_payload": raw_payload,
        "success": true
    })
}

/// Unified "no cached document" error message.
pub(crate) fn no_cached_document_message(tool_name: &str) -> String {
    format!(
        "{tool_name} has no cached page to continue in this session; call {tool_name} with url first"
    )
}

/// Unified "no cached document" structured payload.
pub(crate) fn no_cached_document_payload(tool_name: &str, backend: Option<&str>) -> Value {
    json!({
        "provider": tool_name,
        "backend": backend,
        "kind": "delivery",
        "error_kind": "no_cached_document",
        "retryable": false,
        "success": false
    })
}
