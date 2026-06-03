use super::error::BraveSearchError;
use super::types::{BraveSearchResponse, NormalizedBraveSearchArgs};
use reqwest::header::ACCEPT;
use reqwest::StatusCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
use tracing::{debug, warn};
use url::Url;

pub const BRAVE_WEB_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const SUBSCRIPTION_TOKEN_HEADER: &str = "X-Subscription-Token";
const MAX_RETRIES: usize = 1;
const ERROR_BODY_LIMIT: usize = 500;
const RETRY_DELAY_MS: u64 = 150;

#[derive(Debug, Clone)]
pub struct BraveSearchClient {
    api_key: String,
    endpoint: String,
    http: reqwest::Client,
    limiter: Arc<BraveSearchRateLimiter>,
    timeout: Duration,
}

#[derive(Debug)]
struct BraveSearchRateLimiter {
    semaphore: Semaphore,
    min_delay: Duration,
    last_started: Mutex<Option<Instant>>,
}

impl BraveSearchClient {
    /// Create a Brave Search client.
    ///
    /// # Errors
    ///
    /// Returns [`BraveSearchError::MissingApiKey`] when `api_key` is empty, or
    /// [`BraveSearchError::Request`] when the HTTP client cannot be built.
    pub fn new(
        api_key: impl Into<String>,
        timeout: Duration,
        max_concurrent: usize,
        min_delay: Duration,
    ) -> Result<Self, BraveSearchError> {
        Self::new_with_endpoint(
            api_key,
            timeout,
            max_concurrent,
            min_delay,
            BRAVE_WEB_SEARCH_ENDPOINT,
        )
    }

    fn new_with_endpoint(
        api_key: impl Into<String>,
        timeout: Duration,
        max_concurrent: usize,
        min_delay: Duration,
        endpoint: impl Into<String>,
    ) -> Result<Self, BraveSearchError> {
        let api_key = api_key.into().trim().to_string();
        if api_key.is_empty() {
            return Err(BraveSearchError::MissingApiKey);
        }

        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| BraveSearchError::Request(error.to_string()))?;

        Ok(Self {
            api_key,
            endpoint: endpoint.into(),
            http,
            limiter: Arc::new(BraveSearchRateLimiter {
                semaphore: Semaphore::new(max_concurrent.max(1)),
                min_delay,
                last_started: Mutex::new(None),
            }),
            timeout,
        })
    }

    /// Search the Brave Web Search API.
    ///
    /// # Errors
    ///
    /// Returns mapped [`BraveSearchError`] values for empty queries, HTTP
    /// status failures, network errors, timeouts, and JSON decode failures.
    pub async fn search(
        &self,
        args: &NormalizedBraveSearchArgs,
    ) -> Result<BraveSearchResponse, BraveSearchError> {
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            match self.search_once(args).await {
                Ok(response) => return Ok(response),
                Err(error) if error.is_retryable() && attempt < MAX_RETRIES => {
                    let delay = retry_delay(attempt + 1);
                    warn!(
                        query = %args.query,
                        attempt = attempt + 1,
                        max_retries = MAX_RETRIES,
                        error = %error,
                        retry_after_ms = delay.as_millis() as u64,
                        "Brave Search transient error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.expect("loop ran at least once"))
    }

    async fn search_once(
        &self,
        args: &NormalizedBraveSearchArgs,
    ) -> Result<BraveSearchResponse, BraveSearchError> {
        if args.query.trim().is_empty() {
            return Err(BraveSearchError::EmptyQuery);
        }

        let _permit = self
            .limiter
            .semaphore
            .acquire()
            .await
            .map_err(|error| BraveSearchError::Request(error.to_string()))?;
        self.limiter.wait_turn().await;

        let url = request_url(&self.endpoint, args)?;

        let response = self
            .http
            .get(url)
            .header(ACCEPT, "application/json")
            .header(SUBSCRIPTION_TOKEN_HEADER, self.api_key.as_str())
            .send()
            .await
            .map_err(map_reqwest_error)?;

        log_rate_limit_headers(response.headers());

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(map_status_error(status, truncate_for_error(body)));
        }

        response
            .json::<BraveSearchResponse>()
            .await
            .map_err(|error| BraveSearchError::InvalidResponse(error.to_string()))
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[must_use]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl BraveSearchRateLimiter {
    async fn wait_turn(&self) {
        let mut last_started = self.last_started.lock().await;

        if let Some(last) = *last_started {
            let elapsed = last.elapsed();
            if elapsed < self.min_delay {
                tokio::time::sleep(self.min_delay - elapsed).await;
            }
        }

        *last_started = Some(Instant::now());
    }
}

fn query_params(args: &NormalizedBraveSearchArgs) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("q", args.query.clone()),
        ("count", args.max_results.to_string()),
        ("offset", args.offset.to_string()),
        ("safesearch", args.safesearch.clone()),
        ("extra_snippets", args.extra_snippets.to_string()),
    ];

    push_optional_param(&mut params, "country", args.country.as_deref());
    push_optional_param(&mut params, "search_lang", args.search_lang.as_deref());
    push_optional_param(&mut params, "ui_lang", args.ui_lang.as_deref());
    push_optional_param(&mut params, "freshness", args.freshness.as_deref());

    params
}

fn request_url(endpoint: &str, args: &NormalizedBraveSearchArgs) -> Result<Url, BraveSearchError> {
    let mut url =
        Url::parse(endpoint).map_err(|error| BraveSearchError::Request(error.to_string()))?;
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query_params(args) {
            pairs.append_pair(key, &value);
        }
    }

    Ok(url)
}

fn push_optional_param(
    params: &mut Vec<(&'static str, String)>,
    key: &'static str,
    value: Option<&str>,
) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        params.push((key, value.to_string()));
    }
}

fn map_reqwest_error(error: reqwest::Error) -> BraveSearchError {
    if error.is_timeout() {
        return BraveSearchError::Timeout;
    }
    if error.is_connect() {
        return BraveSearchError::Network(error.to_string());
    }
    if error.is_decode() {
        return BraveSearchError::InvalidResponse(error.to_string());
    }

    BraveSearchError::Request(error.to_string())
}

fn map_status_error(status: StatusCode, body: String) -> BraveSearchError {
    match status.as_u16() {
        401 | 403 => BraveSearchError::Auth { status },
        429 => BraveSearchError::RateLimited,
        500 | 502 | 503 | 504 => BraveSearchError::Server { status, body },
        _ => BraveSearchError::HttpStatus { status, body },
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let jitter_ms = u64::from(fastrand::u16(0..50));
    Duration::from_millis(RETRY_DELAY_MS * attempt as u64 + jitter_ms)
}

fn truncate_for_error(body: String) -> String {
    if body.chars().count() <= ERROR_BODY_LIMIT {
        return body;
    }

    let mut truncated = body.chars().take(ERROR_BODY_LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn log_rate_limit_headers(headers: &reqwest::header::HeaderMap) {
    for name in [
        "x-ratelimit-limit",
        "x-ratelimit-policy",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            debug!(header = name, value, "Brave Search rate-limit header");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    #[derive(Debug)]
    struct CapturedRequest {
        request: String,
    }

    #[tokio::test]
    async fn sends_expected_headers_and_query_params() {
        let (addr, mut rx) = serve_http_sequence(vec![(StatusCode::OK, sample_body())]).await;
        let client = test_client(addr, Duration::ZERO);
        let args = normalized_args();

        let response = client.search(&args).await.expect("search succeeds");
        let request = rx.recv().await.expect("captured request");

        assert_eq!(response.web.expect("web").results.len(), 1);
        let request = request.request.to_ascii_lowercase();
        assert!(request.starts_with("get /res/v1/web/search?"));
        assert!(request.contains("q=rust"));
        assert!(request.contains("count=5"));
        assert!(request.contains("offset=2"));
        assert!(request.contains("country=us"));
        assert!(request.contains("search_lang=en"));
        assert!(request.contains("ui_lang=en-us"));
        assert!(request.contains("freshness=pw"));
        assert!(request.contains("safesearch=moderate"));
        assert!(request.contains("extra_snippets=true"));
        assert!(request.contains("accept: application/json"));
        assert!(!request.contains("accept-encoding: gzip"));
        assert!(request.contains("x-subscription-token: test-key"));
    }

    #[tokio::test]
    async fn maps_non_retryable_status_without_retry() {
        let (addr, mut rx) =
            serve_http_sequence(vec![(StatusCode::TOO_MANY_REQUESTS, "quota")]).await;
        let client = test_client(addr, Duration::ZERO);

        let error = client
            .search(&normalized_args())
            .await
            .expect_err("rate limited");

        assert!(matches!(error, BraveSearchError::RateLimited));
        assert_eq!(error.code(), "rate_limited");
        assert!(error.provider_unavailable());
        assert!(rx.recv().await.is_some());
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn retries_server_error_once() {
        let (addr, mut rx) = serve_http_sequence(vec![
            (StatusCode::BAD_GATEWAY, "bad gateway"),
            (StatusCode::OK, sample_body()),
        ])
        .await;
        let client = test_client(addr, Duration::ZERO);

        let response = client
            .search(&normalized_args())
            .await
            .expect("retry succeeds");

        assert_eq!(response.web.expect("web").results.len(), 1);
        assert!(rx.recv().await.is_some());
        assert!(rx.recv().await.is_some());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn maps_status_codes_to_error_kinds() {
        assert!(matches!(
            map_status_error(StatusCode::UNAUTHORIZED, String::new()),
            BraveSearchError::Auth { .. }
        ));
        assert!(matches!(
            map_status_error(StatusCode::FORBIDDEN, String::new()),
            BraveSearchError::Auth { .. }
        ));
        assert!(matches!(
            map_status_error(StatusCode::TOO_MANY_REQUESTS, String::new()),
            BraveSearchError::RateLimited
        ));
        assert!(matches!(
            map_status_error(StatusCode::INTERNAL_SERVER_ERROR, String::new()),
            BraveSearchError::Server { .. }
        ));
        assert!(matches!(
            map_status_error(StatusCode::BAD_REQUEST, String::new()),
            BraveSearchError::HttpStatus { .. }
        ));
    }

    async fn serve_http_sequence(
        responses: Vec<(StatusCode, &'static str)>,
    ) -> (SocketAddr, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().expect("local address");
        let (tx, rx) = mpsc::channel(responses.len().max(1));
        let responses = Arc::new(responses);
        let counter = Arc::new(AtomicUsize::new(0));

        tokio::spawn(async move {
            loop {
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let Some((status, body)) = responses.get(index).copied() else {
                    break;
                };
                let (mut stream, _) = listener.accept().await.expect("accept request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
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
                tx.send(CapturedRequest {
                    request: String::from_utf8_lossy(&request).to_string(),
                })
                .await
                .expect("send captured request");
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("status"),
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });

        (addr, rx)
    }

    fn test_client(addr: SocketAddr, min_delay: Duration) -> BraveSearchClient {
        BraveSearchClient::new_with_endpoint(
            "test-key",
            Duration::from_secs(2),
            1,
            min_delay,
            format!("http://{addr}/res/v1/web/search"),
        )
        .expect("client")
    }

    fn normalized_args() -> NormalizedBraveSearchArgs {
        NormalizedBraveSearchArgs {
            query: "rust".to_string(),
            max_results: 5,
            offset: 2,
            country: Some("US".to_string()),
            search_lang: Some("en".to_string()),
            ui_lang: Some("en-US".to_string()),
            freshness: Some("pw".to_string()),
            safesearch: "moderate".to_string(),
            extra_snippets: true,
        }
    }

    const fn sample_body() -> &'static str {
        r#"{"web":{"results":[{"title":"Rust","url":"https://www.rust-lang.org/","description":"Rust language"}]}}"#
    }
}
