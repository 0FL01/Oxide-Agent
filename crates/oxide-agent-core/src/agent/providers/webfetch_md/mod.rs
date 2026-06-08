//! Lightweight web page fetcher.
//!
//! Provides `web_markdown`: one HTTP GET for a known URL plus optional HTML to Markdown
//! conversion. It is intentionally not a crawler, browser, or PDF exporter.

mod convert;
mod error;
mod reddit;
mod url;

use convert::{html_to_markdown, truncate_chars};
use error::{
    display_content_type, is_html_content_type, reject_anti_bot_challenge,
    webfetch_failure_message, webfetch_failure_payload, webfetch_host_from_url,
};
use reddit::{
    parse_reddit_atom_entries, reddit_thread_rss_url, render_reddit_atom_markdown, xml_tag_text,
};
use url::{parse_web_url, reject_media_url, reject_unsafe_url};

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Url;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, SERVER, USER_AGENT};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const TOOL_WEB_MARKDOWN: &str = "web_markdown";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 20_000;
const MAX_REDIRECTS: usize = 5;
const MARKDOWN_ACCEPT_HEADER: &str =
    "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const ANTI_BOT_ERROR: &str = "web_markdown blocked by anti-bot protection; this lightweight fetcher cannot solve JS/CAPTCHA challenges";

/// Local provider for fetching a single URL as Markdown.
pub struct WebFetchMdProvider {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Clone)]
struct WebMarkdownArgs {
    url: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

struct FetchResult {
    final_url: Url,
    content_type: String,
    bytes_read: usize,
    text: String,
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

                match reject_unsafe_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(format!("unsafe redirect target: {error}")),
                }
            }))
            .build()
        {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };

        Self { client }
    }

    #[cfg(test)]
    fn with_client(client: reqwest::Client) -> Self {
        Self { client }
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

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_WEB_MARKDOWN.to_string(),
            description: "Fetch one known http/https URL and return Markdown. This tool does not crawl, execute JavaScript, search the web, or export PDFs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Fully-qualified http/https URL to fetch"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional request timeout in seconds, clamped to 1..120"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn fetch_markdown(
        &self,
        args: WebMarkdownArgs,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let url = parse_web_url(&args.url)?;
        reject_media_url(&url)?;
        reject_unsafe_url(&url)?;

        let timeout_secs = args
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        // Reddit thread shortcut: fetch Atom RSS feed directly instead of
        // hitting the HTML page (which is typically blocked by anti-bot).
        if let Some(rss_url) = reddit_thread_rss_url(&url) {
            match self
                .fetch_reddit_rss(&url, &rss_url, timeout_secs, cancellation_token)
                .await
            {
                Ok(markdown) => {
                    let truncated = truncate_chars(markdown.trim().to_string(), MAX_OUTPUT_CHARS);
                    let truncated_label = if truncated.was_truncated { "yes" } else { "no" };
                    return Ok(format!(
                        "## Web Markdown\n\nURL: {}\nContent-Type: text/plain\nFetched-Bytes: 0\nTruncated: {}\n\n{}",
                        url, truncated_label, truncated.text
                    ));
                }
                Err(error) => {
                    tracing::warn!(
                        url = url.as_str(),
                        rss_url = rss_url.as_str(),
                        error = %error,
                        "reddit rss fallback failed, trying normal fetch"
                    );
                }
            }
        }

        let fetched = self
            .fetch_text(url, timeout_secs, cancellation_token)
            .await
            .context("web_markdown fetch failed")?;

        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };

        let truncated = truncate_chars(markdown.trim().to_string(), MAX_OUTPUT_CHARS);
        let truncated_label = if truncated.was_truncated { "yes" } else { "no" };

        Ok(format!(
            "## Web Markdown\n\nURL: {}\nContent-Type: {}\nFetched-Bytes: {}\nTruncated: {}\n\n{}",
            fetched.final_url,
            display_content_type(&fetched.content_type),
            fetched.bytes_read,
            truncated_label,
            truncated.text
        ))
    }

    /// Fetch a Reddit thread via its Atom RSS feed and render as Markdown.
    async fn fetch_reddit_rss(
        &self,
        target_url: &Url,
        rss_url: &Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            bail!("web_markdown cancelled before reddit rss request");
        }

        let response = self
            .client
            .get(rss_url.clone())
            .timeout(Duration::from_secs(timeout_secs))
            .header(USER_AGENT, BROWSER_USER_AGENT)
            .header(
                ACCEPT,
                "application/atom+xml, application/xml, text/xml, */*;q=0.1",
            )
            .send()
            .await
            .context("reddit rss request failed")?;

        let status = response.status();
        if !status.is_success() {
            bail!("reddit rss returned non-success status: {status}");
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let atom = String::from_utf8_lossy(&body).into_owned();

        let feed_title =
            xml_tag_text(&atom, "title").unwrap_or_else(|| "Reddit thread".to_string());
        let entries = parse_reddit_atom_entries(&atom)?;
        if entries.is_empty() {
            bail!("reddit rss parse error: empty Atom entries");
        }

        Ok(render_reddit_atom_markdown(
            target_url,
            &feed_title,
            &entries,
        ))
    }

    async fn fetch_text(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<FetchResult> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            bail!("web_markdown cancelled before request");
        }

        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(timeout_secs))
            .header(ACCEPT, MARKDOWN_ACCEPT_HEADER)
            .header(USER_AGENT, BROWSER_USER_AGENT)
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .context("request failed")?;

        let status = response.status();
        let final_url = response.url().clone();
        let headers = response.headers().clone();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if let Some(content_length) = response.content_length()
            && content_length > MAX_RESPONSE_BYTES as u64
        {
            bail!(
                "response too large by content-length: {} bytes; max is {}",
                content_length,
                MAX_RESPONSE_BYTES
            );
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let bytes_read = body.len();
        let text = String::from_utf8_lossy(&body).into_owned();

        reject_anti_bot_challenge(&headers, &text)?;

        if !status.is_success() {
            bail!("server returned non-success status: {status}");
        }

        Ok(FetchResult {
            final_url,
            content_type,
            bytes_read,
            text,
        })
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

        match self
            .provider
            .fetch_markdown(args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(output) => Ok(normalizer.success(&invocation, &output, "")),
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

fn webfetch_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid web_markdown arguments") {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

async fn read_limited_body(
    response: reqwest::Response,
    cancellation_token: Option<&CancellationToken>,
) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    loop {
        let next_chunk = if let Some(token) = cancellation_token {
            tokio::select! {
                () = token.cancelled() => bail!("web_markdown cancelled while reading response"),
                chunk = stream.next() => chunk,
            }
        } else {
            stream.next().await
        };

        let Some(chunk) = next_chunk else {
            return Ok(body);
        };
        let chunk = chunk.context("failed to read response chunk")?;

        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!(
                "response body too large: exceeds {} bytes",
                MAX_RESPONSE_BYTES
            );
        }
        body.extend_from_slice(&chunk);
    }
}


#[cfg(test)]
mod tests;
