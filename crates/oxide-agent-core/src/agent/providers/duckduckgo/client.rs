use super::backoff::{self, BackoffConfig};
use super::error::DuckDuckGoError;
use super::rate_limit::DuckDuckGoRateLimiter;
use super::types::{
    DuckDuckGoNewsArgs, DuckDuckGoNewsResult, DuckDuckGoSearchArgs, DuckDuckGoSearchResult,
};
use crate::config::{
    DuckDuckGoBrowserConfig, get_duckduckgo_backoff_config, get_duckduckgo_browser_config,
    get_duckduckgo_rate_limit_config, get_duckduckgo_timeout,
};
use duckduckgo::browser::Browser;
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CACHE_CONTROL, HeaderMap, HeaderName, HeaderValue, ORIGIN, PRAGMA,
    REFERER,
};
use reqwest::{Client as HttpClient, StatusCode};
use scraper::{Html, Selector};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use url::Url;

const FALLBACK_USER_AGENT: &str = "Mozilla/5.0";
const DDG_HTML_URL: &str = "https://html.duckduckgo.com/html/";
const DDG_LITE_URL: &str = "https://lite.duckduckgo.com/lite/";
const DDG_URL_BASE: &str = "https://duckduckgo.com";

pub struct DuckDuckGoClient {
    browser: Browser,
    http: HttpClient,
    user_agent: String,
    timeout: Duration,
    limiter: Arc<DuckDuckGoRateLimiter>,
    max_retries: usize,
    backoff: BackoffConfig,
}

#[derive(Debug, Clone, Copy)]
enum SearchBackend {
    Html,
    Lite,
}

impl SearchBackend {
    const fn url(self) -> &'static str {
        match self {
            Self::Html => DDG_HTML_URL,
            Self::Lite => DDG_LITE_URL,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Lite => "lite",
        }
    }
}

struct ParsedSearchPage {
    results: Vec<DuckDuckGoSearchResult>,
    explicit_no_results: bool,
}

impl DuckDuckGoClient {
    pub fn from_config() -> Result<Self, DuckDuckGoError> {
        Self::new(
            get_duckduckgo_browser_config(),
            Duration::from_secs(get_duckduckgo_timeout()),
            get_duckduckgo_rate_limit_config(),
            get_duckduckgo_backoff_config(),
        )
    }

    pub fn new(
        browser_config: DuckDuckGoBrowserConfig,
        timeout: Duration,
        rate_limit_config: crate::config::DuckDuckGoRateLimitConfig,
        retry_config: crate::config::DuckDuckGoBackoffConfig,
    ) -> Result<Self, DuckDuckGoError> {
        let user_agent = resolve_user_agent(browser_config.user_agent.as_deref());
        let mut builder = Browser::builder()
            .user_agent(user_agent.clone())
            .cookie_store(true);
        if let Some(proxy) = browser_config
            .proxy_url
            .clone()
            .filter(|value| !value.trim().is_empty())
        {
            builder = builder.proxy(proxy);
        }

        let browser = builder
            .build()
            .map_err(|error| DuckDuckGoError::ClientInit(error.to_string()))?;

        let mut http_builder = HttpClient::builder()
            .cookie_store(true)
            .user_agent(user_agent.clone())
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(5))
            .default_headers(default_headers(&user_agent));
        if let Some(proxy) = browser_config
            .proxy_url
            .filter(|value| !value.trim().is_empty())
        {
            let proxy = reqwest::Proxy::all(proxy)
                .map_err(|error| DuckDuckGoError::ClientInit(error.to_string()))?;
            http_builder = http_builder.proxy(proxy);
        }
        let http = http_builder
            .build()
            .map_err(|error| DuckDuckGoError::ClientInit(error.to_string()))?;

        Ok(Self {
            browser,
            http,
            user_agent,
            timeout,
            limiter: DuckDuckGoRateLimiter::global(rate_limit_config.into()),
            max_retries: usize::from(retry_config.max_retries),
            backoff: BackoffConfig {
                initial: Duration::from_millis(retry_config.initial_backoff_ms),
                max: Duration::from_millis(retry_config.max_backoff_ms),
            },
        })
    }

    pub async fn lite_search(
        &self,
        args: &DuckDuckGoSearchArgs,
    ) -> Result<Vec<DuckDuckGoSearchResult>, DuckDuckGoError> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(DuckDuckGoError::EmptyQuery);
        }

        let region = args.normalized_region();
        let limit = args.normalized_max_results();
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match self.lite_search_once(query, region, limit).await {
                Ok(results) => return Ok(results),
                Err(error) if error.is_retryable() && attempt < self.max_retries => {
                    self.maybe_mark_cooldown(&error).await;
                    let delay = backoff::retry_delay(attempt + 1, self.backoff);
                    warn!(
                        query = %query,
                        attempt = attempt + 1,
                        max_retries = self.max_retries,
                        error = %error,
                        retry_after_ms = delay.as_millis() as u64,
                        "DuckDuckGo search transient error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(error);
                }
                Err(error) => {
                    self.maybe_mark_cooldown(&error).await;
                    return Err(error);
                }
            }
        }

        Err(last_error.unwrap_or(DuckDuckGoError::Request(
            "request failed without an error".to_string(),
        )))
    }

    pub async fn news(
        &self,
        args: &DuckDuckGoNewsArgs,
    ) -> Result<Vec<DuckDuckGoNewsResult>, DuckDuckGoError> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(DuckDuckGoError::EmptyQuery);
        }

        let region = args.normalized_region();
        let limit = args.normalized_max_results();
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match self.news_once(query, region, args.safe_search, limit).await {
                Ok(results) => return Ok(results),
                Err(error) if error.is_retryable() && attempt < self.max_retries => {
                    self.maybe_mark_cooldown(&error).await;
                    let delay = backoff::retry_delay(attempt + 1, self.backoff);
                    warn!(
                        query = %query,
                        attempt = attempt + 1,
                        max_retries = self.max_retries,
                        error = %error,
                        retry_after_ms = delay.as_millis() as u64,
                        "DuckDuckGo news transient error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(error);
                }
                Err(error) => {
                    self.maybe_mark_cooldown(&error).await;
                    return Err(error);
                }
            }
        }

        Err(last_error.unwrap_or(DuckDuckGoError::Request(
            "request failed without an error".to_string(),
        )))
    }

    async fn lite_search_once(
        &self,
        query: &str,
        region: &str,
        limit: usize,
    ) -> Result<Vec<DuckDuckGoSearchResult>, DuckDuckGoError> {
        match self
            .search_backend(SearchBackend::Html, query, region, limit)
            .await
        {
            Ok(results) => Ok(results),
            Err(primary_error) if should_try_lite_backend(&primary_error) => {
                warn!(
                    query = %query,
                    region = %region,
                    error = %primary_error,
                    "DuckDuckGo html backend failed, trying lite backend"
                );
                match self
                    .search_backend(SearchBackend::Lite, query, region, limit)
                    .await
                {
                    Ok(results) => Ok(results),
                    Err(
                        lite_error @ (DuckDuckGoError::Blocked(_) | DuckDuckGoError::RateLimited),
                    ) => Err(lite_error),
                    Err(lite_error) => Err(DuckDuckGoError::ParserBreak(format!(
                        "html backend failed: {primary_error}; lite backend failed: {lite_error}"
                    ))),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn search_backend(
        &self,
        backend: SearchBackend,
        query: &str,
        region: &str,
        limit: usize,
    ) -> Result<Vec<DuckDuckGoSearchResult>, DuckDuckGoError> {
        let _permit = self.limiter.acquire().await?;
        let request = self
            .http
            .post(backend.url())
            .header(REFERER, DDG_URL_BASE)
            .form(&[("q", query), ("kl", region), ("ia", "web"), ("df", "")]);

        let response = tokio::time::timeout(self.timeout, request.send())
            .await
            .map_err(|_| DuckDuckGoError::Timeout)?
            .map_err(|error| DuckDuckGoError::Request(error.to_string()))?;

        let status = response.status();
        let final_url = response.url().to_string();
        let body = response
            .text()
            .await
            .map_err(|error| DuckDuckGoError::Request(error.to_string()))?;

        reject_block_or_bad_status(backend, status, &final_url, &body)?;

        let parsed = parse_search_page(backend, &body, limit);
        if !parsed.results.is_empty() || parsed.explicit_no_results {
            return Ok(parsed.results);
        }

        Err(DuckDuckGoError::ParserBreak(format!(
            "{} backend returned HTML but no recognizable result nodes; status={status}; url={final_url}; body_len={}",
            backend.name(),
            body.len()
        )))
    }

    async fn news_once(
        &self,
        query: &str,
        region: &str,
        safe_search: bool,
        limit: usize,
    ) -> Result<Vec<DuckDuckGoNewsResult>, DuckDuckGoError> {
        let _permit = self.limiter.acquire().await?;
        let future = self
            .browser
            .news(query, region, safe_search, Some(limit), &self.user_agent);
        let results = tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| DuckDuckGoError::Timeout)?
            .map_err(|error| DuckDuckGoError::Request(error.to_string()))?;

        Ok(results
            .into_iter()
            .map(|result| DuckDuckGoNewsResult {
                date: result.date,
                title: result.title,
                source: result.source,
                url: result.url,
                snippet: result.body,
                image: result.image,
            })
            .collect())
    }

    async fn maybe_mark_cooldown(&self, error: &DuckDuckGoError) {
        if error.should_cooldown() {
            self.limiter.mark_cooldown().await;
        }
    }
}

fn default_headers(user_agent: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(user_agent) {
        headers.insert(reqwest::header::USER_AGENT, value);
    }
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://duckduckgo.com"));
    headers.insert(REFERER, HeaderValue::from_static("https://duckduckgo.com/"));
    headers.insert(
        HeaderName::from_static("dnt"),
        HeaderValue::from_static("1"),
    );
    headers
}

fn reject_block_or_bad_status(
    backend: SearchBackend,
    status: StatusCode,
    final_url: &str,
    body: &str,
) -> Result<(), DuckDuckGoError> {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(DuckDuckGoError::RateLimited);
    }
    if status == StatusCode::FORBIDDEN || status == StatusCode::ACCEPTED {
        return Err(DuckDuckGoError::Blocked(format!(
            "{} backend returned HTTP {status}; url={final_url}",
            backend.name()
        )));
    }
    if !status.is_success() {
        return Err(DuckDuckGoError::Request(format!(
            "{} backend returned HTTP {status}; url={final_url}",
            backend.name()
        )));
    }

    let lower = body.to_ascii_lowercase();
    let looks_like_challenge = lower.contains("captcha")
        || lower.contains("anomaly")
        || lower.contains("unusual traffic")
        || lower.contains("prove you are human")
        || lower.contains("not a robot")
        || lower.contains("challenge-form")
        || lower.contains("/anomaly.js")
        || lower.contains("bots use duckduckgo");
    if looks_like_challenge {
        return Err(DuckDuckGoError::Blocked(format!(
            "{} backend returned a CAPTCHA/block page; url={final_url}; body_len={}",
            backend.name(),
            body.len()
        )));
    }

    Ok(())
}

fn parse_search_page(backend: SearchBackend, body: &str, limit: usize) -> ParsedSearchPage {
    match backend {
        SearchBackend::Html => parse_html_results(body, limit),
        SearchBackend::Lite => parse_lite_results(body, limit),
    }
}

fn parse_html_results(body: &str, limit: usize) -> ParsedSearchPage {
    let document = Html::parse_document(body);
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for selector in [".result", ".web-result", "div[class*='result']"] {
        let Some(result_selector) = parse_selector(selector) else {
            continue;
        };
        for node in document.select(&result_selector) {
            if let Some(result) = parse_html_result_node(node.html().as_str()) {
                push_unique_result(&mut results, &mut seen, result, limit);
            }
            if results.len() >= limit {
                break;
            }
        }
        if !results.is_empty() {
            break;
        }
    }

    ParsedSearchPage {
        results,
        explicit_no_results: has_no_results_marker(body),
    }
}

fn parse_html_result_node(fragment: &str) -> Option<DuckDuckGoSearchResult> {
    let document = Html::parse_fragment(fragment);
    let link = first_link(
        &document,
        &[
            "a.result__a",
            ".result__title a",
            "h2 a",
            "a[href*='uddg=']",
        ],
    )?;
    let url = normalize_result_url(&link.1)?;
    let snippet = first_text(
        &document,
        &[
            ".result__snippet",
            ".result__body",
            ".result__extras__url",
            "a.result__snippet",
        ],
    )
    .unwrap_or_default();

    Some(DuckDuckGoSearchResult {
        title: link.0,
        url,
        snippet,
    })
}

fn parse_lite_results(body: &str, limit: usize) -> ParsedSearchPage {
    let document = Html::parse_document(body);
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for selector in [
        "a.result-link",
        "td.result-link a",
        "tr a[href*='uddg=']",
        "a[href^='http']",
        "a[href^='/l/']",
    ] {
        let Some(link_selector) = parse_selector(selector) else {
            continue;
        };
        for anchor in document.select(&link_selector) {
            let title = normalize_text(anchor.text().collect::<Vec<_>>().join(" ").as_str());
            let Some(href) = anchor.value().attr("href") else {
                continue;
            };
            let Some(url) = normalize_result_url(href) else {
                continue;
            };
            let snippet = lite_snippet_near_anchor(&anchor.html(), body);
            push_unique_result(
                &mut results,
                &mut seen,
                DuckDuckGoSearchResult {
                    title,
                    url,
                    snippet,
                },
                limit,
            );
            if results.len() >= limit {
                break;
            }
        }
        if !results.is_empty() {
            break;
        }
    }

    ParsedSearchPage {
        results,
        explicit_no_results: has_no_results_marker(body),
    }
}

fn first_link(document: &Html, selectors: &[&str]) -> Option<(String, String)> {
    for selector in selectors {
        let Some(selector) = parse_selector(selector) else {
            continue;
        };
        for anchor in document.select(&selector) {
            let title = normalize_text(anchor.text().collect::<Vec<_>>().join(" ").as_str());
            let href = anchor.value().attr("href").unwrap_or_default().trim();
            if !title.is_empty() && !href.is_empty() {
                return Some((title, href.to_string()));
            }
        }
    }
    None
}

fn first_text(document: &Html, selectors: &[&str]) -> Option<String> {
    for selector in selectors {
        let Some(selector) = parse_selector(selector) else {
            continue;
        };
        for node in document.select(&selector) {
            let text = normalize_text(node.text().collect::<Vec<_>>().join(" ").as_str());
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn parse_selector(selector: &str) -> Option<Selector> {
    Selector::parse(selector).ok()
}

fn push_unique_result(
    results: &mut Vec<DuckDuckGoSearchResult>,
    seen: &mut HashSet<String>,
    result: DuckDuckGoSearchResult,
    limit: usize,
) {
    if results.len() >= limit || result.title.trim().is_empty() || result.url.trim().is_empty() {
        return;
    }
    let key = result.url.trim_end_matches('/').to_ascii_lowercase();
    if seen.insert(key) {
        results.push(result);
    }
}

fn normalize_result_url(raw_url: &str) -> Option<String> {
    let raw_url = html_escape::decode_html_entities(raw_url).to_string();
    let raw_url = raw_url.trim();
    if raw_url.is_empty() || raw_url.starts_with("javascript:") || raw_url.starts_with('#') {
        return None;
    }

    let url = Url::parse(raw_url)
        .or_else(|_| Url::parse(DDG_URL_BASE).and_then(|base| base.join(raw_url)));
    let Ok(url) = url else {
        return None;
    };

    if let Some(decoded) = decode_uddg_url(&url) {
        return Some(decoded);
    }

    if matches!(url.scheme(), "http" | "https")
        && !url
            .host_str()
            .is_some_and(|host| host.ends_with("duckduckgo.com"))
    {
        return Some(url.to_string());
    }

    None
}

fn decode_uddg_url(url: &Url) -> Option<String> {
    for (key, value) in url.query_pairs() {
        if key == "uddg" {
            let decoded = value.to_string();
            if decoded.starts_with("http://") || decoded.starts_with("https://") {
                return Some(decoded);
            }
        }
    }
    None
}

fn normalize_text(text: &str) -> String {
    html_escape::decode_html_entities(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn lite_snippet_near_anchor(anchor_html: &str, body: &str) -> String {
    let Some(anchor_pos) = body.find(anchor_html) else {
        return String::new();
    };
    let end = anchor_pos.saturating_add(2_000).min(body.len());
    let window = &body[anchor_pos..end];
    let fragment = Html::parse_fragment(window);
    first_text(
        &fragment,
        &["td.result-snippet", ".result-snippet", "td:nth-child(2)"],
    )
    .unwrap_or_default()
}

fn has_no_results_marker(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("no results")
        || lower.contains("not many results")
        || lower.contains("did not find results")
        || lower.contains("no more results")
}

fn should_try_lite_backend(error: &DuckDuckGoError) -> bool {
    matches!(
        error,
        DuckDuckGoError::ParserBreak(_) | DuckDuckGoError::Timeout | DuckDuckGoError::Request(_)
    )
}

fn resolve_user_agent(configured: Option<&str>) -> String {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    match configured {
        Some(value) => duckduckgo::user_agents::get(value)
            .unwrap_or(value)
            .to_string(),
        None => duckduckgo::user_agents::get("firefox")
            .unwrap_or(FALLBACK_USER_AGENT)
            .to_string(),
    }
}

impl From<crate::config::DuckDuckGoRateLimitConfig>
    for super::rate_limit::DuckDuckGoRateLimitConfig
{
    fn from(value: crate::config::DuckDuckGoRateLimitConfig) -> Self {
        Self {
            max_concurrent: value.max_concurrent,
            min_delay: Duration::from_millis(value.min_delay_ms),
            jitter: Duration::from_millis(value.jitter_ms),
            cooldown: Duration::from_secs(value.cooldown_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_duckduckgo_redirect_url() {
        let url = normalize_result_url("/l/?kh=-1&uddg=https%3A%2F%2Fexample.com%2Fpath%3Fa%3D1");
        assert_eq!(url.as_deref(), Some("https://example.com/path?a=1"));
    }

    #[test]
    fn detects_parser_break_for_empty_html_without_no_results_marker() {
        let parsed = parse_html_results("<html><body>hello</body></html>", 10);
        assert!(parsed.results.is_empty());
        assert!(!parsed.explicit_no_results);
    }

    #[test]
    fn parses_html_result_anchor_and_snippet() {
        let html = r#"
            <div class="result">
              <h2 class="result__title"><a class="result__a" href="/l/?uddg=https%3A%2F%2Fexample.com%2F">Example</a></h2>
              <a class="result__snippet">Example snippet</a>
            </div>
        "#;
        let parsed = parse_html_results(html, 10);
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].url, "https://example.com/");
        assert_eq!(parsed.results[0].title, "Example");
        assert_eq!(parsed.results[0].snippet, "Example snippet");
    }
}
