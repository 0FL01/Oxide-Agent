//! Browser Use provider - high-level browser automation via a self-hosted HTTP bridge.
//!
//! Provides `browser_use_run_task`, `browser_use_get_session`, and
//! `browser_use_close_session` tools via the Browser Use bridge sidecar.

mod response;

#[cfg(test)]
mod tests;

use crate::agent::provider::ToolProvider;
use crate::config::{
    get_browser_use_initial_backoff, get_browser_use_max_backoff, get_browser_use_max_concurrent,
    get_browser_use_max_retries, get_browser_use_timeout,
};
use crate::llm::ToolDefinition;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{header::CONTENT_TYPE, Method};
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use response::{
    format_http_error, format_tool_output, is_json_response, is_retryable_error, ResponsePayload,
};

const TOOL_RUN_TASK: &str = "browser_use_run_task";
const TOOL_GET_SESSION: &str = "browser_use_get_session";
const TOOL_CLOSE_SESSION: &str = "browser_use_close_session";

/// Provider for Browser Use bridge tools.
pub struct BrowserUseProvider {
    base_url: String,
    client: reqwest::Client,
    timeout: Duration,
    max_retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    semaphore: Option<Arc<Semaphore>>,
}

impl BrowserUseProvider {
    /// Create a new Browser Use provider with default config and shared semaphore.
    #[must_use]
    pub fn new_with_semaphore(base_url: &str, semaphore: Arc<Semaphore>) -> Self {
        let timeout = Duration::from_secs(get_browser_use_timeout());
        let max_retries = get_browser_use_max_retries();
        let initial_backoff = Duration::from_secs(get_browser_use_initial_backoff());
        let max_backoff = Duration::from_secs(get_browser_use_max_backoff());
        Self::with_config_and_semaphore(
            base_url,
            timeout,
            max_retries,
            initial_backoff,
            max_backoff,
            Some(semaphore),
        )
    }

    /// Create a new Browser Use provider with default config and no semaphore.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        let timeout = Duration::from_secs(get_browser_use_timeout());
        let max_retries = get_browser_use_max_retries();
        let initial_backoff = Duration::from_secs(get_browser_use_initial_backoff());
        let max_backoff = Duration::from_secs(get_browser_use_max_backoff());
        Self::with_config_and_semaphore(
            base_url,
            timeout,
            max_retries,
            initial_backoff,
            max_backoff,
            None,
        )
    }

    /// Create a Browser Use provider with explicit configuration and optional semaphore.
    #[must_use]
    pub fn with_config_and_semaphore(
        base_url: &str,
        timeout: Duration,
        max_retries: usize,
        initial_backoff: Duration,
        max_backoff: Duration,
        semaphore: Option<Arc<Semaphore>>,
    ) -> Self {
        let client = match reqwest::Client::builder().timeout(timeout).build() {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };

        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
            timeout,
            max_retries,
            initial_backoff,
            max_backoff,
            semaphore,
        }
    }

    /// Create a Browser Use provider with explicit configuration and no semaphore.
    #[must_use]
    pub fn with_config(
        base_url: &str,
        timeout: Duration,
        max_retries: usize,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        Self::with_config_and_semaphore(
            base_url,
            timeout,
            max_retries,
            initial_backoff,
            max_backoff,
            None,
        )
    }

    /// Get configured max concurrent requests.
    #[must_use]
    pub fn max_concurrent() -> usize {
        get_browser_use_max_concurrent()
    }

    /// Create a semaphore with configured max permits.
    #[must_use]
    pub fn create_semaphore() -> Arc<Semaphore> {
        Arc::new(Semaphore::new(get_browser_use_max_concurrent()))
    }

    fn endpoint_url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ResponsePayload> {
        let url = self.endpoint_url(path);
        let mut last_error = None;
        let mut backoff = self.initial_backoff;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                debug!(
                    url = %url,
                    attempt,
                    backoff_secs = backoff.as_secs(),
                    "Browser Use retry",
                );
                await_with_cancellation(
                    sleep(backoff),
                    cancellation_token,
                    "Browser Use request cancelled",
                )
                .await?;
                backoff = Duration::min(backoff * 2, self.max_backoff);
            }

            match self
                .do_request(method.clone(), &url, body.as_ref(), cancellation_token)
                .await
            {
                Ok(payload) => return Ok(payload),
                Err(error) => {
                    let retryable = is_retryable_error(&error.to_string());
                    if !retryable {
                        return Err(error);
                    }
                    warn!(
                        url = %url,
                        attempt,
                        error = %error,
                        "Browser Use request failed, will retry",
                    );
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Browser Use request failed")))
    }

    async fn do_request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ResponsePayload> {
        debug!(
            method = %method,
            url = %url,
            timeout_secs = self.timeout.as_secs(),
            "Browser Use request"
        );

        let mut request = self.client.request(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = await_with_cancellation(
            request.send(),
            cancellation_token,
            "Browser Use request cancelled",
        )
        .await?
        .map_err(|error| anyhow!("Browser Use request failed: {error}"))?;

        let status = response.status();
        if !status.is_success() {
            let text = await_with_cancellation(
                response.text(),
                cancellation_token,
                "Browser Use request cancelled",
            )
            .await?
            .map_err(|error| anyhow!("Browser Use error response read failed: {error}"))?;
            return Err(anyhow!(format_http_error(status, &text)));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = await_with_cancellation(
            response.bytes(),
            cancellation_token,
            "Browser Use request cancelled",
        )
        .await?
        .map_err(|error| anyhow!("Browser Use response read failed: {error}"))?;

        let text = String::from_utf8_lossy(bytes.as_ref()).to_string();
        if is_json_response(&content_type, bytes.as_ref()) {
            match serde_json::from_slice::<Value>(bytes.as_ref()) {
                Ok(value) => Ok(ResponsePayload::Json(value)),
                Err(_) => Ok(ResponsePayload::Text(text)),
            }
        } else {
            Ok(ResponsePayload::Text(text))
        }
    }
}

#[derive(Debug, Deserialize)]
struct RunTaskArgs {
    task: String,
    start_url: Option<String>,
    session_id: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct SessionArgs {
    session_id: String,
}

async fn await_with_cancellation<F, T>(
    future: F,
    cancellation_token: Option<&CancellationToken>,
    cancel_message: &'static str,
) -> Result<T>
where
    F: Future<Output = T>,
{
    if let Some(token) = cancellation_token {
        tokio::select! {
            output = future => Ok(output),
            _ = token.cancelled() => Err(anyhow!(cancel_message)),
        }
    } else {
        Ok(future.await)
    }
}

#[async_trait]
impl ToolProvider for BrowserUseProvider {
    fn name(&self) -> &'static str {
        "browser_use"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_RUN_TASK.to_string(),
                description: "Run a high-level browser automation task via the self-hosted Browser Use bridge. Use when a real browser is needed for dynamic pages, navigation, or interactive flows.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "Browser task instruction"
                        },
                        "start_url": {
                            "type": "string",
                            "description": "Optional starting page URL"
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Optional existing session ID to reuse"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Optional timeout override in seconds"
                        }
                    },
                    "required": ["task"]
                }),
            },
            ToolDefinition {
                name: TOOL_GET_SESSION.to_string(),
                description: "Get the current state of a Browser Use session by session ID.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Browser Use session ID"
                        }
                    },
                    "required": ["session_id"]
                }),
            },
            ToolDefinition {
                name: TOOL_CLOSE_SESSION.to_string(),
                description: "Close a Browser Use session and free browser resources.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Browser Use session ID"
                        }
                    },
                    "required": ["session_id"]
                }),
            },
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_RUN_TASK | TOOL_GET_SESSION | TOOL_CLOSE_SESSION
        )
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        _progress_tx: Option<&tokio::sync::mpsc::Sender<crate::agent::progress::AgentEvent>>,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            return Err(anyhow!("Browser Use request cancelled"));
        }

        let _permit = if let Some(semaphore) = &self.semaphore {
            let permit = semaphore.acquire().await.map_err(|_| {
                anyhow!("Browser Use semaphore acquisition failed (shutdown in progress)")
            })?;
            Some(permit)
        } else {
            None
        };

        match tool_name {
            TOOL_RUN_TASK => {
                let args: RunTaskArgs = serde_json::from_str(arguments)?;
                if args.task.trim().is_empty() {
                    return Err(anyhow!("browser_use_run_task requires a non-empty task"));
                }

                let body = json!({
                    "task": args.task,
                    "start_url": args.start_url,
                    "session_id": args.session_id,
                    "timeout_secs": args.timeout_secs,
                });
                let payload = self
                    .request(
                        Method::POST,
                        "/sessions/run",
                        Some(body),
                        cancellation_token,
                    )
                    .await?;
                Ok(format_tool_output(payload))
            }
            TOOL_GET_SESSION => {
                let args: SessionArgs = serde_json::from_str(arguments)?;
                let session_id = args.session_id.trim();
                if session_id.is_empty() {
                    return Err(anyhow!("browser_use_get_session requires a session_id"));
                }

                let payload = self
                    .request(
                        Method::GET,
                        &format!("/sessions/{session_id}"),
                        None,
                        cancellation_token,
                    )
                    .await?;
                Ok(format_tool_output(payload))
            }
            TOOL_CLOSE_SESSION => {
                let args: SessionArgs = serde_json::from_str(arguments)?;
                let session_id = args.session_id.trim();
                if session_id.is_empty() {
                    return Err(anyhow!("browser_use_close_session requires a session_id"));
                }

                let payload = self
                    .request(
                        Method::DELETE,
                        &format!("/sessions/{session_id}"),
                        None,
                        cancellation_token,
                    )
                    .await?;
                Ok(format_tool_output(payload))
            }
            _ => Err(anyhow!("Unknown Browser Use tool: {tool_name}")),
        }
    }
}
