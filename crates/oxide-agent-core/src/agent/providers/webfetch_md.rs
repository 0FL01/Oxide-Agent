//! Lightweight web page fetcher.
//!
//! Provides `web_markdown`: one HTTP GET for a known URL plus optional HTML to Markdown
//! conversion. It is intentionally not a crawler, browser, or PDF exporter.

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, SERVER, USER_AGENT};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Host;

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
            && content_length > MAX_RESPONSE_BYTES as u64 {
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

fn parse_web_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("invalid URL")?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => bail!("unsupported URL scheme: {other}; only http/https are allowed"),
    }
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
        bail!(
            "web_markdown is for web pages, not direct media/PDF URLs; use a media or file-specific tool instead"
        );
    }

    Ok(())
}

fn reject_unsafe_url(url: &Url) -> Result<()> {
    match url
        .host()
        .ok_or_else(|| anyhow!("URL must include a host"))?
    {
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".localhost") {
                bail!("refusing to fetch localhost URL");
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
        bail!("refusing to fetch private, loopback, link-local, or metadata IPv4 URL");
    }

    Ok(())
}

fn reject_unsafe_ipv6(ip: Ipv6Addr) -> Result<()> {
    let first_segment = ip.segments()[0];
    let is_unique_local = (first_segment & 0xfe00) == 0xfc00;
    let is_link_local = (first_segment & 0xffc0) == 0xfe80;

    if ip.is_loopback() || ip.is_unspecified() || is_unique_local || is_link_local {
        bail!("refusing to fetch local IPv6 URL");
    }

    Ok(())
}

fn is_html_content_type(content_type: &str) -> bool {
    content_type.contains("text/html") || content_type.contains("application/xhtml+xml")
}

fn display_content_type(content_type: &str) -> &str {
    if content_type.trim().is_empty() {
        "(unknown)"
    } else {
        content_type
    }
}

fn reject_anti_bot_challenge(headers: &HeaderMap, body: &str) -> Result<()> {
    if header_contains(headers, "cf-mitigated", "challenge") {
        bail!(ANTI_BOT_ERROR);
    }

    if server_header_contains_cloudflare(headers) && body_has_cloudflare_challenge_marker(body) {
        bail!(ANTI_BOT_ERROR);
    }

    if body_has_anti_bot_marker(body) {
        bail!(ANTI_BOT_ERROR);
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
        || lower.contains("captcha")
}

fn webfetch_failure_payload(
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

fn webfetch_failure_message(args: Option<&WebMarkdownArgs>, error: &anyhow::Error) -> String {
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

fn webfetch_error_kind(error: &anyhow::Error) -> &'static str {
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

fn webfetch_host_from_url(raw_url: &str) -> Option<String> {
    Url::parse(raw_url)
        .ok()?
        .host_str()
        .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
}

fn webfetch_http_status_code(error: &anyhow::Error) -> Option<u16> {
    let message = format!("{error:#}");
    let marker = "non-success status: ";
    let status = message.split(marker).nth(1)?;
    status.split_whitespace().next()?.parse().ok()
}

fn html_to_markdown(html: &str) -> Result<String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "iframe", "object", "embed", "meta", "link", "nav",
            "footer", "aside", "form", "button", "svg", "canvas",
        ])
        .build()
        .convert(html)
        .map_err(|error| anyhow!("html to markdown conversion failed: {error}"))
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
    use reqwest::header::HeaderValue;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn runtime_invocation(raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-webfetch-md"),
            batch_id: ToolBatchId::from("batch-webfetch-md"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-web-markdown"),
            tool_call_id: ToolCallId::from("call-web-markdown"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(TOOL_WEB_MARKDOWN),
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

    async fn serve_http_once(body: &'static str, content_type: &'static str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().expect("local address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        addr
    }

    #[test]
    fn typed_runtime_lists_only_web_markdown_tool() {
        let provider = Arc::new(WebFetchMdProvider::new());
        let tools = provider.tool_runtime_executors();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name().as_str(), TOOL_WEB_MARKDOWN);
    }

    #[tokio::test]
    async fn typed_runtime_executor_fetches_web_markdown() {
        let addr = serve_http_once(
            "<html><body><main><h1>Hello</h1><p>Readable page.</p></main></body></html>",
            "text/html; charset=utf-8",
        )
        .await;
        let client = reqwest::Client::builder()
            .resolve("example.test", addr)
            .build()
            .expect("test client");
        let provider = Arc::new(WebFetchMdProvider::with_client(client));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WEB_MARKDOWN)
            .expect("typed web_markdown executor registered");

        let output = executor
            .execute(runtime_invocation(
                r#"{"url":"http://example.test/article","timeout_secs":5}"#,
            ))
            .await
            .expect("typed web_markdown succeeds");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains("URL: http://example.test/article"));
        assert!(stdout.contains("# Hello"));
        assert!(stdout.contains("Readable page."));
    }

    #[test]
    fn converts_html_to_markdown_and_skips_chrome_tags() {
        let markdown = html_to_markdown(
            r#"
            <html>
                <body>
                    <nav>skip navigation</nav>
                    <main><h1>Hello</h1><p>Readable page.</p></main>
                    <script>alert(1)</script>
                </body>
            </html>
            "#,
        );

        assert!(markdown.is_ok());
        let markdown = markdown.unwrap_or_default();
        assert!(markdown.contains("# Hello"));
        assert!(markdown.contains("Readable page."));
        assert!(!markdown.contains("skip navigation"));
        assert!(!markdown.contains("alert"));
    }

    #[test]
    fn rejects_non_http_urls() {
        let error = parse_web_url("file:///etc/passwd").err();
        assert!(error.is_some());
        assert!(error
            .map(|error| error.to_string().contains("unsupported URL scheme"))
            .unwrap_or(false));
    }

    #[test]
    fn rejects_localhost_and_private_ips() {
        let localhost = Url::parse("http://localhost/page");
        assert!(localhost.is_ok());
        assert!(localhost
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some());

        let private_ip = Url::parse("http://192.168.1.1/page");
        assert!(private_ip.is_ok());
        assert!(private_ip
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some());

        let metadata_ip = Url::parse("http://169.254.169.254/latest/meta-data");
        assert!(metadata_ip.is_ok());
        assert!(metadata_ip
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some());

        let unique_local_ipv6 = Url::parse("http://[fd00::1]/page");
        assert!(unique_local_ipv6.is_ok());
        assert!(unique_local_ipv6
            .ok()
            .and_then(|url| reject_unsafe_url(&url).err())
            .is_some());
    }

    #[test]
    fn allows_public_urls() {
        let public_url = Url::parse("https://example.com/page");
        assert!(public_url.is_ok());
        assert!(public_url
            .ok()
            .map(|url| reject_unsafe_url(&url).is_ok())
            .unwrap_or(false));
    }

    #[test]
    fn rejects_direct_media_urls() {
        let url = Url::parse("https://example.com/photo.jpg");
        assert!(url.is_ok());
        assert!(url
            .ok()
            .and_then(|url| reject_media_url(&url).err())
            .is_some());
    }

    #[test]
    fn detects_cf_mitigated_challenge_header() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-mitigated", HeaderValue::from_static("challenge"));

        let error = reject_anti_bot_challenge(&headers, "").expect_err("challenge must fail");

        assert_eq!(error.to_string(), ANTI_BOT_ERROR);
    }

    #[test]
    fn detects_cloudflare_server_with_challenge_marker() {
        let mut headers = HeaderMap::new();
        headers.insert(SERVER, HeaderValue::from_static("cloudflare"));

        let error = reject_anti_bot_challenge(&headers, "<html>challenge platform</html>")
            .expect_err("cloudflare challenge must fail");

        assert_eq!(error.to_string(), ANTI_BOT_ERROR);
    }

    #[test]
    fn detects_common_antibot_body_markers() {
        let headers = HeaderMap::new();

        for body in [
            "Just a moment...",
            "Making sure you're not a bot!",
            "Checking your browser before accessing the site",
            "Please enable JavaScript and cookies to continue",
            "Anubis uses a Proof-of-Work scheme to protect the server",
            "This page requires the use of modern JavaScript features",
            "<script src=\"/cdn-cgi/challenge-platform/h/b/cf-chl-jschl\"></script>",
            "captcha verification required",
        ] {
            let error = reject_anti_bot_challenge(&headers, body).expect_err("marker must fail");
            assert_eq!(error.to_string(), ANTI_BOT_ERROR);
        }
    }

    #[test]
    fn allows_regular_html_without_antibot_markers() {
        let headers = HeaderMap::new();

        assert!(reject_anti_bot_challenge(
            &headers,
            "<html><body><h1>Regular article</h1></body></html>",
        )
        .is_ok());
    }

    #[test]
    fn truncates_long_output() {
        let output = truncate_chars("abcdef".to_string(), 3);

        assert!(output.was_truncated);
        assert_eq!(output.text, "abc\n\n... (truncated)");
    }

    #[tokio::test]
    async fn typed_runtime_executor_returns_structured_antibot_failure() {
        let addr = serve_http_once(
            r#"<html><body><h1>Making sure you're not a bot!</h1><p>Anubis uses a Proof-of-Work scheme to protect the server.</p></body></html>"#,
            "text/html; charset=utf-8",
        )
        .await;
        let client = reqwest::Client::builder()
            .resolve("example.test", addr)
            .build()
            .expect("test client");
        let provider = Arc::new(WebFetchMdProvider::with_client(client));
        let executor = provider
            .tool_runtime_executors()
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_WEB_MARKDOWN)
            .expect("typed web_markdown executor registered");

        let output = executor
            .execute(runtime_invocation(
                r#"{"url":"http://example.test/protected","timeout_secs":5}"#,
            ))
            .await
            .expect("typed web_markdown returns failure output");

        assert_eq!(output.status, ToolOutputStatus::Failure);
        assert!(output
            .error_message
            .as_deref()
            .expect("error message")
            .contains("anti-bot protection at example.test"));

        let payload = output.structured_payload.expect("structured payload");
        assert_eq!(
            payload.get("provider").and_then(|value| value.as_str()),
            Some("web_markdown")
        );
        assert_eq!(
            payload.get("error_kind").and_then(|value| value.as_str()),
            Some("anti_bot")
        );
        assert_eq!(
            payload.get("host").and_then(|value| value.as_str()),
            Some("example.test")
        );
        assert_eq!(
            payload
                .get("provider_unavailable")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            payload.get("retryable").and_then(|value| value.as_bool()),
            Some(false)
        );
    }
}
