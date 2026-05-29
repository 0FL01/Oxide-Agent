use super::backoff::{self, BackoffConfig};
use super::error::DuckDuckGoError;
use super::rate_limit::DuckDuckGoRateLimiter;
use super::types::{
    DuckDuckGoNewsArgs, DuckDuckGoNewsResult, DuckDuckGoSearchArgs, DuckDuckGoSearchResult,
};
use crate::config::{
    get_duckduckgo_backoff_config, get_duckduckgo_browser_config, get_duckduckgo_rate_limit_config,
    get_duckduckgo_timeout, DuckDuckGoBrowserConfig,
};
use duckduckgo::browser::Browser;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

const FALLBACK_USER_AGENT: &str = "Mozilla/5.0";

pub struct DuckDuckGoClient {
    browser: Browser,
    user_agent: String,
    timeout: Duration,
    limiter: Arc<DuckDuckGoRateLimiter>,
    max_retries: usize,
    backoff: BackoffConfig,
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
            .filter(|value| !value.trim().is_empty())
        {
            builder = builder.proxy(proxy);
        }

        let browser = builder
            .build()
            .map_err(|error| DuckDuckGoError::ClientInit(error.to_string()))?;

        Ok(Self {
            browser,
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
        let _permit = self.limiter.acquire().await?;
        let future = self
            .browser
            .lite_search(query, region, Some(limit), &self.user_agent);
        let results = tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| DuckDuckGoError::Timeout)?
            .map_err(|error| DuckDuckGoError::Request(error.to_string()))?;

        Ok(results
            .into_iter()
            .map(|result| DuckDuckGoSearchResult {
                title: result.title,
                url: result.url,
                snippet: result.snippet,
            })
            .collect())
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
