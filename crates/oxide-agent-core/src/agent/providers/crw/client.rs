use super::error::CrwError;
use super::types::{CrwScrapeArgs, CrwScrapeResponse};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;
use tracing::debug;

/// HTTP client for CRW rendered scrape endpoint.
#[derive(Debug, Clone)]
pub struct CrwScrapeClient {
    base_url: String,
    http: reqwest::Client,
    api_token: String,
}

impl CrwScrapeClient {
    /// Create a scrape client from runtime env. Returns `Ok(None)` unless both
    /// `OXIDE_CRW_BASE_URL` and `OXIDE_CRW_API_TOKEN` are non-empty.
    pub fn new_from_env() -> Result<Option<Self>, CrwError> {
        let (Some(base_url), Some(api_token)) = (
            crate::config::get_crw_base_url(),
            crate::config::get_crw_api_token(),
        ) else {
            return Ok(None);
        };
        Self::new(
            &base_url,
            Duration::from_secs(crate::config::get_crw_timeout_secs()),
            api_token,
        )
        .map(Some)
    }

    /// Create a new CRW scrape client.
    pub fn new(base_url: &str, timeout: Duration, api_token: String) -> Result<Self, CrwError> {
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        let api_token = api_token.trim().to_string();
        if api_token.is_empty() {
            return Err(CrwError::MissingApiToken);
        }
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            api_token,
        })
    }

    /// Scrape via CRW `POST /v1/scrape`.
    ///
    /// Used by `web_crawler` rendered modes. No retry: the caller selected a
    /// concrete render mode and should receive the exact attempt result.
    pub async fn scrape(&self, args: &CrwScrapeArgs) -> Result<CrwScrapeResponse, CrwError> {
        if args.url.trim().is_empty() {
            return Err(CrwError::InvalidUrl);
        }

        let endpoint = format!("{}/v1/scrape", self.base_url);
        let request = args.to_request();

        debug!(url = %args.url, "CRW scrape request");

        let response = self
            .http
            .post(&endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", self.api_token))
            .json(&request)
            .send()
            .await?;

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

        let parsed = response.json::<CrwScrapeResponse>().await?;
        if !parsed.success {
            return Err(CrwError::ApiFailure {
                message: "CRW scrape returned success=false".to_string(),
            });
        }
        Ok(parsed)
    }
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
