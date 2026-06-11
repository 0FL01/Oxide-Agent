use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Url;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::convert::{html_to_markdown, truncate_chars};
use super::error::{display_content_type, is_html_content_type, reject_anti_bot_challenge};
use super::known_sources::{KnownMarkdownSource, classify as classify_known_source};
use super::reddit::{
    parse_reddit_atom_entries, reddit_thread_rss_url, render_reddit_atom_markdown, xml_tag_text,
};
use super::url::{parse_web_url, reject_media_url, reject_unsafe_url};
use super::{
    BROWSER_USER_AGENT, DEFAULT_TIMEOUT_SECS, MARKDOWN_ACCEPT_HEADER, MAX_OUTPUT_CHARS,
    MAX_RESPONSE_BYTES, MAX_TIMEOUT_SECS, WebFetchMdProvider, WebMarkdownArgs,
};

struct FetchResult {
    final_url: Url,
    content_type: String,
    bytes_read: usize,
    text: String,
}

impl WebFetchMdProvider {
    pub(super) async fn fetch_markdown(
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

        if let Some(source) = classify_known_source(&url) {
            match self
                .fetch_known_markdown(&source, timeout_secs, cancellation_token)
                .await
            {
                Ok(output) => return Ok(output),
                Err(error) => {
                    tracing::warn!(
                        url = url.as_str(),
                        fetch_url = source.fetch_url().as_str(),
                        mode = source.mode(),
                        error = %error,
                        "known markdown fast-path failed, trying normal fetch"
                    );
                }
            }
        }

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

    /// Fetch known Markdown sources directly, without fetching their HTML shell.
    async fn fetch_known_markdown(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        reject_unsafe_url(source.fetch_url())?;

        let fetched = self
            .fetch_text(source.fetch_url().clone(), timeout_secs, cancellation_token)
            .await
            .context("known markdown fetch failed")?;

        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };

        let truncated = truncate_chars(markdown.trim().to_string(), MAX_OUTPUT_CHARS);
        let truncated_label = if truncated.was_truncated { "yes" } else { "no" };

        Ok(format!(
            "## Web Markdown\n\nURL: {}\nSource-URL: {}\nMode: {}\nContent-Type: {}\nFetched-Bytes: {}\nTruncated: {}\n\n{}",
            fetched.final_url,
            source.source_url(),
            source.mode(),
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
