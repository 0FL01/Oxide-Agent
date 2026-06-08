//! Browser-rendered Markdown via a configured Crawl4AI REST service.
//!
//! Provides `crawl4ai_markdown`: one validated public URL, one `POST /crawl`,
//! bounded Markdown output. Oxide does not manage Crawl4AI lifecycle.

pub(crate) mod constants;
pub(crate) mod env_helpers;
pub(crate) mod errors;
pub(crate) mod response;
pub(crate) mod types;
pub(crate) mod url_validation;

use constants::*;
use env_helpers::*;
use errors::*;
use response::*;
use types::*;
use url_validation::*;

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Native provider for browser-rendered Markdown through Crawl4AI REST.
pub struct Crawl4AiMarkdownProvider {
    client: reqwest::Client,
    config: Crawl4AiMarkdownConfig,
}

impl Crawl4AiMarkdownProvider {
    /// Create a provider from environment configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(Crawl4AiMarkdownConfig::from_env())
    }

    fn with_config(config: Crawl4AiMarkdownConfig) -> Self {
        let client_timeout = Duration::from_secs(config.max_timeout_secs.saturating_add(5));
        let client = reqwest::Client::builder()
            .timeout(client_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { client, config }
    }

    /// Build native typed runtime executors for the Crawl4AI markdown tool.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(Crawl4AiMarkdownToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_CRAWL4AI_MARKDOWN.to_string(),
            description: concat!(
                "Open one http/https URL with the configured Crawl4AI REST service and return bounded Markdown. ",
                "Use after selecting specific URLs from brave_search or searxng_search. Do not crawl every search result. ",
                "For Reddit thread URLs, omit max_chars or use 15000-30000 so comments and benchmarks are not prematurely truncated. ",
                "Use for pages that need browser rendering, JavaScript, overlay/consent handling, or when web_markdown fails. ",
                "Browser-level optimizations are enabled by default: images blocked (text_mode), background features disabled (light_mode), ad/tracker domains blocked (avoid_ads). ",
                "This tool does not crawl multiple pages, execute JavaScript, run hooks, use LLM extraction, or return screenshots/PDFs."
            ).to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Fully-qualified public http/https URL to open"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional request timeout in seconds, clamped to configured bounds"
                    },
                    "wait_for": {
                        "type": "string",
                        "description": "Optional CSS selector to wait for before extracting Markdown; JavaScript conditions are not allowed"
                    },
                    "fresh": {
                        "type": "boolean",
                        "description": "If true, bypass Crawl4AI content cache for this crawl; default false"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional maximum Markdown characters to return, clamped to configured hard cap"
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        }
    }

    async fn crawl_markdown(
        &self,
        args: Crawl4AiMarkdownArgs,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let target_url = parse_public_http_url(&args.url)?;
        reject_media_url(&target_url)?;
        dns_preflight_public(&target_url).await?;

        let timeout_secs = self.effective_timeout(args.timeout_secs);
        let max_chars = self.effective_max_chars(args.max_chars);
        let wait_for = normalize_wait_for(args.wait_for.as_deref())?;
        let started = Instant::now();

        if let Some(rss_url) = reddit_thread_rss_url(&target_url)
            && let Ok(result) = self
                .fetch_reddit_rss(&target_url, rss_url, timeout_secs, cancellation_token)
                .await
        {
            return self.success_payload(&args, &target_url, result, max_chars, started);
        }

        let result = self
            .crawl_with_retries(
                &args,
                &target_url,
                wait_for.as_deref(),
                timeout_secs,
                cancellation_token,
            )
            .await?;

        self.success_payload(&args, &target_url, result, max_chars, started)
    }

    async fn crawl_with_retries(
        &self,
        args: &Crawl4AiMarkdownArgs,
        target_url: &Url,
        wait_for: Option<&str>,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<CrawlResult> {
        let mut attempt = 0;
        loop {
            let result = self
                .crawl_once(args, target_url, wait_for, timeout_secs, cancellation_token)
                .await;
            if result.is_ok() || attempt >= self.config.max_retries {
                return result;
            }
            let error = result.err().expect("checked error");
            if !crawl4ai_error_retryable(crawl4ai_error_kind(&error), &error) {
                return Err(error);
            }
            attempt += 1;
            debug!(
                attempt,
                max_retries = self.config.max_retries,
                error_kind = crawl4ai_error_kind(&error),
                "crawl4ai_markdown: retry"
            );
            self.sleep_jitter(cancellation_token).await?;
        }
    }

    async fn crawl_once(
        &self,
        args: &Crawl4AiMarkdownArgs,
        target_url: &Url,
        wait_for: Option<&str>,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<CrawlResult> {
        ensure_not_cancelled(cancellation_token)?;
        self.health_check(cancellation_token).await?;
        ensure_not_cancelled(cancellation_token)?;

        let request = self.crawl_request_payload(target_url, wait_for, timeout_secs, args.fresh);
        debug!(
            url = %target_url,
            timeout = timeout_secs,
            fresh = args.fresh,
            wait_for = ?wait_for,
            "crawl4ai_markdown: POST /crawl"
        );
        let response = self
            .apply_auth(
                self.client
                    .post(self.endpoint("crawl")?)
                    .timeout(Duration::from_secs(timeout_secs))
                    .header(CONTENT_TYPE, "application/json")
                    .json(&request),
            )
            .send()
            .await
            .context("crawl4ai crawl request failed")?;

        let status = response.status();
        let body = read_limited_body(response, cancellation_token).await?;
        debug!(
            status = status.as_u16(),
            body_len = body.len(),
            body_head = %String::from_utf8_lossy(&body[..body.len().min(LOG_BODY_HEAD_MAX_CHARS)]),
            "crawl4ai_markdown: response"
        );
        if !status.is_success() {
            return Err(crawl4ai_http_status_error(status.as_u16(), &body));
        }

        parse_crawl_response(&body).await
    }

    async fn health_check(&self, cancellation_token: Option<&CancellationToken>) -> Result<()> {
        ensure_not_cancelled(cancellation_token)?;
        let response = self
            .apply_auth(
                self.client
                    .get(self.endpoint("health")?)
                    .timeout(Duration::from_millis(self.config.health_timeout_ms)),
            )
            .send()
            .await
            .context("crawl4ai health request failed")?;

        if !response.status().is_success() {
            bail!(
                "crawl4ai health returned non-success status: {}",
                response.status()
            );
        }
        debug!("crawl4ai_markdown: health ok");
        Ok(())
    }

    fn crawl_request_payload(
        &self,
        target_url: &Url,
        wait_for: Option<&str>,
        timeout_secs: u64,
        fresh: bool,
    ) -> Value {
        let mut crawler_params = json!({
            "stream": false,
            "cache_mode": if fresh { "bypass" } else { "enabled" },
            "page_timeout": timeout_secs.saturating_mul(1_000),
            "wait_until": "domcontentloaded",
            "remove_overlay_elements": true,
            "remove_consent_popups": true,
            "simulate_user": true,
            "override_navigator": true,
            "excluded_tags": [
                "script", "style", "noscript", "iframe", "object", "embed", "meta", "link",
                "nav", "footer", "aside", "form", "button", "svg", "canvas", "header", "menu", "dialog"
            ],
            "exclude_external_links": true,
            "exclude_social_media_links": true,
            "word_count_threshold": 3,
            "markdown_generator": {
                "type": "DefaultMarkdownGenerator",
                "params": {
                    "content_filter": {
                        "type": "PruningContentFilter",
                        "params": {
                            "threshold": 0.35,
                            "threshold_type": "fixed",
                            "min_word_threshold": 3
                        }
                    }
                }
            }
        });

        if let Some(wait_for) = wait_for {
            crawler_params["wait_for"] = Value::String(wait_for.to_string());
        }

        json!({
            "urls": [target_url.as_str()],
            "browser_config": {
                "type": "BrowserConfig",
                "params": {
                    "browser_type": "chromium",
                    "headless": true,
                    "java_script_enabled": true,
                    "enable_stealth": true,
                    "text_mode": self.config.text_mode,
                    "light_mode": self.config.light_mode,
                    "avoid_ads": self.config.avoid_ads
                }
            },
            "crawler_config": {
                "type": "CrawlerRunConfig",
                "params": crawler_params
            }
        })
    }

    fn success_payload(
        &self,
        args: &Crawl4AiMarkdownArgs,
        target_url: &Url,
        result: CrawlResult,
        max_chars: usize,
        started: Instant,
    ) -> Result<String> {
        let markdown = truncate_chars(result.markdown.trim().to_string(), max_chars);
        let payload = json!({
            "provider": TOOL_CRAWL4AI_MARKDOWN,
            "url": target_url.as_str(),
            "final_url": result.final_url.as_ref().map(Url::as_str),
            "status_code": result.status_code,
            "success": true,
            "markdown_kind": result.markdown_kind,
            "content_mode": result.content_mode,
            "source_kind": result.source_kind,
            "markdown": markdown.text,
            "truncated": markdown.was_truncated,
            "chars": markdown.text.chars().count(),
            "raw_chars": result.raw_chars,
            "selected_chars": result.selected_chars,
            "entries_count": result.entries_count,
            "noise_filtered": result.noise_filtered,
            "elapsed_ms": result.elapsed_ms.unwrap_or_else(|| millis_u64(started.elapsed())),
            "fresh": args.fresh
        });

        serde_json::to_string_pretty(&payload).context("serialize crawl4ai markdown output")
    }

    fn endpoint(&self, path: &str) -> Result<Url> {
        self.config
            .base_url
            .join(path)
            .with_context(|| format!("invalid crawl4ai {path} endpoint"))
    }

    fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(token) = self.config.api_token.as_deref() {
            request.header(AUTHORIZATION, format!("Bearer {token}"))
        } else {
            request
        }
    }

    fn effective_timeout(&self, timeout_secs: Option<u64>) -> u64 {
        timeout_secs
            .unwrap_or(self.config.default_timeout_secs)
            .clamp(1, self.config.max_timeout_secs)
    }

    fn effective_max_chars(&self, max_chars: Option<usize>) -> usize {
        max_chars
            .unwrap_or(DEFAULT_OUTPUT_CHARS)
            .clamp(1, self.config.max_output_chars)
    }

    async fn sleep_jitter(&self, cancellation_token: Option<&CancellationToken>) -> Result<()> {
        let min = self.config.jitter_min_ms;
        let max = self.config.jitter_max_ms.max(min);
        let delay_ms = if max == min {
            min
        } else {
            fastrand::u64(min..=max)
        };
        let delay = tokio::time::sleep(Duration::from_millis(delay_ms));
        tokio::pin!(delay);

        if let Some(token) = cancellation_token {
            tokio::select! {
                () = token.cancelled() => bail!("crawl4ai_markdown cancelled during retry jitter"),
                () = &mut delay => Ok(()),
            }
        } else {
            delay.await;
            Ok(())
        }
    }

    async fn fetch_reddit_rss(
        &self,
        target_url: &Url,
        rss_url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<CrawlResult> {
        ensure_not_cancelled(cancellation_token)?;
        dns_preflight_public(&rss_url)
            .await
            .context("reddit rss URL blocked")?;
        ensure_not_cancelled(cancellation_token)?;

        let response = self
            .client
            .get(rss_url.clone())
            .timeout(Duration::from_secs(timeout_secs))
            .header(USER_AGENT, "Oxide-Agent crawl4ai_markdown/1.0")
            .send()
            .await
            .context("reddit rss fetch request failed")?;
        let status = response.status();
        let body = read_limited_body(response, cancellation_token).await?;
        if !status.is_success() {
            let tail = response_tail(&body, RESPONSE_TAIL_MAX_CHARS);
            bail!(
                "reddit rss fetch returned non-success status: {}; response_tail: {tail}",
                status.as_u16()
            );
        }

        let atom = String::from_utf8(body).context("reddit rss response is not utf-8")?;
        reddit_atom_to_crawl_result(target_url, &rss_url, status.as_u16(), &atom)
    }
}

impl Default for Crawl4AiMarkdownProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Crawl4AiMarkdownConfig {
    fn from_env() -> Self {
        let max_timeout_secs = env_u64("OXIDE_CRAWL4AI_MAX_TIMEOUT_SECS", DEFAULT_MAX_TIMEOUT_SECS);
        let default_timeout_secs =
            env_u64("OXIDE_CRAWL4AI_DEFAULT_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS)
                .clamp(1, max_timeout_secs);
        let jitter_min_ms = env_u64("OXIDE_CRAWL4AI_JITTER_MIN_MS", DEFAULT_JITTER_MIN_MS);
        let jitter_max_ms =
            env_u64("OXIDE_CRAWL4AI_JITTER_MAX_MS", DEFAULT_JITTER_MAX_MS).max(jitter_min_ms);

        Self {
            base_url: env_url("OXIDE_CRAWL4AI_BASE_URL", DEFAULT_BASE_URL),
            api_token: env_non_empty("OXIDE_CRAWL4AI_API_TOKEN"),
            default_timeout_secs,
            max_timeout_secs,
            max_output_chars: env_usize(
                "OXIDE_CRAWL4AI_MAX_OUTPUT_CHARS",
                DEFAULT_MAX_OUTPUT_CHARS,
            )
            .max(1),
            health_timeout_ms: env_u64(
                "OXIDE_CRAWL4AI_HEALTH_TIMEOUT_MS",
                DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .max(1),
            jitter_min_ms,
            jitter_max_ms,
            max_retries: env_usize("OXIDE_CRAWL4AI_MAX_RETRIES", DEFAULT_MAX_RETRIES),
            text_mode: env_bool("OXIDE_CRAWL4AI_TEXT_MODE", true),
            light_mode: env_bool("OXIDE_CRAWL4AI_LIGHT_MODE", true),
            avoid_ads: env_bool("OXIDE_CRAWL4AI_AVOID_ADS", true),
        }
    }
}

struct Crawl4AiMarkdownToolExecutor {
    provider: Arc<Crawl4AiMarkdownProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for Crawl4AiMarkdownToolExecutor {
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

        if self.name.as_str() != TOOL_CRAWL4AI_MARKDOWN {
            return Err(ToolRuntimeError::Failure(format!(
                "unknown crawl4ai_markdown tool: {}",
                self.name.as_str()
            )));
        }

        let args = parse_crawl4ai_markdown_args(&invocation.raw_arguments)
            .map_err(crawl4ai_runtime_error)?;

        match self
            .provider
            .crawl_markdown(args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(output) => Ok(normalizer.success(&invocation, &output, "")),
            Err(error) => {
                let mut output = normalizer.failure(
                    &invocation,
                    crawl4ai_failure_message(Some(&args), &self.provider.config, &error),
                );
                output.structured_payload = Some(crawl4ai_failure_payload(
                    Some(&args),
                    &self.provider.config,
                    &error,
                ));
                Ok(output)
            }
        }
    }
}

fn parse_crawl4ai_markdown_args(arguments: &str) -> Result<Crawl4AiMarkdownArgs> {
    serde_json::from_str(arguments).context("invalid crawl4ai_markdown arguments")
}

fn crawl4ai_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid crawl4ai_markdown arguments") {
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
                () = token.cancelled() => bail!("crawl4ai_markdown cancelled while reading response"),
                chunk = stream.next() => chunk,
            }
        } else {
            stream.next().await
        };

        let Some(chunk) = next_chunk else {
            return Ok(body);
        };
        let chunk = chunk.context("failed to read crawl4ai response chunk")?;

        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!("crawl4ai response too large: exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
}

fn reddit_thread_rss_url(url: &Url) -> Option<Url> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "new.reddit.com" | "sh.reddit.com"
    ) {
        return None;
    }

    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 4 || segments[0] != "r" || segments[2] != "comments" {
        return None;
    }

    let mut rss_url = url.clone();
    rss_url.set_host(Some("www.reddit.com")).ok()?;
    rss_url.set_query(None);
    rss_url.set_fragment(None);

    let mut path = rss_url.path().trim_end_matches('/').to_string();
    if !path.ends_with(".rss") {
        path.push_str("/.rss");
    }
    rss_url.set_path(&path);
    Some(rss_url)
}

fn reddit_atom_to_crawl_result(
    target_url: &Url,
    rss_url: &Url,
    status_code: u16,
    atom: &str,
) -> Result<CrawlResult> {
    let feed_title = xml_tag_text(atom, "title").unwrap_or_else(|| "Reddit thread".to_string());
    let entries = parse_reddit_atom_entries(atom)?;
    if entries.is_empty() {
        bail!("reddit rss parse error: empty Atom entries");
    }

    let markdown = render_reddit_atom_markdown(target_url, &feed_title, &entries);
    let selected_chars = markdown.chars().count();
    Ok(CrawlResult {
        final_url: Some(rss_url.clone()),
        status_code: Some(status_code),
        markdown_kind: "reddit_rss_fallback",
        content_mode: "reddit_rss_fallback",
        source_kind: "reddit_thread",
        markdown,
        raw_chars: atom.chars().count(),
        selected_chars,
        elapsed_ms: None,
        entries_count: Some(entries.len()),
        noise_filtered: true,
    })
}

fn parse_reddit_atom_entries(atom: &str) -> Result<Vec<RedditAtomEntry>> {
    let mut entries = Vec::new();
    let mut rest = atom;
    while let Some(start) = rest.find("<entry") {
        let after_start = &rest[start..];
        let Some(open_end) = after_start.find('>') else {
            break;
        };
        let entry_body_start = start + open_end + 1;
        let after_body_start = &rest[entry_body_start..];
        let Some(end) = after_body_start.find("</entry>") else {
            break;
        };
        let block = &after_body_start[..end];
        rest = &after_body_start[end + "</entry>".len()..];

        let title =
            xml_tag_text(block, "title").unwrap_or_else(|| "Untitled Reddit entry".to_string());
        let author = xml_tag_block(block, "author").and_then(|author| xml_tag_text(author, "name"));
        let content_html = xml_tag_text(block, "content").unwrap_or_default();
        let markdown = html_to_markdown(&content_html)
            .unwrap_or_else(|_| content_html.clone())
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        entries.push(RedditAtomEntry {
            title,
            author,
            markdown,
        });
    }
    Ok(entries)
}

fn render_reddit_atom_markdown(
    target_url: &Url,
    feed_title: &str,
    entries: &[RedditAtomEntry],
) -> String {
    let mut output = String::new();
    output.push_str("# ");
    output.push_str(feed_title.trim());
    output.push_str("\n\n");
    output.push_str("Source: ");
    output.push_str(target_url.as_str());
    output.push_str("\nMode: reddit_rss_fallback\nEntries: ");
    output.push_str(&entries.len().to_string());
    output.push_str("\n\n");

    for (index, entry) in entries.iter().enumerate() {
        if index == 0 {
            output.push_str("## Original post\n\n");
        } else if index == 1 {
            output.push_str("## Comments\n\n");
        }

        if index > 0 {
            output.push_str("### ");
            output.push_str(&index.to_string());
            output.push_str(". ");
        } else {
            output.push_str("**");
        }
        output.push_str(entry.title.trim());
        if index == 0 {
            output.push_str("**");
        }
        output.push_str("\n\n");

        if let Some(author) = entry
            .author
            .as_deref()
            .filter(|author| !author.trim().is_empty())
        {
            output.push_str("Author: ");
            output.push_str(author.trim());
            output.push_str("\n\n");
        }
        if !entry.markdown.trim().is_empty() {
            output.push_str(entry.markdown.trim());
            output.push_str("\n\n");
        }
    }

    output.trim().to_string()
}

fn xml_tag_text(input: &str, tag: &str) -> Option<String> {
    xml_tag_block(input, tag).map(|text| html_escape::decode_html_entities(text).trim().to_string())
}

fn xml_tag_block<'a>(input: &'a str, tag: &str) -> Option<&'a str> {
    let start_marker = format!("<{tag}");
    let start = input.find(&start_marker)?;
    let after_start = &input[start..];
    let open_end = after_start.find('>')?;
    let body_start = start + open_end + 1;
    let end_marker = format!("</{tag}>");
    let end = input[body_start..].find(&end_marker)?;
    Some(&input[body_start..body_start + end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const PUBLIC_TEST_URL: &str = "http://93.184.216.34/article";

    #[derive(Clone)]
    struct MockResponse {
        status: u16,
        body: &'static str,
    }

    #[derive(Debug)]
    struct ObservedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: String,
    }

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-crawl4ai-markdown"),
            batch_id: ToolBatchId::from("batch-crawl4ai-markdown"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-crawl4ai-markdown"),
            tool_call_id: ToolCallId::from("call-crawl4ai-markdown"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(TOOL_CRAWL4AI_MARKDOWN),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    fn test_config(base_url: Url) -> Crawl4AiMarkdownConfig {
        Crawl4AiMarkdownConfig {
            base_url,
            api_token: Some("test-token".to_string()),
            default_timeout_secs: 5,
            max_timeout_secs: 10,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
            health_timeout_ms: 1_000,
            jitter_min_ms: 0,
            jitter_max_ms: 0,
            max_retries: 0,
            text_mode: true,
            light_mode: true,
            avoid_ads: true,
        }
    }

    async fn serve_crawl4ai_sequence(
        responses: Vec<MockResponse>,
    ) -> (SocketAddr, Arc<Mutex<Vec<ObservedRequest>>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().expect("local address");
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_task = Arc::clone(&observed);

        tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("accept request");
                let request = read_http_request(&mut stream).await;
                observed_for_task
                    .lock()
                    .expect("observed request lock")
                    .push(request);
                let status_text = match response.status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    503 => "Service Unavailable",
                    _ => "Error",
                };
                let raw_response = format!(
                    "HTTP/1.1 {} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(raw_response.as_bytes())
                    .await
                    .expect("write response");
            }
        });

        (addr, observed)
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> ObservedRequest {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_len = loop {
            let read = stream.read(&mut buffer).await.expect("read request");
            if read == 0 {
                break request.len();
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break header_end + 4;
            }
        };

        let headers_raw = String::from_utf8_lossy(&request[..header_len]);
        let mut lines = headers_raw.lines();
        let request_line = lines.next().expect("request line");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let headers: HashMap<String, String> = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.to_ascii_lowercase(), value.trim().to_string()))
            })
            .collect();
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        while request.len().saturating_sub(header_len) < content_length {
            let read = stream.read(&mut buffer).await.expect("read request body");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }

        let body = String::from_utf8_lossy(&request[header_len..]).to_string();
        ObservedRequest {
            method,
            path,
            headers,
            body,
        }
    }

    #[test]
    fn tool_definition_is_static_and_bounded() {
        let spec = Crawl4AiMarkdownProvider::tool_definition();

        assert_eq!(spec.name, TOOL_CRAWL4AI_MARKDOWN);
        assert!(
            spec.description
                .contains("configured Crawl4AI REST service")
        );
        assert!(
            spec.description
                .contains("Use after selecting specific URLs from brave_search or searxng_search")
        );
        assert!(
            spec.description
                .contains("Do not crawl every search result")
        );
        assert!(
            spec.description
                .contains("For Reddit thread URLs, omit max_chars or use 15000-30000")
        );
        assert_eq!(spec.parameters["required"], json!(["url"]));
        assert_eq!(spec.parameters["additionalProperties"], json!(false));
        assert!(spec.parameters["properties"].get("headers").is_none());
        assert!(spec.parameters["properties"].get("js").is_none());
        assert!(spec.parameters["properties"].get("base_url").is_none());
    }

    #[test]
    fn typed_runtime_lists_only_crawl4ai_markdown_tool() {
        let provider = Arc::new(Crawl4AiMarkdownProvider::new());
        let tools = provider.tool_runtime_executors();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name().as_str(), TOOL_CRAWL4AI_MARKDOWN);
    }

    #[tokio::test]
    async fn typed_runtime_executor_posts_expected_crawl_contract() {
        let (addr, observed) = serve_crawl4ai_sequence(vec![
            MockResponse {
                status: 200,
                body: r#"{"status":"ok"}"#,
            },
            MockResponse {
                status: 200,
                body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/final","status_code":200,"elapsed_ms":42,"markdown":{"raw_markdown":"# Rendered\n\nArticle body"}}]}"##,
            },
        ])
        .await;
        let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
        let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_CRAWL4AI_MARKDOWN)
            .expect("typed crawl4ai_markdown executor registered");

        let output = executor
            .execute(runtime_invocation(&format!(
                r#"{{"url":"{PUBLIC_TEST_URL}","timeout_secs":3,"wait_for":"main article","fresh":true,"max_chars":1000}}"#
            )))
            .await
            .expect("crawl4ai_markdown runtime output");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        let payload: Value = serde_json::from_str(stdout).expect("success payload json");
        assert_eq!(payload["provider"], json!(TOOL_CRAWL4AI_MARKDOWN));
        assert_eq!(payload["url"], json!(PUBLIC_TEST_URL));
        assert_eq!(payload["final_url"], json!("http://93.184.216.34/final"));
        assert_eq!(payload["status_code"], json!(200));
        assert_eq!(payload["markdown_kind"], json!("raw_markdown"));
        assert_eq!(payload["content_mode"], json!("crawl4ai_raw_markdown"));
        assert_eq!(payload["source_kind"], json!("web_page"));
        assert_eq!(payload["markdown"], json!("# Rendered\n\nArticle body"));
        assert_eq!(payload["selected_chars"], json!(24));
        assert_eq!(payload["entries_count"], Value::Null);
        assert_eq!(payload["noise_filtered"], json!(false));
        assert_eq!(payload["fresh"], json!(true));

        let observed = observed.lock().expect("observed request lock");
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0].method, "GET");
        assert_eq!(observed[0].path, "/health");
        assert_eq!(
            observed[0].headers.get("authorization"),
            Some(&"Bearer test-token".to_string())
        );
        assert_eq!(observed[1].method, "POST");
        assert_eq!(observed[1].path, "/crawl");
        assert_eq!(
            observed[1].headers.get("authorization"),
            Some(&"Bearer test-token".to_string())
        );
        let crawl_request: Value =
            serde_json::from_str(&observed[1].body).expect("crawl request json");
        assert_eq!(crawl_request["urls"], json!([PUBLIC_TEST_URL]));
        assert_eq!(
            crawl_request["browser_config"]["params"]["browser_type"],
            json!("chromium")
        );
        assert_eq!(
            crawl_request["browser_config"]["params"]["headless"],
            json!(true)
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["cache_mode"],
            json!("bypass")
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["wait_for"],
            json!("css:main article")
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["page_timeout"],
            json!(3000)
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["exclude_external_links"],
            json!(true)
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["exclude_social_media_links"],
            json!(true)
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["word_count_threshold"],
            json!(3)
        );
        assert_eq!(
            crawl_request["crawler_config"]["params"]["markdown_generator"]["type"],
            json!("DefaultMarkdownGenerator")
        );
        assert!(
            crawl_request["crawler_config"]["params"]
                .get("js_code")
                .is_none()
        );
    }

    #[tokio::test]
    async fn health_unavailable_returns_structured_failure() {
        let (addr, observed) = serve_crawl4ai_sequence(vec![MockResponse {
            status: 503,
            body: r#"{"status":"down"}"#,
        }])
        .await;
        let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
        let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("executor");

        let output = executor
            .execute(runtime_invocation(&format!(
                r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
            )))
            .await
            .expect("structured failure output");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        let payload = output
            .structured_payload
            .expect("structured crawl4ai failure payload");
        assert_eq!(payload["error_kind"], json!("crawl4ai_unavailable"));
        assert_eq!(payload["provider_unavailable"], json!(true));
        assert_eq!(payload["retryable"], json!(true));
        assert_eq!(payload["status_code"], json!(503));
        assert!(!payload.to_string().contains("test-token"));
        let observed = observed.lock().expect("observed request lock");
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].path, "/health");
    }

    #[tokio::test]
    async fn retries_retryable_crawl_status_once_when_configured() {
        let (addr, observed) = serve_crawl4ai_sequence(vec![
            MockResponse {
                status: 200,
                body: r#"{"status":"ok"}"#,
            },
            MockResponse {
                status: 500,
                body: r#"{"error":"temporary"}"#,
            },
            MockResponse {
                status: 200,
                body: r#"{"status":"ok"}"#,
            },
            MockResponse {
                status: 200,
                body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/article","status_code":200,"markdown":"# Retry succeeded"}]}"##,
            },
        ])
        .await;
        let mut config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
        config.max_retries = 1;
        let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("executor");

        let output = executor
            .execute(runtime_invocation(&format!(
                r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
            )))
            .await
            .expect("retry success output");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains("# Retry succeeded"));
        let observed = observed.lock().expect("observed request lock");
        let paths = observed
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["/health", "/crawl", "/health", "/crawl"]);
    }

    #[test]
    fn rejects_non_http_urls_and_no_host() {
        assert!(parse_public_http_url("file:///etc/passwd").is_err());
        assert!(parse_public_http_url("data:text/plain,hello").is_err());
        assert!(parse_public_http_url("https://").is_err());
    }

    #[test]
    fn rejects_localhost_and_private_ips() {
        for raw_url in [
            "http://localhost/page",
            "http://app.localhost/page",
            "http://127.0.0.1/page",
            "http://10.0.0.1/page",
            "http://172.16.0.1/page",
            "http://192.168.0.1/page",
            "http://169.254.169.254/latest/meta-data",
            "http://0.0.0.0/page",
            "http://255.255.255.255/page",
            "http://[::1]/page",
            "http://[::]/page",
            "http://[fd00::1]/page",
            "http://[fe80::1]/page",
            "http://[::ffff:192.168.0.1]/page",
        ] {
            let error = parse_public_http_url(raw_url).err();
            assert!(error.is_some(), "expected {raw_url} to be rejected");
        }
    }

    #[test]
    fn allows_public_url_hosts_before_dns_preflight() {
        let url = parse_public_http_url("https://example.com/page");
        assert!(url.is_ok());
    }

    #[test]
    fn rejects_direct_media_urls() {
        let url = Url::parse("https://example.com/photo.jpg").expect("url");
        assert!(reject_media_url(&url).is_err());
    }

    #[test]
    fn wait_for_accepts_only_css_selectors() {
        assert_eq!(
            normalize_wait_for(Some(".main")).expect("css selector accepted"),
            Some("css:.main".to_string())
        );
        assert_eq!(
            normalize_wait_for(Some("css:#article")).expect("prefixed css selector accepted"),
            Some("css:#article".to_string())
        );
        assert!(normalize_wait_for(Some("js:document.readyState === 'complete'")).is_err());
        assert!(normalize_wait_for(Some("() => true")).is_err());
        assert!(normalize_wait_for(Some("main; body")).is_err());
    }

    #[test]
    fn parses_markdown_string_and_object_shapes() {
        let string_result = json!({"markdown":"# Title"});
        let selected = select_markdown(&string_result).expect("string markdown");
        assert_eq!(selected.kind, "raw_markdown");
        assert_eq!(selected.text, "# Title");

        let object_result = json!({"markdown":{"raw_markdown":"# Raw navigation and article body", "markdown_with_citations":"# Cited", "fit_markdown":"# Clean article body"}});
        let selected = select_markdown(&object_result).expect("object markdown");
        assert_eq!(selected.kind, "fit_markdown");
        assert_eq!(selected.content_mode, "crawl4ai_fit_markdown");
        assert_eq!(selected.text, "# Clean article body");
        assert_eq!(selected.raw_chars, 33);
        assert!(selected.noise_filtered);
    }

    #[tokio::test]
    async fn blocked_markdown_returns_structured_failure() {
        let (addr, _observed) = serve_crawl4ai_sequence(vec![
            MockResponse {
                status: 200,
                body: r#"{"status":"ok"}"#,
            },
            MockResponse {
                status: 200,
                body: r##"{"results":[{"success":true,"url":"http://93.184.216.34/article","status_code":200,"markdown":{"fit_markdown":"You've been blocked by network security. To continue, log in to your Reddit account and use your developer token."}}]}"##,
            },
        ])
        .await;
        let config = test_config(Url::parse(&format!("http://{addr}")).expect("base url"));
        let provider = Arc::new(Crawl4AiMarkdownProvider::with_config(config));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .next()
            .expect("executor");

        let output = executor
            .execute(runtime_invocation(&format!(
                r#"{{"url":"{PUBLIC_TEST_URL}"}}"#
            )))
            .await
            .expect("structured blocked output");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        let payload = output.structured_payload.expect("failure payload");
        assert_eq!(payload["error_kind"], json!("blocked_or_noise"));
        assert!(
            payload["message"]
                .as_str()
                .unwrap_or_default()
                .contains("blocked/noise")
        );
    }

    #[test]
    fn reddit_thread_url_normalizes_to_rss() {
        let url = Url::parse(
            "https://sh.reddit.com/r/LocalLLaMA/comments/1tes1wx/mtp_support_merged_into_llamacpp/?utm_source=x#fragment",
        )
        .expect("reddit url");

        let rss_url = reddit_thread_rss_url(&url).expect("reddit rss url");

        assert_eq!(
            rss_url.as_str(),
            "https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/mtp_support_merged_into_llamacpp/.rss"
        );
    }

    #[test]
    fn reddit_atom_feed_converts_to_compact_markdown() {
        let target_url = Url::parse("https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/thread/")
            .expect("target url");
        let rss_url = reddit_thread_rss_url(&target_url).expect("rss url");
        let atom = r#"
            <feed><title>Reddit title</title>
              <entry><title>Original title</title><author><name>op_user</name></author><content type="html">&lt;p&gt;Post body &lt;strong&gt;important&lt;/strong&gt;.&lt;/p&gt;</content></entry>
              <entry><title>Comment title</title><author><name>commenter</name></author><content type="html">&lt;p&gt;Useful comment.&lt;/p&gt;</content></entry>
            </feed>
        "#;

        let result = reddit_atom_to_crawl_result(&target_url, &rss_url, 200, atom)
            .expect("reddit atom parsed");

        assert_eq!(result.content_mode, "reddit_rss_fallback");
        assert_eq!(result.source_kind, "reddit_thread");
        assert_eq!(result.entries_count, Some(2));
        assert!(result.noise_filtered);
        assert!(result.markdown.contains("Mode: reddit_rss_fallback"));
        assert!(result.markdown.contains("## Original post"));
        assert!(result.markdown.contains("Author: op_user"));
        assert!(result.markdown.contains("Useful comment"));
    }

    #[test]
    fn reddit_rss_fallback_output_respects_max_chars() {
        let target_url = Url::parse("https://www.reddit.com/r/LocalLLaMA/comments/1tes1wx/thread/")
            .expect("target url");
        let rss_url = reddit_thread_rss_url(&target_url).expect("rss url");
        let atom = r#"
            <feed><title>Reddit title</title>
              <entry><title>Original title</title><author><name>op_user</name></author><content type="html">&lt;p&gt;This is a deliberately long Reddit post body with enough text to exceed the tiny test cap.&lt;/p&gt;</content></entry>
            </feed>
        "#;
        let result = reddit_atom_to_crawl_result(&target_url, &rss_url, 200, atom)
            .expect("reddit atom parsed");
        let provider = Crawl4AiMarkdownProvider::with_config(test_config(
            Url::parse(DEFAULT_BASE_URL).expect("base url"),
        ));
        let args = Crawl4AiMarkdownArgs {
            url: target_url.to_string(),
            timeout_secs: None,
            wait_for: None,
            fresh: false,
            max_chars: Some(60),
        };

        let output = provider
            .success_payload(&args, &target_url, result, 60, Instant::now())
            .expect("success payload");
        let payload: Value = serde_json::from_str(&output).expect("payload json");

        assert_eq!(payload["truncated"], json!(true));
        assert_eq!(payload["content_mode"], json!("reddit_rss_fallback"));
        assert!(
            payload["markdown"]
                .as_str()
                .unwrap_or_default()
                .contains("... (truncated)")
        );
    }

    #[test]
    fn falls_back_to_html_when_markdown_is_empty() {
        let result = json!({
            "markdown": {"raw_markdown": "", "markdown_with_citations": "", "fit_markdown": ""},
            "html": "<html><head><title>Ignored</title></head><body><main><h1>Gemma Guide</h1><p>Article body.</p><script>ignore()</script></main></body></html>"
        });

        let selected = select_markdown(&result).expect("html fallback markdown");

        assert_eq!(selected.kind, "html_fallback");
        assert!(selected.text.contains("Gemma Guide"));
        assert!(selected.text.contains("Article body"));
        assert!(!selected.text.contains("ignore()"));
    }

    #[test]
    fn classifies_provider_unavailable_without_leaking_token() {
        let config = Crawl4AiMarkdownConfig {
            base_url: Url::parse(DEFAULT_BASE_URL).expect("url"),
            api_token: Some("secret-token".to_string()),
            default_timeout_secs: DEFAULT_TIMEOUT_SECS,
            max_timeout_secs: DEFAULT_MAX_TIMEOUT_SECS,
            max_output_chars: DEFAULT_MAX_OUTPUT_CHARS,
            health_timeout_ms: DEFAULT_HEALTH_TIMEOUT_MS,
            jitter_min_ms: DEFAULT_JITTER_MIN_MS,
            jitter_max_ms: DEFAULT_JITTER_MAX_MS,
            max_retries: DEFAULT_MAX_RETRIES,
            text_mode: true,
            light_mode: true,
            avoid_ads: true,
        };
        let args = Crawl4AiMarkdownArgs {
            url: "https://example.com".to_string(),
            timeout_secs: None,
            wait_for: None,
            fresh: false,
            max_chars: None,
        };
        let error = anyhow!("crawl4ai health request failed: connection refused");

        let payload = crawl4ai_failure_payload(Some(&args), &config, &error);

        assert_eq!(payload["error_kind"], json!("crawl4ai_unavailable"));
        assert_eq!(payload["provider_unavailable"], json!(true));
        assert_eq!(payload["retryable"], json!(true));
        assert!(!payload.to_string().contains("secret-token"));
    }
}
