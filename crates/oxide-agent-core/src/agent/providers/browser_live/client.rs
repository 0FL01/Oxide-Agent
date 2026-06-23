use super::error::BrowserSidecarError;
use super::types::{
    ActionRequest, ActionResponse, CloseSessionRequest, CloseSessionResponse, ConsoleDebugQuery,
    ConsoleDebugResponse, CreateSessionRequest, CreateSessionResponse, DomExtractRequest,
    DomExtractResponse, GotoRequest, GotoResponse, NetworkDebugQuery, NetworkDebugResponse,
    ObserveQuery, ObserveResponse, ScreenshotQuery, ScreenshotResponse, SidecarErrorBody,
};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

const IDEMPOTENCY_KEY_HEADER: &str = "Idempotency-Key";
const ERROR_BODY_LIMIT: usize = 500;

/// Per-endpoint timeout configuration for the browser sidecar client.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BrowserSidecarTimeouts {
    /// `POST /sessions` timeout.
    pub create_session: Duration,
    /// `DELETE /sessions/{id}` timeout.
    pub close_session: Duration,
    /// `POST /sessions/{id}/goto` timeout.
    pub goto: Duration,
    /// Cached observe timeout.
    pub observe: Duration,
    /// Fresh observe timeout.
    pub observe_fresh: Duration,
    /// `POST /sessions/{id}/action` timeout.
    pub action: Duration,
    /// DOM extract timeout.
    pub dom_extract: Duration,
    /// Latest screenshot metadata timeout.
    pub screenshot_metadata: Duration,
    /// Network debug timeout.
    pub debug_network: Duration,
    /// Console debug timeout.
    pub debug_console: Duration,
}

impl Default for BrowserSidecarTimeouts {
    fn default() -> Self {
        Self {
            create_session: Duration::from_secs(30),
            close_session: Duration::from_secs(15),
            goto: Duration::from_secs(60),
            observe: Duration::from_secs(5),
            observe_fresh: Duration::from_secs(15),
            action: Duration::from_secs(60),
            dom_extract: Duration::from_secs(15),
            screenshot_metadata: Duration::from_secs(5),
            debug_network: Duration::from_secs(10),
            debug_console: Duration::from_secs(10),
        }
    }
}

impl BrowserSidecarTimeouts {
    /// Longest configured timeout. Used as the underlying reqwest client cap;
    /// individual requests also set their endpoint-specific timeout.
    #[must_use]
    pub fn max_timeout(self) -> Duration {
        [
            self.create_session,
            self.close_session,
            self.goto,
            self.observe,
            self.observe_fresh,
            self.action,
            self.dom_extract,
            self.screenshot_metadata,
            self.debug_network,
            self.debug_console,
        ]
        .into_iter()
        .max()
        .unwrap_or(Duration::from_secs(60))
    }
}

/// Required idempotency key for mutating sidecar requests.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Build a non-empty idempotency key.
    ///
    /// # Errors
    /// Returns [`BrowserSidecarError::MissingIdempotencyKey`] when the value is empty.
    pub fn new(value: impl Into<String>) -> Result<Self, BrowserSidecarError> {
        let value = value.into().trim().to_string();
        if value.is_empty() {
            return Err(BrowserSidecarError::MissingIdempotencyKey);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed REST client for the browser sidecar (`oxide-browser-sidecar`).
#[derive(Debug, Clone)]
pub struct BrowserSidecarClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
    timeouts: BrowserSidecarTimeouts,
}

/// Testable browser sidecar seam used by browser tools and loop code.
///
/// Production uses [`BrowserSidecarClient`]. Unit tests can use the fake
/// implementation from the test-only `test_support` module without running
/// Chromium, OpenCode Go, or any external service.
#[async_trait]
pub trait BrowserSidecar: Send + Sync {
    /// Check sidecar health without exposing browser state.
    async fn healthz(&self) -> Result<serde_json::Value, BrowserSidecarError>;

    /// Create a browser session.
    async fn create_session(
        &self,
        request: &CreateSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CreateSessionResponse, BrowserSidecarError>;

    /// Close a browser session.
    async fn close_session(
        &self,
        session_id: &str,
        request: &CloseSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CloseSessionResponse, BrowserSidecarError>;

    /// Navigate an existing session.
    async fn goto(
        &self,
        session_id: &str,
        request: &GotoRequest,
        key: &IdempotencyKey,
    ) -> Result<GotoResponse, BrowserSidecarError>;

    /// Observe the current browser state.
    async fn observe(
        &self,
        session_id: &str,
        query: &ObserveQuery,
    ) -> Result<ObserveResponse, BrowserSidecarError>;

    /// Execute one browser action.
    async fn execute_action(
        &self,
        session_id: &str,
        request: &ActionRequest,
        key: &IdempotencyKey,
    ) -> Result<ActionResponse, BrowserSidecarError>;

    /// Extract bounded DOM rows from the current page.
    async fn extract_dom(
        &self,
        session_id: &str,
        request: &DomExtractRequest,
    ) -> Result<DomExtractResponse, BrowserSidecarError>;

    /// Return latest screenshot metadata without image bytes.
    async fn latest_screenshot(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<ScreenshotResponse, BrowserSidecarError>;

    /// Return latest screenshot image bytes for model-side vision calls.
    async fn latest_screenshot_bytes(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<Vec<u8>, BrowserSidecarError>;

    /// Return network debug diagnostics.
    async fn debug_network(
        &self,
        session_id: &str,
        query: &NetworkDebugQuery,
    ) -> Result<NetworkDebugResponse, BrowserSidecarError>;

    /// Return console debug diagnostics.
    async fn debug_console(
        &self,
        session_id: &str,
        query: &ConsoleDebugQuery,
    ) -> Result<ConsoleDebugResponse, BrowserSidecarError>;
}

impl BrowserSidecarClient {
    /// Create a sidecar client with default endpoint timeouts.
    ///
    /// # Errors
    /// Returns an error if the base URL is invalid, the token is missing, or the HTTP client fails to build.
    pub fn new(base_url: &str, token: &str) -> Result<Self, BrowserSidecarError> {
        Self::with_timeouts(base_url, token, BrowserSidecarTimeouts::default())
    }

    /// Create a sidecar client with explicit endpoint timeouts.
    ///
    /// # Errors
    /// Returns an error if the base URL is invalid, the token is missing, or the HTTP client fails to build.
    pub fn with_timeouts(
        base_url: &str,
        token: &str,
        timeouts: BrowserSidecarTimeouts,
    ) -> Result<Self, BrowserSidecarError> {
        let base_url = normalize_base_url(base_url)?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(BrowserSidecarError::MissingToken);
        }
        let http = reqwest::Client::builder()
            .timeout(timeouts.max_timeout())
            .build()?;
        Ok(Self {
            base_url,
            token,
            http,
            timeouts,
        })
    }

    /// Effective timeout configuration.
    #[must_use]
    pub const fn timeouts(&self) -> BrowserSidecarTimeouts {
        self.timeouts
    }

    /// `GET /healthz`. The endpoint must not expose session data.
    pub async fn healthz(&self) -> Result<serde_json::Value, BrowserSidecarError> {
        self.get_json("/healthz", Option::<&()>::None, self.timeouts.observe)
            .await
    }

    /// `POST /sessions`.
    pub async fn create_session(
        &self,
        request: &CreateSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CreateSessionResponse, BrowserSidecarError> {
        self.send_json(
            reqwest::Method::POST,
            "/sessions",
            request,
            Some(key),
            self.timeouts.create_session,
        )
        .await
    }

    /// `DELETE /sessions/{id}`.
    pub async fn close_session(
        &self,
        session_id: &str,
        request: &CloseSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CloseSessionResponse, BrowserSidecarError> {
        let path = session_path(session_id, "")?;
        self.send_json(
            reqwest::Method::DELETE,
            &path,
            request,
            Some(key),
            self.timeouts.close_session,
        )
        .await
    }

    /// `POST /sessions/{id}/goto`.
    pub async fn goto(
        &self,
        session_id: &str,
        request: &GotoRequest,
        key: &IdempotencyKey,
    ) -> Result<GotoResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/goto")?;
        self.send_json(
            reqwest::Method::POST,
            &path,
            request,
            Some(key),
            self.timeouts.goto,
        )
        .await
    }

    /// `GET /sessions/{id}/observe`.
    pub async fn observe(
        &self,
        session_id: &str,
        query: &ObserveQuery,
    ) -> Result<ObserveResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/observe")?;
        let timeout = if query.fresh {
            self.timeouts.observe_fresh
        } else {
            self.timeouts.observe
        };
        self.get_json(&path, Some(query), timeout).await
    }

    /// `POST /sessions/{id}/action`.
    pub async fn execute_action(
        &self,
        session_id: &str,
        request: &ActionRequest,
        key: &IdempotencyKey,
    ) -> Result<ActionResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/action")?;
        self.send_json(
            reqwest::Method::POST,
            &path,
            request,
            Some(key),
            self.timeouts.action,
        )
        .await
    }

    /// `POST /sessions/{id}/extract/dom`.
    pub async fn extract_dom(
        &self,
        session_id: &str,
        request: &DomExtractRequest,
    ) -> Result<DomExtractResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/extract/dom")?;
        self.send_json(
            reqwest::Method::POST,
            &path,
            request,
            None,
            self.timeouts.dom_extract,
        )
        .await
    }

    /// `GET /sessions/{id}/screenshot/latest` metadata endpoint.
    pub async fn latest_screenshot(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<ScreenshotResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/screenshot/latest")?;
        self.get_json(&path, Some(query), self.timeouts.screenshot_metadata)
            .await
    }

    /// `GET /sessions/{id}/screenshot/latest?format=binary` binary endpoint.
    pub async fn latest_screenshot_bytes(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<Vec<u8>, BrowserSidecarError> {
        let path = session_path(session_id, "/screenshot/latest")?;
        self.get_bytes(&path, Some(query), self.timeouts.screenshot_metadata)
            .await
    }

    /// `GET /sessions/{id}/debug/network`.
    pub async fn debug_network(
        &self,
        session_id: &str,
        query: &NetworkDebugQuery,
    ) -> Result<NetworkDebugResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/debug/network")?;
        self.get_json(&path, Some(query), self.timeouts.debug_network)
            .await
    }

    /// `GET /sessions/{id}/debug/console`.
    pub async fn debug_console(
        &self,
        session_id: &str,
        query: &ConsoleDebugQuery,
    ) -> Result<ConsoleDebugResponse, BrowserSidecarError> {
        let path = session_path(session_id, "/debug/console")?;
        self.get_json(&path, Some(query), self.timeouts.debug_console)
            .await
    }

    async fn send_json<T, R>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &T,
        key: Option<&IdempotencyKey>,
        timeout: Duration,
    ) -> Result<R, BrowserSidecarError>
    where
        T: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let endpoint = self.endpoint(path);
        let mut req = self
            .http
            .request(method, endpoint)
            .headers(self.common_headers(key)?)
            .timeout(timeout)
            .json(body);
        req = req.header(CONTENT_TYPE, "application/json");
        parse_response(req.send().await?).await
    }

    async fn get_json<Q, R>(
        &self,
        path: &str,
        query: Option<&Q>,
        timeout: Duration,
    ) -> Result<R, BrowserSidecarError>
    where
        Q: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let mut endpoint = self.endpoint(path);
        if let Some(query) = query {
            endpoint = endpoint_with_query(&endpoint, query)?;
        }
        let req = self
            .http
            .get(endpoint)
            .headers(self.common_headers(None)?)
            .timeout(timeout);
        parse_response(req.send().await?).await
    }

    async fn get_bytes<Q>(
        &self,
        path: &str,
        query: Option<&Q>,
        timeout: Duration,
    ) -> Result<Vec<u8>, BrowserSidecarError>
    where
        Q: Serialize + ?Sized,
    {
        let mut endpoint = self.endpoint(path);
        if let Some(query) = query {
            endpoint = endpoint_with_query(&endpoint, query)?;
        }
        let mut headers = self.common_headers(None)?;
        headers.insert(ACCEPT, HeaderValue::from_static("image/*"));
        let response = self
            .http
            .get(endpoint)
            .headers(headers)
            .timeout(timeout)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(BrowserSidecarError::HttpStatus {
                status,
                body: truncate_for_error(body),
            });
        }
        Ok(response.bytes().await?.to_vec())
    }

    fn common_headers(
        &self,
        key: Option<&IdempotencyKey>,
    ) -> Result<HeaderMap, BrowserSidecarError> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .map_err(|err| BrowserSidecarError::InvalidResponse(err.to_string()))?,
        );
        if let Some(key) = key {
            headers.insert(
                IDEMPOTENCY_KEY_HEADER,
                HeaderValue::from_str(key.as_str())
                    .map_err(|err| BrowserSidecarError::InvalidResponse(err.to_string()))?,
            );
        }
        Ok(headers)
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

#[async_trait]
impl BrowserSidecar for BrowserSidecarClient {
    async fn healthz(&self) -> Result<serde_json::Value, BrowserSidecarError> {
        BrowserSidecarClient::healthz(self).await
    }

    async fn create_session(
        &self,
        request: &CreateSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CreateSessionResponse, BrowserSidecarError> {
        BrowserSidecarClient::create_session(self, request, key).await
    }

    async fn close_session(
        &self,
        session_id: &str,
        request: &CloseSessionRequest,
        key: &IdempotencyKey,
    ) -> Result<CloseSessionResponse, BrowserSidecarError> {
        BrowserSidecarClient::close_session(self, session_id, request, key).await
    }

    async fn goto(
        &self,
        session_id: &str,
        request: &GotoRequest,
        key: &IdempotencyKey,
    ) -> Result<GotoResponse, BrowserSidecarError> {
        BrowserSidecarClient::goto(self, session_id, request, key).await
    }

    async fn observe(
        &self,
        session_id: &str,
        query: &ObserveQuery,
    ) -> Result<ObserveResponse, BrowserSidecarError> {
        BrowserSidecarClient::observe(self, session_id, query).await
    }

    async fn execute_action(
        &self,
        session_id: &str,
        request: &ActionRequest,
        key: &IdempotencyKey,
    ) -> Result<ActionResponse, BrowserSidecarError> {
        BrowserSidecarClient::execute_action(self, session_id, request, key).await
    }

    async fn extract_dom(
        &self,
        session_id: &str,
        request: &DomExtractRequest,
    ) -> Result<DomExtractResponse, BrowserSidecarError> {
        BrowserSidecarClient::extract_dom(self, session_id, request).await
    }

    async fn latest_screenshot(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<ScreenshotResponse, BrowserSidecarError> {
        BrowserSidecarClient::latest_screenshot(self, session_id, query).await
    }

    async fn latest_screenshot_bytes(
        &self,
        session_id: &str,
        query: &ScreenshotQuery,
    ) -> Result<Vec<u8>, BrowserSidecarError> {
        BrowserSidecarClient::latest_screenshot_bytes(self, session_id, query).await
    }

    async fn debug_network(
        &self,
        session_id: &str,
        query: &NetworkDebugQuery,
    ) -> Result<NetworkDebugResponse, BrowserSidecarError> {
        BrowserSidecarClient::debug_network(self, session_id, query).await
    }

    async fn debug_console(
        &self,
        session_id: &str,
        query: &ConsoleDebugQuery,
    ) -> Result<ConsoleDebugResponse, BrowserSidecarError> {
        BrowserSidecarClient::debug_console(self, session_id, query).await
    }
}

async fn parse_response<R>(response: reqwest::Response) -> Result<R, BrowserSidecarError>
where
    R: DeserializeOwned,
{
    let status = response.status();
    let text = response.text().await?;
    let value = serde_json::from_str::<serde_json::Value>(&text).map_err(|err| {
        if status.is_success() {
            BrowserSidecarError::InvalidResponse(err.to_string())
        } else {
            BrowserSidecarError::HttpStatus {
                status,
                body: truncate_for_error(text),
            }
        }
    })?;

    if let Some(error) = extract_sidecar_error(&value) {
        return Err(BrowserSidecarError::ApiFailure {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
            hint: error.hint,
        });
    }

    if !status.is_success() {
        return Err(BrowserSidecarError::HttpStatus {
            status,
            body: truncate_for_error(value.to_string()),
        });
    }

    serde_json::from_value(value)
        .map_err(|err| BrowserSidecarError::InvalidResponse(err.to_string()))
}

fn extract_sidecar_error(value: &serde_json::Value) -> Option<SidecarErrorBody> {
    let ok = value.get("ok").and_then(serde_json::Value::as_bool);
    if ok != Some(false) {
        return None;
    }
    serde_json::from_value(value.get("error")?.clone()).ok()
}

fn normalize_base_url(base_url: &str) -> Result<String, BrowserSidecarError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() || reqwest::Url::parse(trimmed).is_err() {
        return Err(BrowserSidecarError::InvalidBaseUrl(base_url.to_string()));
    }
    Ok(trimmed.to_string())
}

fn session_path(session_id: &str, suffix: &str) -> Result<String, BrowserSidecarError> {
    let trimmed = session_id.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|ch| matches!(ch, '/' | '?' | '#' | '\\'))
    {
        return Err(BrowserSidecarError::InvalidSessionId);
    }
    Ok(format!("/sessions/{trimmed}{suffix}"))
}

fn endpoint_with_query<T>(endpoint: &str, query: &T) -> Result<String, BrowserSidecarError>
where
    T: Serialize + ?Sized,
{
    let mut url = reqwest::Url::parse(endpoint)
        .map_err(|err| BrowserSidecarError::InvalidBaseUrl(err.to_string()))?;
    let value = serde_json::to_value(query)
        .map_err(|err| BrowserSidecarError::InvalidResponse(err.to_string()))?;
    let Some(object) = value.as_object() else {
        return Ok(url.to_string());
    };
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in object {
            if value.is_null() {
                continue;
            }
            if let Some(value) = scalar_query_value(value) {
                pairs.append_pair(key, &value);
            }
        }
    }
    Ok(url.to_string())
}

fn scalar_query_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn truncate_for_error(body: String) -> String {
    if body.chars().count() <= ERROR_BODY_LIMIT {
        return body;
    }
    let mut truncated: String = body.chars().take(ERROR_BODY_LIMIT).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::providers::browser_live::types::{BrowserMode, BrowserProfile, Viewport};
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    #[derive(Debug)]
    struct CapturedRequest {
        request: String,
    }

    #[tokio::test]
    async fn create_session_sends_auth_and_idempotency_headers() {
        let (addr, mut rx) = serve_once(reqwest::StatusCode::OK, create_response()).await;
        let client = test_client(addr);
        let key = IdempotencyKey::new("task:session:1").expect("key");

        let response = client
            .create_session(&create_request(), &key)
            .await
            .expect("create session response");
        let request = rx.recv().await.expect("captured request").request;
        let lower = request.to_ascii_lowercase();

        assert_eq!(response.session_id, "br_1");
        assert!(lower.starts_with("post /sessions http/1.1"));
        assert!(lower.contains("authorization: bearer test-token"));
        assert!(lower.contains("idempotency-key: task:session:1"));
        assert!(request.contains(r#"task_id":"task-1"#));
    }

    #[tokio::test]
    async fn maps_error_envelope_to_retryable_sidecar_error() {
        let body = r#"{"request_id":"req-1","session_id":"br_1","ok":false,"error":{"code":"timeout","message":"timed out","retryable":true,"hint":"observe then retry","details":{}}}"#;
        let (addr, _rx) = serve_once(reqwest::StatusCode::GATEWAY_TIMEOUT, body).await;
        let client = test_client(addr);
        let key = IdempotencyKey::new("task:session:2").expect("key");

        let error = client
            .create_session(&create_request(), &key)
            .await
            .expect_err("sidecar error");

        assert!(error.is_retryable());
        assert_eq!(error.kind(), "browser_sidecar_timeout");
        assert_eq!(error.agent_message(), "observe then retry");
    }

    #[test]
    fn rejects_missing_token_and_empty_idempotency_key() {
        assert!(matches!(
            BrowserSidecarClient::new("http://127.0.0.1:8787", " "),
            Err(BrowserSidecarError::MissingToken)
        ));
        assert!(matches!(
            IdempotencyKey::new(" "),
            Err(BrowserSidecarError::MissingIdempotencyKey)
        ));
    }

    #[test]
    fn preserves_explicit_timeout_config() {
        let timeouts = BrowserSidecarTimeouts {
            create_session: Duration::from_secs(1),
            close_session: Duration::from_secs(2),
            goto: Duration::from_secs(3),
            observe: Duration::from_secs(4),
            observe_fresh: Duration::from_secs(5),
            action: Duration::from_secs(6),
            dom_extract: Duration::from_secs(7),
            screenshot_metadata: Duration::from_secs(8),
            debug_network: Duration::from_secs(9),
            debug_console: Duration::from_secs(10),
        };
        let client =
            BrowserSidecarClient::with_timeouts("http://127.0.0.1:8787/", "token", timeouts)
                .expect("client");

        assert_eq!(client.timeouts(), timeouts);
        assert_eq!(timeouts.max_timeout(), Duration::from_secs(10));
    }

    #[test]
    fn rejects_invalid_session_path_segment() {
        assert!(matches!(
            session_path("br/1", "/observe"),
            Err(BrowserSidecarError::InvalidSessionId)
        ));
    }

    async fn serve_once(
        status: reqwest::StatusCode,
        body: &'static str,
    ) -> (SocketAddr, mpsc::Receiver<CapturedRequest>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let addr = listener.local_addr().expect("local address");
        let (tx, rx) = mpsc::channel(1);

        tokio::spawn(async move {
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
        });

        (addr, rx)
    }

    fn test_client(addr: SocketAddr) -> BrowserSidecarClient {
        BrowserSidecarClient::new(&format!("http://{addr}"), "test-token").expect("client")
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            task_id: "task-1".to_string(),
            profile: BrowserProfile::Ephemeral,
            mode: BrowserMode::DiagnosticDebug,
            viewport: Viewport::default(),
            timezone: Some("UTC".to_string()),
            locale: Some("en-US".to_string()),
            record_console: true,
            record_network: true,
            allow_downloads: false,
            allow_uploads: false,
            start_url: Some("https://example.com".to_string()),
        }
    }

    const fn create_response() -> &'static str {
        r#"{"request_id":"req-1","session_id":"br_1","ok":true,"browser":{"browser_id":"chromium-1","page_id":"page-1","cdp_connected":true},"viewport":{"width":1365,"height":768,"device_scale_factor":1.0},"artifact_root":"browser/task-1/br_1/","error":null}"#
    }
}
