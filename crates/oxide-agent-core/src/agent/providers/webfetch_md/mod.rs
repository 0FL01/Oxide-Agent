//! Lightweight web page fetcher.
//!
//! Provides `web_markdown`: one HTTP GET for a known URL plus optional HTML to Markdown
//! conversion. It is intentionally not a crawler, browser, or PDF exporter.

mod convert;
mod delivery;
mod error;
mod fetch;
mod known_sources;
mod reddit;
mod url;

use error::{webfetch_failure_message, webfetch_failure_payload};

pub(crate) use convert::OutputWindow;
pub(crate) use delivery::{
    MarkdownDeliveryCache, MarkdownDeliveryResult, MarkdownReadMode, document_metadata,
};
pub(crate) use fetch::{FetchedMarkdownDocument, format_markdown_document_output};

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const TOOL_WEB_MARKDOWN: &str = "web_markdown";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 20_000;
const MIN_OUTPUT_CHARS: usize = 1_000;
const MAX_OUTPUT_CHARS_REQUEST: usize = 100_000;
const MAX_OFFSET_CHARS: usize = 1_000_000;
const MAX_REDIRECTS: usize = 5;
const MARKDOWN_ACCEPT_HEADER: &str =
    "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const SIMPLE_BOT_USER_AGENT: &str = "oxide-agent-webfetch/0.1";
const ANTI_BOT_ERROR: &str = "web_markdown blocked by anti-bot protection; this lightweight fetcher cannot solve JS/CAPTCHA challenges";

/// Local provider for fetching a single URL as Markdown.
pub struct WebFetchMdProvider {
    client: reqwest::Client,
    delivery: MarkdownDeliveryCache,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct WebMarkdownArgs {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub read: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_chars: Option<usize>,
    #[serde(default)]
    pub offset_chars: Option<usize>,
}

impl WebFetchMdProvider {
    /// Create a new local web markdown provider.
    ///
    #[must_use]
    pub fn new() -> Self {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(MAX_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.stop();
                }

                match url::reject_unsafe_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(format!("unsafe redirect target: {error}")),
                }
            }))
            .build()
        {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };

        Self {
            client,
            delivery: MarkdownDeliveryCache::default(),
        }
    }

    #[cfg(test)]
    fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            delivery: MarkdownDeliveryCache::default(),
        }
    }

    pub(crate) async fn store_markdown_window(
        &self,
        session_id: i64,
        requested_url: String,
        document: FetchedMarkdownDocument,
        output_window: OutputWindow,
    ) -> MarkdownDeliveryResult {
        self.delivery
            .store_document_window(session_id, requested_url, document, output_window)
            .await
    }

    pub(crate) async fn next_markdown_window(
        &self,
        session_id: i64,
        requested_url: Option<&str>,
        output_window: OutputWindow,
    ) -> Option<MarkdownDeliveryResult> {
        self.delivery
            .next_document_window(session_id, requested_url, output_window)
            .await
    }

    /// Build native typed runtime executors for the web markdown tool.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(WebFetchMdToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    #[must_use]
    pub(crate) fn failure_payload(
        args: Option<&WebMarkdownArgs>,
        error: &anyhow::Error,
    ) -> serde_json::Value {
        error::webfetch_failure_payload(args, error)
    }

    #[must_use]
    pub(crate) fn failure_message(args: Option<&WebMarkdownArgs>, error: &anyhow::Error) -> String {
        error::webfetch_failure_message(args, error)
    }

    #[must_use]
    pub(crate) fn error_kind(error: &anyhow::Error) -> &'static str {
        error::webfetch_error_kind(error)
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_WEB_MARKDOWN.to_string(),
            description: "Fetch one known http/https URL and return Markdown. This tool does not crawl, execute JavaScript, search the web, or export PDFs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Fully-qualified http/https URL to fetch. Required unless read is \"next\"."
                    },
                    "read": {
                        "type": "string",
                        "enum": ["auto", "next"],
                        "description": "auto fetches the URL and starts reading; next continues the last cached page in this session without requiring offset_chars"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional request timeout in seconds, clamped to 1..120"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional maximum Markdown output characters, clamped to 1000..100000; default is 20000"
                    },
                    "offset_chars": {
                        "type": "integer",
                        "description": "Optional character offset into extracted Markdown for reading later chunks; default is 0"
                    }
                },
                "additionalProperties": false
            }),
        }
    }
}

impl Default for WebFetchMdProvider {
    fn default() -> Self {
        Self::new()
    }
}

struct WebFetchMdToolExecutor {
    provider: Arc<WebFetchMdProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for WebFetchMdToolExecutor {
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

        if self.name.as_str() != TOOL_WEB_MARKDOWN {
            return Err(ToolRuntimeError::Failure(format!(
                "unknown webfetch_md tool: {}",
                self.name.as_str()
            )));
        }

        let args =
            parse_web_markdown_args(&invocation.raw_arguments).map_err(webfetch_runtime_error)?;

        if web_markdown_read_mode(&args)? == MarkdownReadMode::Next {
            let Some(delivery) = self
                .provider
                .next_markdown_window(
                    invocation.session_id.as_i64(),
                    args.url.as_deref(),
                    web_markdown_output_window(&args, 0),
                )
                .await
            else {
                let mut output = normalizer.failure(
                    &invocation,
                    "web_markdown has no cached page to continue in this session; call web_markdown with url first",
                );
                output.structured_payload = Some(json!({
                    "provider": TOOL_WEB_MARKDOWN,
                    "kind": "delivery",
                    "error_kind": "no_cached_document",
                    "retryable": false,
                    "success": false
                }));
                return Ok(output);
            };

            let output_text = format_markdown_document_output(
                &delivery.document,
                delivery.output_window,
                &delivery.windowed,
            );
            let mut output = normalizer.success(&invocation, &output_text, "");
            output.structured_payload = Some(web_markdown_success_payload(&delivery));
            return Ok(output);
        }

        let requested_url = web_markdown_required_url(&args)?.to_string();

        match self
            .provider
            .fetch_markdown_document(args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(document) => {
                let delivery = self
                    .provider
                    .store_markdown_window(
                        invocation.session_id.as_i64(),
                        requested_url,
                        document,
                        web_markdown_output_window(&args, args.offset_chars.unwrap_or(0)),
                    )
                    .await;
                let output_text = format_markdown_document_output(
                    &delivery.document,
                    delivery.output_window,
                    &delivery.windowed,
                );
                let mut output = normalizer.success(&invocation, &output_text, "");
                output.structured_payload = Some(web_markdown_success_payload(&delivery));
                Ok(output)
            }
            Err(error) => {
                let mut output =
                    normalizer.failure(&invocation, webfetch_failure_message(Some(&args), &error));
                output.structured_payload = Some(webfetch_failure_payload(Some(&args), &error));
                Ok(output)
            }
        }
    }
}

fn parse_web_markdown_args(arguments: &str) -> Result<WebMarkdownArgs> {
    serde_json::from_str(arguments).context("invalid web_markdown arguments")
}

fn web_markdown_required_url(
    args: &WebMarkdownArgs,
) -> std::result::Result<&str, ToolRuntimeError> {
    args.url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            ToolRuntimeError::InvalidArguments(
                "web_markdown requires url unless read is \"next\"".to_string(),
            )
        })
}

fn web_markdown_read_mode(
    args: &WebMarkdownArgs,
) -> std::result::Result<MarkdownReadMode, ToolRuntimeError> {
    match args
        .read
        .as_deref()
        .map(str::trim)
        .filter(|read| !read.is_empty())
    {
        None | Some("auto") => Ok(MarkdownReadMode::Auto),
        Some("next") => Ok(MarkdownReadMode::Next),
        Some(other) => Err(ToolRuntimeError::InvalidArguments(format!(
            "invalid web_markdown read mode '{other}'; expected 'auto' or 'next'"
        ))),
    }
}

fn web_markdown_output_window(args: &WebMarkdownArgs, offset_chars: usize) -> OutputWindow {
    OutputWindow {
        max_chars: args
            .max_chars
            .unwrap_or(MAX_OUTPUT_CHARS)
            .clamp(MIN_OUTPUT_CHARS, MAX_OUTPUT_CHARS_REQUEST),
        offset_chars: offset_chars.min(MAX_OFFSET_CHARS),
    }
}

fn web_markdown_success_payload(delivery: &MarkdownDeliveryResult) -> serde_json::Value {
    let start_chars = delivery.output_window.offset_chars;
    let end_chars = start_chars + delivery.windowed.returned_chars;
    let has_more = delivery.windowed.was_truncated;
    let continue_with = has_more.then(|| {
        json!({
            "tool": TOOL_WEB_MARKDOWN,
            "args": { "read": "next" }
        })
    });

    json!({
        "provider": TOOL_WEB_MARKDOWN,
        "kind": "fetch",
        "url": delivery.requested_url,
        "final_url": document_metadata(&delivery.document, "URL"),
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
        "success": true
    })
}

fn webfetch_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid web_markdown arguments") {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

#[cfg(test)]
mod tests;
