//! Crawl orchestration: retry loop, HTTP request assembly, response body reading.

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::constants::*;
use super::env_helpers::*;
use super::errors::*;
use super::reddit_rss::*;
use super::response::*;
use super::types::*;
use super::url_validation::*;

use super::Crawl4AiMarkdownProvider;

impl Crawl4AiMarkdownProvider {
    pub(super) async fn crawl_markdown(
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

    pub(super) fn success_payload(
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

pub(super) async fn read_limited_body(
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
