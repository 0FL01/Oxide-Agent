use super::error::CrwError;
use super::types::{CrwScrapeArgs, CrwScrapeResponse, CrwSearchArgs, CrwSearchResponse};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

const MAX_RETRIES: usize = 3;

/// HTTP client for CRW search and scrape endpoints.
#[derive(Debug, Clone)]
pub struct CrwClient {
    base_url: String,
    http: reqwest::Client,
    api_token: Option<String>,
}

impl CrwClient {
    /// Create a new CRW client.
    ///
    /// # Errors
    /// Returns `CrwError::Request` if the HTTP client builder fails.
    pub fn new(
        base_url: &str,
        timeout: Duration,
        api_token: Option<String>,
    ) -> Result<Self, CrwError> {
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            api_token: api_token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    /// Search via CRW `POST /v1/search` with retry on transient errors.
    pub async fn search(&self, args: &CrwSearchArgs) -> Result<CrwSearchResponse, CrwError> {
        if args.query.trim().is_empty() {
            return Err(CrwError::EmptyQuery);
        }

        let request = args.to_request();

        for attempt in 0..=MAX_RETRIES {
            match self.search_once(&request).await {
                Ok(response) => return Ok(response),
                Err(error) if error.is_retryable() && attempt < MAX_RETRIES => {
                    let delay = retry_delay(attempt + 1);
                    warn!(
                        query = %args.query.trim(),
                        attempt = attempt + 1,
                        max_retries = MAX_RETRIES,
                        error = %error,
                        retry_after_ms = delay.as_millis() as u64,
                        "CRW search transient error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("retry loop ran at least once");
    }

    /// Scrape via CRW `POST /v1/scrape`.
    ///
    /// Used by `web_crawler` rendered modes (lightpanda/playwright).
    /// No retry — the caller controls render mode and expects a single attempt.
    pub async fn scrape(&self, args: &CrwScrapeArgs) -> Result<CrwScrapeResponse, CrwError> {
        if args.url.trim().is_empty() {
            return Err(CrwError::InvalidUrl);
        }

        let endpoint = format!("{}/v1/scrape", self.base_url);
        let request = args.to_request();

        debug!(url = %args.url, "CRW scrape request");

        let mut req = self
            .http
            .post(&endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .json(&request);

        if let Some(token) = self.api_token.as_deref() {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }

        let response = req.send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(CrwError::HttpStatus {
                status,
                body: truncate_for_error(body),
            });
        }

        Ok(response.json::<CrwScrapeResponse>().await?)
    }

    async fn search_once(
        &self,
        request: &super::types::CrwSearchRequest,
    ) -> Result<CrwSearchResponse, CrwError> {
        let endpoint = format!("{}/v1/search", self.base_url);

        info!(
            query_len = request.query.chars().count(),
            limit = request.limit,
            sources = ?request.sources,
            lang = ?request.lang,
            tbs = ?request.tbs,
            categories = ?request.categories,
            token_configured = self.api_token.is_some(),
            "CRW search request"
        );
        debug!(query = %request.query, "CRW search query");

        let mut req = self
            .http
            .post(&endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .json(request);

        if let Some(token) = self.api_token.as_deref() {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }

        let started_at = Instant::now();
        let response = req.send().await?;
        let elapsed_ms = started_at.elapsed().as_millis() as u64;

        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(CrwError::HttpStatus {
                status,
                body: truncate_for_error(body),
            });
        }

        let parsed = response.json::<CrwSearchResponse>().await?;
        if !parsed.success {
            warn!(
                status = status.as_u16(),
                elapsed_ms,
                content_type,
                error_present = parsed.error.is_some(),
                "CRW search returned failure envelope"
            );
            return Err(CrwError::ApiFailure {
                message: truncate_for_error(
                    parsed
                        .error
                        .unwrap_or_else(|| "search provider returned success=false".to_string()),
                ),
            });
        }

        let result_count = parsed.data.len();
        if result_count == 0 {
            warn!(
                status = status.as_u16(),
                elapsed_ms, content_type, "CRW search returned successful empty result set"
            );
        } else {
            info!(
                status = status.as_u16(),
                elapsed_ms, content_type, result_count, "CRW search response parsed"
            );
        }

        Ok(parsed)
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let base_ms = 500_u64;
    let max_ms = 10_000;
    let delay = base_ms * 2u64.pow(attempt.saturating_sub(1) as u32);
    let capped = delay.min(max_ms);
    let jitter = (capped as f64 * 0.2).round() as u64;
    Duration::from_millis(capped + jitter)
}

fn truncate_for_error(body: String) -> String {
    const LIMIT: usize = 500;
    if body.chars().count() <= LIMIT {
        return body;
    }
    let mut truncated: String = body.chars().take(LIMIT).collect();
    truncated.push_str("...");
    truncated
}
