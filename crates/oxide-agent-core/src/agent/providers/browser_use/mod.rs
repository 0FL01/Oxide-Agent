//! Browser Use provider - high-level browser automation via a self-hosted HTTP bridge.
//!
//! Provides Browser Use session automation and inspection tools via the bridge sidecar.

mod response;

#[cfg(test)]
mod tests;

use crate::agent::provider::ToolProvider;
use crate::agent::tool_runtime::current_tool_model_route;
use crate::config::{
    get_browser_use_initial_backoff, get_browser_use_max_backoff, get_browser_use_max_concurrent,
    get_browser_use_max_retries, get_browser_use_timeout,
};
use crate::llm::ToolDefinition;
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use lazy_regex::lazy_regex;
use reqwest::{header::CONTENT_TYPE, Method};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shell_escape::escape;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use response::{
    format_http_error, format_tool_output, is_json_response, is_retryable_error, ResponsePayload,
};

const TOOL_RUN_TASK: &str = "browser_use_run_task";
const TOOL_GET_SESSION: &str = "browser_use_get_session";
const TOOL_CLOSE_SESSION: &str = "browser_use_close_session";
const TOOL_EXTRACT_CONTENT: &str = "browser_use_extract_content";
const TOOL_SCREENSHOT: &str = "browser_use_screenshot";
const MINIMAX_DEFAULT_API_BASE: &str = "https://api.minimax.io/anthropic";
const OPENROUTER_DEFAULT_API_BASE: &str = "https://openrouter.ai/api/v1";
const OXIDE_BROWSER_LLM_API_KEY_HEADER: &str = "x-oxide-browser-llm-api-key";
const BROWSER_USE_UNSTABLE_VISUAL_ROUTES_ENV: &str = "BROWSER_USE_UNSTABLE_VISUAL_ROUTES";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisionRequirement {
    NotNeeded,
    Recommended,
    Required,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RunTaskSteering {
    prefer_screenshot_tool: bool,
    prefer_extract_tool: bool,
    prefer_visual_description: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum RunTaskExecutionMode {
    Autonomous,
    NavigationOnly,
}

impl RunTaskSteering {
    fn is_empty(self) -> bool {
        !self.prefer_screenshot_tool && !self.prefer_extract_tool && !self.prefer_visual_description
    }
}

/// Provider for Browser Use bridge tools.
pub struct BrowserUseProvider {
    base_url: String,
    client: reqwest::Client,
    settings: Arc<crate::config::AgentSettings>,
    profile_scope: Option<String>,
    sandbox: Arc<Mutex<Option<SandboxManager>>>,
    sandbox_scope: Option<SandboxScope>,
    timeout: Duration,
    max_retries: usize,
    initial_backoff: Duration,
    max_backoff: Duration,
    semaphore: Option<Arc<Semaphore>>,
}

impl BrowserUseProvider {
    fn run_task_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_RUN_TASK.to_string(),
            description: "Run a high-level browser automation task via the self-hosted Browser Use bridge. Use when a real browser is needed for navigation, login, or interactive flows. Prefer `browser_use_screenshot` and `browser_use_extract_content` for the final screenshot or page-content capture once the session is on the target page.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Browser task instruction focused on navigation or interaction; avoid asking this tool to produce the final screenshot or raw page extraction when dedicated follow-up tools can do that after the session is ready"
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
                    },
                    "reuse_profile": {
                        "type": "boolean",
                        "description": "Create and attach a reusable browser profile when starting a new profiled session"
                    },
                    "profile_id": {
                        "type": "string",
                        "description": "Reuse a previously returned Browser Use profile ID"
                    }
                },
                "required": ["task"]
            }),
        }
    }

    fn get_session_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_GET_SESSION.to_string(),
            description: "Get the current state of a Browser Use session by session ID."
                .to_string(),
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
        }
    }

    fn close_session_definition() -> ToolDefinition {
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
        }
    }

    fn extract_content_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_EXTRACT_CONTENT.to_string(),
            description:
                "Extract text or HTML from the current page of an active Browser Use session after `browser_use_run_task` has navigated to the target page."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Browser Use session ID"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "html"],
                        "description": "Content format to extract, defaults to text"
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": "Optional truncation limit for extracted content"
                    }
                },
                "required": ["session_id"]
            }),
        }
    }

    fn screenshot_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_SCREENSHOT.to_string(),
            description:
                "Capture a screenshot from the current page of an active Browser Use session after `browser_use_run_task` has navigated to the target page."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "session_id": {
                        "type": "string",
                        "description": "Browser Use session ID"
                    },
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full page when supported by the browser runtime"
                    }
                },
                "required": ["session_id"]
            }),
        }
    }

    /// Create a new Browser Use provider with default config and shared semaphore.
    #[must_use]
    pub fn new_with_semaphore(
        base_url: &str,
        settings: Arc<crate::config::AgentSettings>,
        semaphore: Arc<Semaphore>,
    ) -> Self {
        let timeout = Duration::from_secs(get_browser_use_timeout());
        let max_retries = get_browser_use_max_retries();
        let initial_backoff = Duration::from_secs(get_browser_use_initial_backoff());
        let max_backoff = Duration::from_secs(get_browser_use_max_backoff());
        Self::with_config_and_semaphore(
            base_url,
            settings,
            timeout,
            max_retries,
            initial_backoff,
            max_backoff,
            Some(semaphore),
        )
    }

    /// Create a new Browser Use provider with default config and no semaphore.
    #[must_use]
    pub fn new(base_url: &str, settings: Arc<crate::config::AgentSettings>) -> Self {
        let timeout = Duration::from_secs(get_browser_use_timeout());
        let max_retries = get_browser_use_max_retries();
        let initial_backoff = Duration::from_secs(get_browser_use_initial_backoff());
        let max_backoff = Duration::from_secs(get_browser_use_max_backoff());
        Self::with_config_and_semaphore(
            base_url,
            settings,
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
        settings: Arc<crate::config::AgentSettings>,
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
            settings,
            profile_scope: None,
            sandbox: Arc::new(Mutex::new(None)),
            sandbox_scope: None,
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
        settings: Arc<crate::config::AgentSettings>,
        timeout: Duration,
        max_retries: usize,
        initial_backoff: Duration,
        max_backoff: Duration,
    ) -> Self {
        Self::with_config_and_semaphore(
            base_url,
            settings,
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

    /// Attach a runtime-injected profile scope for persistent profile reuse.
    #[must_use]
    pub fn with_profile_scope(mut self, profile_scope: impl Into<String>) -> Self {
        let profile_scope = profile_scope.into();
        if !profile_scope.trim().is_empty() {
            self.profile_scope = Some(profile_scope);
        }
        self
    }

    /// Attach sandbox scope so screenshot artifacts can be materialized for file tools.
    #[must_use]
    pub fn with_sandbox_scope(mut self, scope: impl Into<SandboxScope>) -> Self {
        self.sandbox_scope = Some(scope.into());
        self
    }

    fn endpoint_url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    fn runtime_profile_scope(&self) -> Option<&str> {
        self.profile_scope
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
    }

    fn request_profile_scope(&self, args: &RunTaskArgs) -> Result<Option<String>> {
        if args.reuse_profile.unwrap_or(false) || args.profile_id.is_some() {
            let profile_scope = self.runtime_profile_scope().ok_or_else(|| {
                anyhow!(
                    "Browser Use profile reuse requires a topic-scoped runtime context; retry from a topic-bound session or omit reuse_profile/profile_id"
                )
            })?;
            return Ok(Some(profile_scope.to_string()));
        }

        Ok(None)
    }

    fn browser_llm_config_for_active_route(&self) -> Result<Option<(BrowserLlmConfig, String)>> {
        current_tool_model_route()
            .map(|route| self.browser_llm_config_for_route(&route))
            .transpose()
    }

    fn browser_llm_config_for_request(&self) -> Result<Option<(BrowserLlmConfig, String)>> {
        if let Some(route) = self.settings.get_configured_browser_use_model() {
            return self.browser_llm_config_for_route(&route).map(Some);
        }

        self.browser_llm_config_for_active_route()
    }

    fn browser_llm_config_for_route(
        &self,
        route: &crate::config::ModelInfo,
    ) -> Result<(BrowserLlmConfig, String)> {
        let provider = route.provider.to_ascii_lowercase();
        let supports_vision = route_supports_vision(&provider, &route.id);
        let supports_tools = !matches!(provider.as_str(), "groq");

        let (bridge_provider, api_base, api_key) = match provider.as_str() {
            "gemini" => (
                "google",
                None,
                self.require_route_api_key("gemini", self.settings.gemini_api_key.as_deref())?,
            ),
            "minimax" => (
                "minimax",
                Some(MINIMAX_DEFAULT_API_BASE.to_string()),
                self.require_route_api_key("minimax", self.settings.minimax_api_key.as_deref())?,
            ),
            "zai" => (
                "zai",
                Some(self.settings.zai_api_base.clone()),
                self.require_route_api_key("zai", self.settings.zai_api_key.as_deref())?,
            ),
            "openrouter" => (
                "openrouter",
                Some(OPENROUTER_DEFAULT_API_BASE.to_string()),
                self.require_route_api_key(
                    "openrouter",
                    self.settings.openrouter_api_key.as_deref(),
                )?,
            ),
            unsupported => {
                return Err(anyhow!(
                    "Browser Use route inheritance does not support provider `{unsupported}` yet; supported routes: gemini, minimax, zai, openrouter"
                ));
            }
        };

        Ok((
            BrowserLlmConfig {
                provider: bridge_provider.to_string(),
                model: route.id.clone(),
                api_base,
                api_key_ref: None,
                supports_vision,
                supports_tools,
            },
            api_key,
        ))
    }

    fn require_route_api_key(&self, provider: &str, api_key: Option<&str>) -> Result<String> {
        let api_key = api_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "Browser Use route inheritance requires configured credential for provider `{provider}`"
                )
            })?;
        Ok(api_key.to_string())
    }

    fn vision_policy_warning(
        &self,
        task: &str,
        browser_llm_config: Option<&BrowserLlmConfig>,
    ) -> Result<Option<String>> {
        let Some(config) = browser_llm_config else {
            return Ok(None);
        };
        if config.supports_vision {
            return Ok(None);
        }

        let route_label = format!("{}/{}", config.provider, config.model);
        match classify_vision_requirement(task) {
            VisionRequirement::NotNeeded => Ok(None),
            VisionRequirement::Recommended => Ok(Some(format!(
                "Warning: Browser Use is running with text-only route `{route_label}`. This task looks UI-heavy, so execution may run in degraded mode without visual grounding."
            ))),
            VisionRequirement::Required => Err(anyhow!(
                "Browser Use task appears to require visual grounding, but selected route `{route_label}` is text-only. Switch to a vision-capable route such as Gemini, `zai/GLM-4.6V`, or a vision-capable OpenRouter model, or simplify the task to text-only browsing/extraction."
            )),
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        browser_llm_api_key: Option<&str>,
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
                .do_request(
                    method.clone(),
                    &url,
                    body.as_ref(),
                    browser_llm_api_key,
                    cancellation_token,
                )
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
        browser_llm_api_key: Option<&str>,
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
        if let Some(api_key) = browser_llm_api_key {
            request = request.header(OXIDE_BROWSER_LLM_API_KEY_HEADER, api_key);
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

    async fn request_bytes(
        &self,
        method: Method,
        path: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<u8>> {
        let url = self.endpoint_url(path);
        let mut last_error = None;
        let mut backoff = self.initial_backoff;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                await_with_cancellation(
                    sleep(backoff),
                    cancellation_token,
                    "Browser Use request cancelled",
                )
                .await?;
                backoff = Duration::min(backoff * 2, self.max_backoff);
            }

            match self
                .do_request_bytes(method.clone(), &url, cancellation_token)
                .await
            {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    let retryable = is_retryable_error(&error.to_string());
                    if !retryable {
                        return Err(error);
                    }
                    warn!(
                        url = %url,
                        attempt,
                        error = %error,
                        "Browser Use binary request failed, will retry",
                    );
                    last_error = Some(error);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Browser Use request failed")))
    }

    async fn do_request_bytes(
        &self,
        method: Method,
        url: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<Vec<u8>> {
        let request = self.client.request(method, url);
        let response = await_with_cancellation(
            request.send(),
            cancellation_token,
            "Browser Use request cancelled",
        )
        .await?
        .map_err(|error| anyhow!("Browser Use request failed: {error}"))?;

        let status = response.status();
        let bytes = await_with_cancellation(
            response.bytes(),
            cancellation_token,
            "Browser Use request cancelled",
        )
        .await?
        .map_err(|error| anyhow!("Browser Use response read failed: {error}"))?;

        if !status.is_success() {
            let text = String::from_utf8_lossy(bytes.as_ref()).to_string();
            return Err(anyhow!(format_http_error(status, &text)));
        }

        Ok(bytes.to_vec())
    }

    async fn ensure_sandbox(&self) -> Result<()> {
        if self
            .sandbox
            .lock()
            .await
            .as_ref()
            .is_some_and(SandboxManager::is_running)
        {
            return Ok(());
        }

        let sandbox_scope = self
            .sandbox_scope
            .clone()
            .ok_or_else(|| anyhow!("Sandbox scope is not configured for Browser Use"))?;
        let mut sandbox = SandboxManager::new(sandbox_scope).await?;
        sandbox.create_sandbox().await?;
        *self.sandbox.lock().await = Some(sandbox);
        Ok(())
    }

    async fn write_screenshot_file(
        &self,
        session_id: &str,
        file_name: &str,
        content: &[u8],
    ) -> Result<String> {
        self.ensure_sandbox().await?;
        let mut sandbox = {
            let guard = self.sandbox.lock().await;
            guard
                .as_ref()
                .cloned()
                .ok_or_else(|| anyhow!("Sandbox not initialized"))?
        };

        let sanitized_name = Path::new(file_name)
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow!("Browser Use screenshot is missing a valid file name"))?;
        let sandbox_path = format!("/workspace/browser_use/{session_id}/{sanitized_name}");
        ensure_parent_dir(&mut sandbox, &sandbox_path).await?;
        sandbox.write_file(&sandbox_path, content).await?;
        *self.sandbox.lock().await = Some(sandbox);
        Ok(sandbox_path)
    }

    async fn hydrate_screenshot_payload(
        &self,
        payload: ResponsePayload,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<ResponsePayload> {
        if self.sandbox_scope.is_none() {
            return Ok(payload);
        }

        let ResponsePayload::Json(mut value) = payload else {
            return Ok(payload);
        };

        let Some(session_id) = value
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
        else {
            return Ok(ResponsePayload::Json(value));
        };

        let Some(artifact) = value.get_mut("artifact").and_then(Value::as_object_mut) else {
            return Ok(ResponsePayload::Json(value));
        };

        let Some(download_path) = artifact
            .get("download_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
        else {
            return Ok(ResponsePayload::Json(value));
        };

        let file_name = artifact
            .get("file_name")
            .and_then(Value::as_str)
            .or_else(|| artifact.get("artifact_id").and_then(Value::as_str))
            .or_else(|| {
                Path::new(download_path.as_str())
                    .file_name()
                    .and_then(|value| value.to_str())
            })
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Browser Use screenshot response is missing file_name"))?;
        let bridge_path = artifact
            .get("path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let bytes = self
            .request_bytes(Method::GET, download_path.as_str(), cancellation_token)
            .await?;
        let sandbox_path = self
            .write_screenshot_file(session_id.as_str(), file_name.as_str(), &bytes)
            .await?;

        if let Some(path) = bridge_path {
            artifact.insert("bridge_path".to_string(), Value::String(path));
        }
        artifact.insert("path".to_string(), Value::String(sandbox_path.clone()));
        artifact.insert("sandbox_path".to_string(), Value::String(sandbox_path));

        Ok(ResponsePayload::Json(value))
    }

    async fn execute_run_task(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let args: RunTaskArgs = serde_json::from_str(arguments)?;
        if args.task.trim().is_empty() {
            return Err(anyhow!("browser_use_run_task requires a non-empty task"));
        }
        let steering = classify_run_task_steering(&args.task);
        let vision_task = vision_policy_task(&args.task, steering);
        let rewritten_task = rewrite_run_task_instruction(&args.task, steering);
        if !steering.is_empty() {
            debug!(
                prefer_screenshot_tool = steering.prefer_screenshot_tool,
                prefer_extract_tool = steering.prefer_extract_tool,
                prefer_visual_description = steering.prefer_visual_description,
                "Browser Use run_task split into navigation-only with follow-up guidance"
            );
        }

        let inherited_llm = self.browser_llm_config_for_request()?;
        let (browser_llm_config, browser_llm_api_key) = match inherited_llm {
            Some((config, api_key)) => (Some(config), Some(api_key)),
            None => (None, None),
        };
        self.fast_fail_unstable_autonomous_visual_route(
            &args.task,
            steering,
            browser_llm_config.as_ref(),
        )?;
        let vision_warning =
            self.vision_policy_warning(&vision_task, browser_llm_config.as_ref())?;
        if let Some(warning) = vision_warning.as_ref() {
            warn!(
                warning = %warning,
                "Browser Use run_task proceeds on text-only route in degraded mode"
            );
        }
        let profile_scope = self.request_profile_scope(&args)?;
        let body = serde_json::to_value(RunTaskRequestBody {
            task: rewritten_task,
            start_url: args.start_url,
            session_id: args.session_id,
            timeout_secs: args.timeout_secs,
            reuse_profile: args.reuse_profile.unwrap_or(false),
            profile_id: args.profile_id,
            profile_scope,
            execution_mode: execution_mode_for_steering(steering),
            browser_llm_config,
        })?;
        let payload = self
            .request(
                Method::POST,
                "/sessions/run",
                Some(body),
                browser_llm_api_key.as_deref(),
                cancellation_token,
            )
            .await?;
        let follow_up_guidance = run_task_follow_up_guidance(steering, &payload);
        let mut output = format_tool_output(payload);
        if let Some(guidance) = follow_up_guidance {
            output = format!("{output}\n\n{guidance}");
        }
        if let Some(warning) = vision_warning {
            Ok(format!("{warning}\n\n{output}"))
        } else {
            Ok(output)
        }
    }

    fn fast_fail_unstable_autonomous_visual_route(
        &self,
        task: &str,
        steering: RunTaskSteering,
        browser_llm_config: Option<&BrowserLlmConfig>,
    ) -> Result<()> {
        if !steering.is_empty() {
            return Ok(());
        }

        if classify_vision_requirement(task) != VisionRequirement::Required {
            return Ok(());
        }

        let Some(config) = browser_llm_config else {
            return Ok(());
        };

        if !is_unstable_autonomous_visual_route(config) {
            return Ok(());
        }

        warn!(
            provider = %config.provider,
            model = %config.model,
            task = %task,
            "Browser Use autonomous visual task blocked on unstable route"
        );

        Err(anyhow!(
            "Browser Use autonomous visual tasks are disabled on route `{}/{}` because this route is unstable for strict structured browser-use outputs and can stall on expensive retries. Run navigation-only first, then use `browser_use_screenshot` + `describe_image_file` (or `browser_use_extract_content`) for the final result.",
            config.provider,
            config.model,
        ))
    }

    async fn execute_get_session(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
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
                None,
                cancellation_token,
            )
            .await?;
        Ok(format_tool_output(payload))
    }

    async fn execute_close_session(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
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
                None,
                cancellation_token,
            )
            .await?;
        Ok(format_tool_output(payload))
    }

    async fn execute_extract_content(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let args: ExtractContentArgs = serde_json::from_str(arguments)?;
        let session_id = args.session_id.trim();
        if session_id.is_empty() {
            return Err(anyhow!("browser_use_extract_content requires a session_id"));
        }
        let format = args
            .format
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("text");
        if !matches!(format, "text" | "html") {
            return Err(anyhow!(
                "browser_use_extract_content format must be either 'text' or 'html'"
            ));
        }

        let payload = self
            .request(
                Method::POST,
                &format!("/sessions/{session_id}/extract_content"),
                Some(serde_json::to_value(ExtractContentRequestBody {
                    format: format.to_string(),
                    max_chars: args.max_chars,
                })?),
                None,
                cancellation_token,
            )
            .await?;
        Ok(format_tool_output(payload))
    }

    async fn execute_screenshot(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let args: ScreenshotArgs = serde_json::from_str(arguments)?;
        let session_id = args.session_id.trim();
        if session_id.is_empty() {
            return Err(anyhow!("browser_use_screenshot requires a session_id"));
        }

        let payload = self
            .request(
                Method::POST,
                &format!("/sessions/{session_id}/screenshot"),
                Some(serde_json::to_value(ScreenshotRequestBody {
                    full_page: args.full_page.unwrap_or(false),
                })?),
                None,
                cancellation_token,
            )
            .await?;
        let payload = self
            .hydrate_screenshot_payload(payload, cancellation_token)
            .await?;
        Ok(format_tool_output(payload))
    }
}

async fn ensure_parent_dir(sandbox: &mut SandboxManager, path: &str) -> Result<()> {
    let parent = Path::new(path).parent().map_or_else(
        || "/workspace".to_string(),
        |value| value.to_string_lossy().to_string(),
    );
    let command = format!("mkdir -p {}", escape(parent.as_str().into()));
    let result = sandbox.exec_command(&command, None).await?;
    if result.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "Failed to create Browser Use artifact directory {parent}: {}",
            result.combined_output()
        )
    }
}

#[derive(Debug, Deserialize)]
struct RunTaskArgs {
    task: String,
    start_url: Option<String>,
    session_id: Option<String>,
    timeout_secs: Option<u64>,
    reuse_profile: Option<bool>,
    profile_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct BrowserLlmConfig {
    provider: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key_ref: Option<String>,
    supports_vision: bool,
    supports_tools: bool,
}

#[derive(Debug, Serialize)]
struct RunTaskRequestBody {
    task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reuse_profile: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_scope: Option<String>,
    execution_mode: RunTaskExecutionMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_llm_config: Option<BrowserLlmConfig>,
}

fn execution_mode_for_steering(steering: RunTaskSteering) -> RunTaskExecutionMode {
    if steering.is_empty() {
        RunTaskExecutionMode::Autonomous
    } else {
        RunTaskExecutionMode::NavigationOnly
    }
}

#[derive(Debug, Deserialize)]
struct ExtractContentArgs {
    session_id: String,
    format: Option<String>,
    max_chars: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ExtractContentRequestBody {
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_chars: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ScreenshotArgs {
    session_id: String,
    full_page: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ScreenshotRequestBody {
    full_page: bool,
}

fn route_supports_vision(provider: &str, model: &str) -> bool {
    match provider {
        "gemini" => true,
        "zai" => is_zai_vision_model(model),
        "openrouter" => is_openrouter_vision_model(model),
        _ => false,
    }
}

fn is_zai_vision_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    ["glm-4.6v", "glm-4v"]
        .iter()
        .any(|needle| model.contains(needle))
}

fn is_openrouter_vision_model(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    [
        "gemini",
        "gpt-4o",
        "gpt-4.1",
        "claude-3",
        "claude-sonnet-4",
        "claude-opus-4",
        "vision",
        "vl",
        "pixtral",
        "llama-4",
        "qwen-vl",
    ]
    .iter()
    .any(|needle| model.contains(needle))
}

fn is_unstable_autonomous_visual_route(config: &BrowserLlmConfig) -> bool {
    let route = format!(
        "{}/{}",
        config.provider.to_ascii_lowercase(),
        config.model.to_ascii_lowercase()
    );

    unstable_visual_route_patterns()
        .iter()
        .any(|pattern| route_matches_pattern(&route, pattern))
}

fn unstable_visual_route_patterns() -> Vec<String> {
    let parsed = std::env::var(BROWSER_USE_UNSTABLE_VISUAL_ROUTES_ENV)
        .ok()
        .map(|raw| parse_unstable_visual_route_patterns(&raw));

    match parsed {
        Some(patterns) if !patterns.is_empty() => patterns,
        Some(_) => Vec::new(),
        None => vec!["zai/glm-4.6v".to_string()],
    }
}

fn parse_unstable_visual_route_patterns(raw: &str) -> Vec<String> {
    let normalized = raw.trim().to_ascii_lowercase();
    if matches!(normalized.as_str(), "off" | "none" | "disabled") {
        return Vec::new();
    }

    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect()
}

fn route_matches_pattern(route: &str, pattern: &str) -> bool {
    if pattern.contains('/') {
        route.contains(pattern)
    } else {
        route
            .split('/')
            .nth(1)
            .is_some_and(|model| model.contains(pattern))
    }
}

fn classify_vision_requirement(task: &str) -> VisionRequirement {
    let task = task.to_lowercase();

    let required_keywords = [
        "visual",
        "visually",
        "appearance",
        "layout",
        "look like",
        "on screen",
        "screenshot",
        "color",
        "colour",
        "icon",
        "captcha",
        "визуал",
        "внешн",
        "выгляд",
        "на экране",
        "скриншот",
        "скрин",
        "цвет",
        "иконк",
        "капча",
    ];
    if required_keywords.iter().any(|needle| task.contains(needle)) {
        return VisionRequirement::Required;
    }

    let recommended_keywords = [
        "click",
        "button",
        "dropdown",
        "menu",
        "modal",
        "dialog",
        "form",
        "checkbox",
        "radio",
        "tab",
        "sign in",
        "log in",
        "upload",
        "drag",
        "interactive",
        "wizard",
        "клик",
        "нажм",
        "кнопк",
        "выпада",
        "меню",
        "модал",
        "диалог",
        "форм",
        "чекбокс",
        "радиокноп",
        "вкладк",
        "войти",
        "логин",
        "загруз",
        "перетян",
        "интерактив",
    ];
    if recommended_keywords
        .iter()
        .any(|needle| task.contains(needle))
    {
        return VisionRequirement::Recommended;
    }

    VisionRequirement::NotNeeded
}

fn classify_run_task_steering(task: &str) -> RunTaskSteering {
    let task = task.to_lowercase();

    let prefer_screenshot_tool = [
        "screenshot",
        "screen shot",
        "png",
        "snapshot",
        "capture image",
        "screen capture",
        "скриншот",
        "скрин",
        "снимок экрана",
    ]
    .iter()
    .any(|needle| task.contains(needle));

    let prefer_extract_tool = [
        "extract text",
        "extract html",
        "extract content",
        "page text",
        "page html",
        "raw html",
        "html source",
        "page source",
        "full text",
        "извлеки текст",
        "извлеки html",
        "извлеки контент",
        "текст страницы",
        "html страницы",
        "содержимое страницы",
        "исходный код страницы",
    ]
    .iter()
    .any(|needle| task.contains(needle));

    let prefer_visual_description = [
        "describe what you see",
        "what do you see",
        "how it looks",
        "how does it look",
        "visual description",
        "visually describe",
        "describe the visual",
        "describe the layout",
        "describe the appearance",
        "appearance",
        "layout",
        "look like",
        "on screen",
        "what's on the screen",
        "screen contents",
        "color scheme",
        "colors",
        "colours",
        "опиши что видишь",
        "что ты видишь",
        "как выглядит",
        "опиши визуально",
        "визуально",
        "внешний вид",
        "что на экране",
        "цвета",
        "цветовую схему",
        "раскладку страницы",
    ]
    .iter()
    .any(|needle| task.contains(needle));

    RunTaskSteering {
        prefer_screenshot_tool,
        prefer_extract_tool,
        prefer_visual_description,
    }
}

fn rewrite_run_task_instruction(task: &str, steering: RunTaskSteering) -> String {
    if steering.is_empty() {
        return task.to_string();
    }

    let navigation_goal = navigation_goal_from_task(task, steering);
    let follow_up_plan = follow_up_plan_for_task(steering);

    format!(
        "Browser Use execution rules for this run:\n- Use this step only for navigation and interaction needed to reach the target page or UI state.\n- Do not take screenshots, describe the final visual result, save PDFs, or perform final page-content extraction in this step.\n- Leave the session on the target page for Oxide follow-up tools.\n- Return a short navigation/status summary only.\n\nNavigation goal for this run:\n{navigation_goal}\n\nOxide follow-up plan after this run:\n{follow_up_plan}"
    )
}

fn run_task_follow_up_guidance(
    steering: RunTaskSteering,
    payload: &ResponsePayload,
) -> Option<String> {
    if steering.is_empty() {
        return None;
    }

    let session_id = match payload {
        ResponsePayload::Json(value) => extract_session_id_from_value(value),
        ResponsePayload::Text(text) => extract_session_id(text),
    };
    let session_ref = session_id
        .map(|id| format!(" with `session_id` `{id}`"))
        .unwrap_or_default();

    if let Some((false, dead_reason)) = extract_browser_runtime_state(payload) {
        let dead_reason_suffix = dead_reason
            .filter(|reason| !reason.trim().is_empty())
            .map(|reason| format!(" Reported reason: `{reason}`."))
            .unwrap_or_default();
        return Some(format!(
            "Follow-up note: Browser Use reported that this session runtime{session_ref} is no longer alive. Re-run `browser_use_run_task` before calling `browser_use_screenshot` or `browser_use_extract_content`.{dead_reason_suffix}"
        ));
    }

    let mut steps = Vec::new();
    if steering.prefer_screenshot_tool || steering.prefer_visual_description {
        steps.push(format!(
            "- Use `browser_use_screenshot`{session_ref} to capture the final page state."
        ));
    }
    if steering.prefer_visual_description {
        steps.push(
            "- Then pass the returned `artifact.path` from `browser_use_screenshot` to `describe_image_file` for the final visual description."
                .to_string(),
        );
    }
    if steering.prefer_extract_tool {
        steps.push(format!(
            "- Use `browser_use_extract_content`{session_ref} for the final text or HTML extraction."
        ));
    }

    if steps.is_empty() {
        None
    } else {
        Some(format!("Follow-up guidance:\n{}", steps.join("\n")))
    }
}

fn navigation_goal_from_task(task: &str, steering: RunTaskSteering) -> String {
    if let Some(prefix) = strip_follow_up_clause(task, steering) {
        return prefix;
    }

    if let Some(url) = first_url(task) {
        return format!("Open {url} and leave the target page ready for Oxide follow-up tools.");
    }

    if steering.prefer_extract_tool {
        return "Navigate to the page that contains the requested content and stop once the target page is ready for follow-up extraction.".to_string();
    }

    if steering.prefer_visual_description {
        return "Navigate to the page or UI state the user requested and stop once it is ready for Oxide follow-up tools.".to_string();
    }

    "Navigate to the page or UI state the user wants captured and stop once it is ready for Oxide follow-up tools.".to_string()
}

fn vision_policy_task(task: &str, steering: RunTaskSteering) -> String {
    if steering.is_empty() {
        task.to_string()
    } else {
        navigation_goal_from_task(task, steering)
    }
}

fn follow_up_plan_for_task(steering: RunTaskSteering) -> String {
    let mut steps = Vec::new();
    if steering.prefer_screenshot_tool || steering.prefer_visual_description {
        steps.push("- Oxide will call `browser_use_screenshot` after navigation is complete.");
    }
    if steering.prefer_visual_description {
        steps.push(
            "- Oxide will then call `describe_image_file` on the screenshot path returned by `browser_use_screenshot`.",
        );
    }
    if steering.prefer_extract_tool {
        steps.push("- Oxide will call `browser_use_extract_content` after navigation is complete.");
    }
    steps.join("\n")
}

fn strip_follow_up_clause(task: &str, steering: RunTaskSteering) -> Option<String> {
    let task_lower = task.to_lowercase();
    let mut cut_index: Option<usize> = None;

    for marker in follow_up_markers(steering) {
        if let Some(index) = task_lower.find(marker) {
            cut_index = Some(cut_index.map_or(index, |current| current.min(index)));
        }
    }

    let cut_index = cut_index?;
    let prefix = task.get(..cut_index)?.trim_end();
    let prefix = trim_follow_up_prefix(prefix);
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

fn follow_up_markers(steering: RunTaskSteering) -> Vec<&'static str> {
    let mut markers = Vec::new();
    if steering.prefer_screenshot_tool {
        markers.extend([
            "take a screenshot",
            "take screenshot",
            "capture a screenshot",
            "capture screenshot",
            "screen shot",
            "screen capture",
            "snapshot",
            "скриншот",
            "скрин",
            "снимок экрана",
        ]);
    }
    if steering.prefer_extract_tool {
        markers.extend([
            "extract text",
            "extract html",
            "extract content",
            "page text",
            "page html",
            "raw html",
            "html source",
            "page source",
            "full text",
            "извлеки текст",
            "извлеки html",
            "извлеки контент",
            "текст страницы",
            "html страницы",
            "содержимое страницы",
            "исходный код страницы",
        ]);
    }
    if steering.prefer_visual_description {
        markers.extend([
            "describe what you see",
            "what do you see",
            "how it looks",
            "how does it look",
            "visual description",
            "visually describe",
            "describe the visual",
            "describe the layout",
            "describe the appearance",
            "appearance",
            "layout",
            "look like",
            "what's on the screen",
            "screen contents",
            "color scheme",
            "colors",
            "colours",
            "опиши что видишь",
            "что ты видишь",
            "как выглядит",
            "опиши визуально",
            "визуально",
            "внешний вид",
            "что на экране",
            "цвета",
        ]);
    }
    markers
}

fn trim_follow_up_prefix(prefix: &str) -> String {
    static RE_TRAILING_JOINERS: lazy_regex::Lazy<regex::Regex> = lazy_regex!(
        r"(?iu)(?:\s|,|;|:|-)*(?:and|then|after that|afterwards|to|for|with|и|затем|потом|чтобы|для|с)\s*$"
    );
    static RE_TRAILING_PUNCT: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?u)[\s,;:.-]+$");

    let trimmed = RE_TRAILING_JOINERS.replace(prefix.trim(), "");
    let trimmed = RE_TRAILING_PUNCT.replace(trimmed.as_ref(), "");
    trimmed.trim().to_string()
}

fn first_url(task: &str) -> Option<&str> {
    static RE_URL: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"https?://\S+");
    RE_URL.find(task).map(|capture| capture.as_str())
}

fn extract_browser_runtime_state(payload: &ResponsePayload) -> Option<(bool, Option<&str>)> {
    let ResponsePayload::Json(value) = payload else {
        return None;
    };

    let alive = value.get("browser_runtime_alive")?.as_bool()?;
    let dead_reason = value
        .get("browser_runtime_dead_reason")
        .and_then(Value::as_str);
    Some((alive, dead_reason))
}

fn extract_session_id_from_value(value: &Value) -> Option<&str> {
    value.get("session_id").and_then(Value::as_str)
}

fn extract_session_id(output: &str) -> Option<&str> {
    let key = "\"session_id\": \"";
    let start = output.find(key)? + key.len();
    let rest = output.get(start..)?;
    let end = rest.find('"')?;
    rest.get(..end)
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
            Self::run_task_definition(),
            Self::get_session_definition(),
            Self::close_session_definition(),
            Self::extract_content_definition(),
            Self::screenshot_definition(),
        ]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        matches!(
            tool_name,
            TOOL_RUN_TASK
                | TOOL_GET_SESSION
                | TOOL_CLOSE_SESSION
                | TOOL_EXTRACT_CONTENT
                | TOOL_SCREENSHOT
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
            TOOL_RUN_TASK => self.execute_run_task(arguments, cancellation_token).await,
            TOOL_GET_SESSION => {
                self.execute_get_session(arguments, cancellation_token)
                    .await
            }
            TOOL_CLOSE_SESSION => {
                self.execute_close_session(arguments, cancellation_token)
                    .await
            }
            TOOL_EXTRACT_CONTENT => {
                self.execute_extract_content(arguments, cancellation_token)
                    .await
            }
            TOOL_SCREENSHOT => self.execute_screenshot(arguments, cancellation_token).await,
            _ => Err(anyhow!("Unknown Browser Use tool: {tool_name}")),
        }
    }
}
