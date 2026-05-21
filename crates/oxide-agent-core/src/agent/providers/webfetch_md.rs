//! Lightweight web page fetcher.
//!
//! Provides `web_markdown`: one HTTP GET for a known URL plus optional HTML to Markdown
//! conversion. It is intentionally not a crawler, browser, or PDF exporter.

use crate::agent::provider::ToolProvider;
use crate::llm::ToolDefinition;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// Local provider for fetching a single URL as Markdown.
pub struct WebFetchMdProvider {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
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
            .context("request failed")?
            .error_for_status()
            .context("server returned non-success status")?;

        let final_url = response.url().clone();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if let Some(content_length) = response.content_length() {
            if content_length > MAX_RESPONSE_BYTES as u64 {
                bail!(
                    "response too large by content-length: {} bytes; max is {}",
                    content_length,
                    MAX_RESPONSE_BYTES
                );
            }
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let bytes_read = body.len();
        let text = String::from_utf8_lossy(&body).into_owned();

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

#[async_trait]
impl ToolProvider for WebFetchMdProvider {
    fn name(&self) -> &'static str {
        "webfetch_md"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
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
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == TOOL_WEB_MARKDOWN
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        if tool_name != TOOL_WEB_MARKDOWN {
            bail!("unknown webfetch_md tool: {tool_name}");
        }

        let args: WebMarkdownArgs =
            serde_json::from_str(arguments).context("invalid web_markdown arguments")?;
        self.fetch_markdown(args, cancellation_token).await
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
    use crate::agent::provider::ToolProvider;

    #[test]
    fn lists_only_web_markdown_tool() {
        let provider = WebFetchMdProvider::new();
        let tools = provider.tools();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, TOOL_WEB_MARKDOWN);
        assert!(provider.can_handle(TOOL_WEB_MARKDOWN));
        assert!(!provider.can_handle("web_search"));
        assert!(!provider.can_handle("web_extract"));
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
    fn truncates_long_output() {
        let output = truncate_chars("abcdef".to_string(), 3);

        assert!(output.was_truncated);
        assert_eq!(output.text, "abc\n\n... (truncated)");
    }
}
