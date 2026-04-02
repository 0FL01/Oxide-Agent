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
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use reqwest::{header::CONTENT_TYPE, Method};
use serde::{Deserialize, Serialize};
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
const TOOL_EXTRACT_CONTENT: &str = "browser_use_extract_content";
const TOOL_SCREENSHOT: &str = "browser_use_screenshot";
const MINIMAX_DEFAULT_API_BASE: &str = "https://api.minimax.io/anthropic";
const OPENROUTER_DEFAULT_API_BASE: &str = "https://openrouter.ai/api/v1";
const OXIDE_BROWSER_LLM_API_KEY_HEADER: &str = "x-oxide-browser-llm-api-key";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisionRequirement {
    NotNeeded,
    Recommended,
    Required,
}

/// Provider for Browser Use bridge tools.
pub struct BrowserUseProvider {
    base_url: String,
    client: reqwest::Client,
    settings: Arc<crate::config::AgentSettings>,
    profile_scope: Option<String>,
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
                "Extract text or HTML from the current page of an active Browser Use session."
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
                "Capture a screenshot from the current page of an active Browser Use session."
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
                "Browser Use task appears to require visual grounding, but inherited route `{route_label}` is text-only. Switch to a vision-capable route such as Gemini or a vision-capable OpenRouter model, or simplify the task to text-only browsing/extraction."
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

    async fn execute_run_task(
        &self,
        arguments: &str,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let args: RunTaskArgs = serde_json::from_str(arguments)?;
        if args.task.trim().is_empty() {
            return Err(anyhow!("browser_use_run_task requires a non-empty task"));
        }

        let inherited_llm = self.browser_llm_config_for_request()?;
        let (browser_llm_config, browser_llm_api_key) = match inherited_llm {
            Some((config, api_key)) => (Some(config), Some(api_key)),
            None => (None, None),
        };
        let vision_warning = self.vision_policy_warning(&args.task, browser_llm_config.as_ref())?;
        let profile_scope = self.request_profile_scope(&args)?;
        let body = serde_json::to_value(RunTaskRequestBody {
            task: args.task,
            start_url: args.start_url,
            session_id: args.session_id,
            timeout_secs: args.timeout_secs,
            reuse_profile: args.reuse_profile.unwrap_or(false),
            profile_id: args.profile_id,
            profile_scope,
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
        let output = format_tool_output(payload);
        if let Some(warning) = vision_warning {
            Ok(format!("{warning}\n\n{output}"))
        } else {
            Ok(output)
        }
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
        Ok(format_tool_output(payload))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_llm_config: Option<BrowserLlmConfig>,
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

fn classify_vision_requirement(task: &str) -> VisionRequirement {
    let task = task.to_ascii_lowercase();

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
    ];
    if recommended_keywords
        .iter()
        .any(|needle| task.contains(needle))
    {
        return VisionRequirement::Recommended;
    }

    VisionRequirement::NotNeeded
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
