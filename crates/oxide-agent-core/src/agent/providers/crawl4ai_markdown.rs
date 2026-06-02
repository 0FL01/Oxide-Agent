//! Browser-rendered Markdown via a configured Crawl4AI REST service.
//!
//! Provides `crawl4ai_markdown`: one validated public URL, one `POST /crawl`,
//! bounded Markdown output. Oxide does not manage Crawl4AI lifecycle.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use url::Host;

const TOOL_CRAWL4AI_MARKDOWN: &str = "crawl4ai_markdown";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11235";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_MAX_TIMEOUT_SECS: u64 = 120;
const DEFAULT_OUTPUT_CHARS: usize = 20_000;
const DEFAULT_MAX_OUTPUT_CHARS: usize = 30_000;
const DEFAULT_HEALTH_TIMEOUT_MS: u64 = 1_500;
const DEFAULT_JITTER_MIN_MS: u64 = 250;
const DEFAULT_JITTER_MAX_MS: u64 = 1_500;
const DEFAULT_MAX_RETRIES: usize = 0;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_WAIT_FOR_CHARS: usize = 256;
const ERROR_MESSAGE_MAX_CHARS: usize = 1_000;
const RESPONSE_TAIL_MAX_CHARS: usize = 2_000;

/// Native provider for browser-rendered Markdown through Crawl4AI REST.
pub struct Crawl4AiMarkdownProvider {
    client: reqwest::Client,
    config: Crawl4AiMarkdownConfig,
}

#[derive(Debug, Clone)]
struct Crawl4AiMarkdownConfig {
    base_url: Url,
    api_token: Option<String>,
    default_timeout_secs: u64,
    max_timeout_secs: u64,
    max_output_chars: usize,
    health_timeout_ms: u64,
    jitter_min_ms: u64,
    jitter_max_ms: u64,
    max_retries: usize,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Crawl4AiMarkdownArgs {
    url: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    wait_for: Option<String>,
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    max_chars: Option<usize>,
}

struct CrawlResult {
    final_url: Option<Url>,
    status_code: Option<u16>,
    markdown_kind: &'static str,
    markdown: String,
    elapsed_ms: Option<u64>,
}

struct MarkdownSelection {
    kind: &'static str,
    text: String,
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
            description: "Open one http/https URL with the configured Crawl4AI REST service and return bounded Markdown. Use for pages that need browser rendering, JavaScript, overlay/consent handling, or when web_markdown fails. This tool does not crawl multiple pages, execute JavaScript, run hooks, use LLM extraction, or return screenshots/PDFs.".to_string(),
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
            "override_navigator": true
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
                    "enable_stealth": true
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
            "markdown": markdown.text,
            "truncated": markdown.was_truncated,
            "chars": markdown.text.chars().count(),
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

fn parse_public_http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("invalid URL")?;
    match url.scheme() {
        "http" | "https" => {}
        other => bail!("unsupported URL scheme: {other}; only http/https are allowed"),
    }
    reject_unsafe_url_host(&url)?;
    Ok(url)
}

async fn dns_preflight_public(url: &Url) -> Result<()> {
    let Some(Host::Domain(domain)) = url.host() else {
        return Ok(());
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL must include a known port for DNS preflight"))?;
    let host = domain.trim_end_matches('.').to_ascii_lowercase();
    let records = tokio::net::lookup_host((host.as_str(), port))
        .await
        .with_context(|| format!("dns preflight failed for host: {host}"))?;

    let mut saw_record = false;
    for addr in records {
        saw_record = true;
        reject_unsafe_ip(addr.ip())?;
    }
    if !saw_record {
        bail!("dns preflight returned no records for host: {host}");
    }
    Ok(())
}

fn reject_unsafe_url_host(url: &Url) -> Result<()> {
    match url
        .host()
        .ok_or_else(|| anyhow!("URL must include a host"))?
    {
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".localhost") {
                bail!("refusing to crawl localhost URL");
            }
        }
        Host::Ipv4(ipv4) => reject_unsafe_ip(IpAddr::V4(ipv4))?,
        Host::Ipv6(ipv6) => reject_unsafe_ip(IpAddr::V6(ipv6))?,
    }
    Ok(())
}

fn reject_unsafe_ip(ip: IpAddr) -> Result<()> {
    match ip {
        IpAddr::V4(ipv4) => reject_unsafe_ipv4(ipv4),
        IpAddr::V6(ipv6) => reject_unsafe_ipv6(ipv6),
    }
}

fn reject_unsafe_ipv4(ip: Ipv4Addr) -> Result<()> {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets() == [169, 254, 169, 254]
    {
        bail!(
            "refusing to crawl private, loopback, link-local, documentation, or metadata IPv4 URL"
        );
    }
    Ok(())
}

fn reject_unsafe_ipv6(ip: Ipv6Addr) -> Result<()> {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return reject_unsafe_ipv4(mapped);
    }

    let first_segment = ip.segments()[0];
    let is_unique_local = (first_segment & 0xfe00) == 0xfc00;
    let is_link_local = (first_segment & 0xffc0) == 0xfe80;

    if ip.is_loopback() || ip.is_unspecified() || is_unique_local || is_link_local {
        bail!("refusing to crawl local IPv6 URL");
    }
    Ok(())
}

fn reject_media_url(url: &Url) -> Result<()> {
    let path = url.path().to_ascii_lowercase();
    if matches!(
        path.rsplit('.').next(),
        Some(
            "gif"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "bmp"
                | "svg"
                | "mp4"
                | "mov"
                | "webm"
                | "mkv"
                | "avi"
                | "mp3"
                | "wav"
                | "flac"
                | "ogg"
                | "pdf"
        )
    ) {
        bail!("crawl4ai_markdown is for web pages, not direct media/PDF URLs");
    }
    Ok(())
}

fn normalize_wait_for(wait_for: Option<&str>) -> Result<Option<String>> {
    let Some(selector) = wait_for.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if selector.chars().count() > MAX_WAIT_FOR_CHARS {
        bail!("wait_for selector is too long; max is {MAX_WAIT_FOR_CHARS} chars");
    }

    let lower = selector.to_ascii_lowercase();
    if lower.starts_with("js:")
        || lower.contains("function")
        || lower.contains("=>")
        || selector.contains('{')
        || selector.contains('}')
        || selector.contains(';')
        || selector.contains('\n')
        || selector.contains('\r')
    {
        bail!("wait_for accepts only CSS selectors, not JavaScript conditions");
    }

    Ok(Some(if selector.starts_with("css:") {
        selector.to_string()
    } else {
        format!("css:{selector}")
    }))
}

async fn parse_crawl_response(body: &[u8]) -> Result<CrawlResult> {
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

    Ok(CrawlResult {
        final_url,
        status_code: result
            .get("status_code")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        markdown_kind: markdown.kind,
        markdown: markdown.text,
        elapsed_ms: result.get("elapsed_ms").and_then(Value::as_u64),
    })
}

async fn parse_final_url(result: &Value) -> Result<Option<Url>> {
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

fn select_markdown(result: &Value) -> Result<MarkdownSelection> {
    let markdown = result
        .get("markdown")
        .ok_or_else(|| anyhow!("crawl4ai response parse error: missing markdown"))?;

    if let Some(text) = markdown.as_str().filter(|text| !text.trim().is_empty()) {
        return Ok(MarkdownSelection {
            kind: "raw_markdown",
            text: text.to_string(),
        });
    }

    let object = markdown
        .as_object()
        .ok_or_else(|| anyhow!("crawl4ai response parse error: unsupported markdown shape"))?;
    for (kind, field) in [
        ("raw_markdown", "raw_markdown"),
        ("markdown_with_citations", "markdown_with_citations"),
        ("fit_markdown", "fit_markdown"),
    ] {
        if let Some(text) = object.get(field).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return Ok(MarkdownSelection {
                    kind,
                    text: text.to_string(),
                });
            }
        }
    }

    bail!("crawl4ai response parse error: empty markdown")
}

fn crawl4ai_failure_payload(
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
        "provider_unavailable": error_kind == "crawl4ai_unavailable",
        "message": crawl4ai_failure_message(args, config, error),
        "response_tail": crawl4ai_response_tail(error)
    })
}

fn crawl4ai_failure_message(
    _args: Option<&Crawl4AiMarkdownArgs>,
    _config: &Crawl4AiMarkdownConfig,
    error: &anyhow::Error,
) -> String {
    truncate_for_message(&format!("{error:#}"), ERROR_MESSAGE_MAX_CHARS)
}

fn crawl4ai_error_kind(error: &anyhow::Error) -> &'static str {
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
    } else if message.contains("crawl4ai returned non-success status") {
        "crawl4ai_http_status"
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

fn crawl4ai_error_retryable(error_kind: &str, error: &anyhow::Error) -> bool {
    match error_kind {
        "crawl4ai_unavailable" | "timeout" | "network" => true,
        "crawl4ai_http_status" => crawl4ai_http_status_code(error)
            .is_some_and(|status| status == 429 || (500..=599).contains(&status)),
        _ => false,
    }
}

fn crawl4ai_http_status_error(status: u16, body: &[u8]) -> anyhow::Error {
    let tail = response_tail(body, RESPONSE_TAIL_MAX_CHARS);
    if status == 401 || status == 403 {
        anyhow!("crawl4ai auth failed with status: {status}; response_tail: {tail}")
    } else {
        anyhow!("crawl4ai returned non-success status: {status}; response_tail: {tail}")
    }
}

fn crawl4ai_http_status_code(error: &anyhow::Error) -> Option<u16> {
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

fn crawl4ai_response_tail(error: &anyhow::Error) -> Option<String> {
    let message = format!("{error:#}");
    message
        .split("response_tail: ")
        .nth(1)
        .map(|tail| truncate_for_message(tail, RESPONSE_TAIL_MAX_CHARS))
}

fn host_from_url(raw_url: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()?
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

fn ensure_not_cancelled(cancellation_token: Option<&CancellationToken>) -> Result<()> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        bail!("crawl4ai_markdown cancelled before request");
    }
    Ok(())
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_url(name: &str, default: &str) -> Url {
    let raw = env_non_empty(name).unwrap_or_else(|| default.to_string());
    Url::parse(&raw)
        .unwrap_or_else(|_| Url::parse(default).expect("valid default Crawl4AI base URL"))
}

fn env_u64(name: &str, default: u64) -> u64 {
    env_non_empty(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    env_non_empty(name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

struct TruncatedOutput {
    text: String,
    was_truncated: bool,
}

fn truncate_chars(input: String, max_chars: usize) -> TruncatedOutput {
    if input.chars().count() <= max_chars {
        return TruncatedOutput {
            text: input,
            was_truncated: false,
        };
    }

    let mut text = input.chars().take(max_chars).collect::<String>();
    text.push_str("\n\n... (truncated)");
    TruncatedOutput {
        text,
        was_truncated: true,
    }
}

fn truncate_for_message(input: &str, max_chars: usize) -> String {
    truncate_chars(input.to_string(), max_chars).text
}

fn response_tail(body: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(body);
    let total_chars = text.chars().count();
    if total_chars <= max_chars {
        return text.into_owned();
    }
    text.chars()
        .skip(total_chars.saturating_sub(max_chars))
        .collect()
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
        assert!(spec
            .description
            .contains("configured Crawl4AI REST service"));
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
        assert_eq!(payload["markdown"], json!("# Rendered\n\nArticle body"));
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
        assert!(crawl_request["crawler_config"]["params"]
            .get("js_code")
            .is_none());
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

        let object_result =
            json!({"markdown":{"raw_markdown":"", "markdown_with_citations":"# Cited"}});
        let selected = select_markdown(&object_result).expect("object markdown");
        assert_eq!(selected.kind, "markdown_with_citations");
        assert_eq!(selected.text, "# Cited");
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
