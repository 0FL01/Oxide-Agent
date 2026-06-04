use crate::config::{get_opencode_go_max_concurrent, OPENCODE_GO_CHAT_TEMPERATURE};
use crate::llm::providers::protocol_profiles::{
    ANTHROPIC_CLIENT_TOOL_PROFILE, CHAT_LIKE_TOOL_PROFILE,
};
use crate::llm::support::http::{create_http_client, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, MessageContentPart,
    TokenUsage, ToolCall, ToolDefinition,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use discovery::{
    ModelProtocol, OpenCodeGoDiscoveryConfig, OpenCodeGoModelCatalog, OPENCODE_GO_PROVIDER_ID,
    OPENCODE_ZEN_PROVIDER_ID,
};
use reqwest::{Client as HttpClient, Url};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{debug, trace, warn};

pub mod discovery;
pub(crate) mod module;
pub(crate) use module::{OpenCodeGoProviderModule, OpenCodeZenProviderModule};

const OPENCODE_GO_FAILURES_BEFORE_COOLDOWN: usize = 3;
const OPENCODE_GO_COOLDOWN_STEP_SECS: u64 = 5;
const OPENCODE_GO_MAX_COOLDOWN_SECS: u64 = 60;
const OPENCODE_GO_SUCCESS_STREAK_TO_INCREASE: usize = 3;
const ANTHROPIC_VERSION_HEADER: &str = "2023-06-01";
const OPENCODE_GO_IMAGE_ANALYSIS_MAX_TOKENS: u32 = 4000;

/// Reasoning effort sent for models that support thinking/CoT parameters.
const OPENCODE_GO_REASONING_EFFORT: &str = "high";

#[derive(Debug, Clone, Copy)]
struct OpenCodeProviderProfile {
    provider_id: &'static str,
    model_prefix: &'static str,
    display_name: &'static str,
    module_id: &'static str,
}

impl OpenCodeProviderProfile {
    const fn go() -> Self {
        Self {
            provider_id: OPENCODE_GO_PROVIDER_ID,
            model_prefix: OPENCODE_GO_PROVIDER_ID,
            display_name: "OpenCode Go",
            module_id: "llm-provider/opencode-go",
        }
    }

    const fn zen() -> Self {
        Self {
            provider_id: OPENCODE_ZEN_PROVIDER_ID,
            model_prefix: OPENCODE_ZEN_PROVIDER_ID,
            display_name: "OpenCode Zen",
            module_id: "llm-provider/opencode-zen",
        }
    }
}

/// LLM provider implementation for OpenCode Go's OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenCodeGoProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
    api_base_messages: String,
    profile: OpenCodeProviderProfile,
    throttle: Arc<OpenCodeGoAdaptiveThrottle>,
    model_catalog: Arc<OpenCodeGoModelCatalog>,
}

impl OpenCodeGoProvider {
    /// Create a new OpenCode Go provider instance.
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        let http_client = create_http_client();
        Self::new_with_client(api_key, api_base, http_client)
    }

    /// Create a new OpenCode Go provider with a shared HTTP client.
    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        let api_base_messages = derive_messages_api_base(&api_base);
        Self::new_with_client_and_discovery(
            api_key,
            api_base,
            api_base_messages,
            http_client,
            OpenCodeGoDiscoveryConfig::from_env(),
        )
    }

    /// Create a new OpenCode Go provider with an explicit model discovery config.
    #[must_use]
    pub fn new_with_client_and_discovery(
        api_key: String,
        api_base: String,
        api_base_messages: String,
        http_client: HttpClient,
        discovery_config: OpenCodeGoDiscoveryConfig,
    ) -> Self {
        Self::new_with_profile_and_client_and_discovery(
            api_key,
            api_base,
            api_base_messages,
            http_client,
            discovery_config,
            OpenCodeProviderProfile::go(),
        )
    }

    /// Create a new OpenCode Zen provider with an explicit model discovery config.
    #[must_use]
    pub(crate) fn new_zen_with_client_and_discovery(
        api_key: String,
        api_base: String,
        api_base_messages: String,
        http_client: HttpClient,
        discovery_config: OpenCodeGoDiscoveryConfig,
    ) -> Self {
        Self::new_with_profile_and_client_and_discovery(
            api_key,
            api_base,
            api_base_messages,
            http_client,
            discovery_config,
            OpenCodeProviderProfile::zen(),
        )
    }

    fn new_with_profile_and_client_and_discovery(
        api_key: String,
        api_base: String,
        api_base_messages: String,
        http_client: HttpClient,
        discovery_config: OpenCodeGoDiscoveryConfig,
        profile: OpenCodeProviderProfile,
    ) -> Self {
        let model_catalog = Arc::new(OpenCodeGoModelCatalog::new(
            http_client.clone(),
            api_key.clone(),
            discovery_config,
        ));
        Arc::clone(&model_catalog).spawn_background_refresh();
        Self {
            http_client,
            api_key,
            api_base,
            api_base_messages,
            profile,
            throttle: OpenCodeGoAdaptiveThrottle::from_env(),
            model_catalog,
        }
    }

    /// Access the OpenCode Go model discovery catalog.
    #[must_use]
    pub fn model_catalog(&self) -> Arc<OpenCodeGoModelCatalog> {
        Arc::clone(&self.model_catalog)
    }

    async fn resolve_model_protocol(&self, model_id: &str) -> ModelProtocol {
        let raw_model_id = normalize_model_id_for_prefix(model_id, self.profile.model_prefix);
        let qualified_model_id = format!("{}/{raw_model_id}", self.profile.model_prefix);
        self.model_catalog
            .models()
            .await
            .into_iter()
            .find(|model| {
                model.model_id == raw_model_id || model.qualified_id == qualified_model_id
            })
            .map_or(ModelProtocol::Unknown, |model| model.protocol)
    }
}

#[derive(Debug)]
struct OpenCodeGoAdaptiveThrottle {
    state: Mutex<OpenCodeGoThrottleState>,
    notify: Notify,
}

#[derive(Debug)]
struct OpenCodeGoThrottleState {
    max_concurrent: usize,
    current_limit: usize,
    in_flight: usize,
    consecutive_failures: usize,
    success_streak: usize,
    cooldown_secs: u64,
    cooldown_until: Option<Instant>,
}

#[derive(Debug, Clone, Copy)]
enum OpenCodeGoAcquireWait {
    Slot,
    Cooldown(Duration),
}

#[derive(Debug)]
struct OpenCodeGoPermit {
    throttle: Arc<OpenCodeGoAdaptiveThrottle>,
}

impl OpenCodeGoAdaptiveThrottle {
    fn from_env() -> Arc<Self> {
        Arc::new(Self::new(get_opencode_go_max_concurrent()))
    }

    fn new(max_concurrent: usize) -> Self {
        let max_concurrent = max_concurrent.max(1);
        Self {
            state: Mutex::new(OpenCodeGoThrottleState {
                max_concurrent,
                current_limit: max_concurrent,
                in_flight: 0,
                consecutive_failures: 0,
                success_streak: 0,
                cooldown_secs: 0,
                cooldown_until: None,
            }),
            notify: Notify::new(),
        }
    }

    async fn acquire(self: &Arc<Self>, model_id: &str) -> OpenCodeGoPermit {
        loop {
            match self.try_acquire() {
                Ok(()) => {
                    debug!(
                        model = normalize_model_id(model_id),
                        "OpenCode Go request admitted by adaptive throttle"
                    );
                    return OpenCodeGoPermit {
                        throttle: Arc::clone(self),
                    };
                }
                Err(OpenCodeGoAcquireWait::Slot) => {
                    self.notify.notified().await;
                }
                Err(OpenCodeGoAcquireWait::Cooldown(wait)) => {
                    debug!(
                        model = normalize_model_id(model_id),
                        wait_ms = wait.as_millis(),
                        "OpenCode Go adaptive throttle is cooling down"
                    );
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        () = self.notify.notified() => {}
                    }
                }
            }
        }
    }

    fn try_acquire(&self) -> Result<(), OpenCodeGoAcquireWait> {
        let mut state = self.lock_state();
        if let Some(until) = state.cooldown_until {
            let now = Instant::now();
            if until > now {
                trace!(
                    current_limit = state.current_limit,
                    max_concurrent = state.max_concurrent,
                    in_flight = state.in_flight,
                    consecutive_failures = state.consecutive_failures,
                    success_streak = state.success_streak,
                    cooldown_secs = state.cooldown_secs,
                    cooldown_remaining_ms = until.duration_since(now).as_millis(),
                    "OpenCode Go adaptive throttle denied request during cooldown"
                );
                return Err(OpenCodeGoAcquireWait::Cooldown(until.duration_since(now)));
            }
            state.cooldown_until = None;
        }

        if state.in_flight < state.current_limit {
            state.in_flight += 1;
            trace!(
                current_limit = state.current_limit,
                max_concurrent = state.max_concurrent,
                in_flight = state.in_flight,
                consecutive_failures = state.consecutive_failures,
                success_streak = state.success_streak,
                cooldown_secs = state.cooldown_secs,
                "OpenCode Go adaptive throttle granted request slot"
            );
            Ok(())
        } else {
            trace!(
                current_limit = state.current_limit,
                max_concurrent = state.max_concurrent,
                in_flight = state.in_flight,
                consecutive_failures = state.consecutive_failures,
                success_streak = state.success_streak,
                cooldown_secs = state.cooldown_secs,
                "OpenCode Go adaptive throttle waiting for request slot"
            );
            Err(OpenCodeGoAcquireWait::Slot)
        }
    }

    fn record_result<T>(&self, result: &Result<T, LlmError>) {
        match result {
            Ok(_) => {
                trace!("OpenCode Go adaptive throttle recorded successful request");
                self.record_success();
            }
            Err(error) if opencode_go_should_throttle(error) => {
                trace!(
                    error = %error,
                    "OpenCode Go adaptive throttle recorded retryable failure"
                );
                self.record_retryable_failure(error)
            }
            Err(error) => {
                trace!(
                    error = %error,
                    "OpenCode Go adaptive throttle ignored non-retryable failure"
                );
            }
        }
    }

    fn record_success(&self) {
        let mut state = self.lock_state();
        state.consecutive_failures = 0;

        if state.current_limit >= state.max_concurrent {
            state.success_streak = 0;
            state.cooldown_secs = 0;
            return;
        }

        state.success_streak += 1;
        if state.success_streak < OPENCODE_GO_SUCCESS_STREAK_TO_INCREASE {
            return;
        }

        state.success_streak = 0;
        state.current_limit += 1;
        if state.current_limit >= state.max_concurrent {
            state.cooldown_secs = 0;
        }

        debug!(
            current_limit = state.current_limit,
            max_concurrent = state.max_concurrent,
            "OpenCode Go adaptive throttle increased concurrency"
        );
        drop(state);
        self.notify.notify_waiters();
    }

    fn record_retryable_failure(&self, error: &LlmError) {
        let mut state = self.lock_state();
        state.success_streak = 0;
        state.consecutive_failures += 1;

        if state.consecutive_failures < OPENCODE_GO_FAILURES_BEFORE_COOLDOWN {
            return;
        }

        state.consecutive_failures = 0;
        state.current_limit = state.current_limit.saturating_sub(1).max(1);
        state.cooldown_secs = if state.cooldown_secs == 0 {
            OPENCODE_GO_COOLDOWN_STEP_SECS
        } else {
            (state.cooldown_secs + OPENCODE_GO_COOLDOWN_STEP_SECS)
                .min(OPENCODE_GO_MAX_COOLDOWN_SECS)
        };
        state.cooldown_until = Some(Instant::now() + Duration::from_secs(state.cooldown_secs));

        warn!(
            error = %error,
            current_limit = state.current_limit,
            max_concurrent = state.max_concurrent,
            cooldown_secs = state.cooldown_secs,
            "OpenCode Go adaptive throttle entered cooldown"
        );
        drop(state);
        self.notify.notify_waiters();
    }

    fn release(&self) {
        let mut state = self.lock_state();
        state.in_flight = state.in_flight.saturating_sub(1);
        trace!(
            current_limit = state.current_limit,
            max_concurrent = state.max_concurrent,
            in_flight = state.in_flight,
            consecutive_failures = state.consecutive_failures,
            success_streak = state.success_streak,
            cooldown_secs = state.cooldown_secs,
            "OpenCode Go adaptive throttle released request slot"
        );
        drop(state);
        self.notify.notify_waiters();
    }

    fn lock_state(&self) -> MutexGuard<'_, OpenCodeGoThrottleState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Drop for OpenCodeGoPermit {
    fn drop(&mut self) {
        self.throttle.release();
    }
}

#[async_trait]
impl LlmProvider for OpenCodeGoProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let protocol = self.resolve_model_protocol(model_id).await;
        let (request_kind, api_base, body, extra_headers): (&str, &str, Value, Vec<(&str, &str)>) =
            match protocol {
                ModelProtocol::OpenAiChatCompletions => (
                    "chat_completion",
                    &self.api_base,
                    build_chat_completion_body(
                        system_prompt,
                        history,
                        user_message,
                        model_id,
                        max_tokens,
                    ),
                    Vec::new(),
                ),
                ModelProtocol::AnthropicMessages => (
                    "messages",
                    &self.api_base_messages,
                    build_anthropic_completion_body(
                        system_prompt,
                        history,
                        user_message,
                        model_id,
                        max_tokens,
                    ),
                    anthropic_extra_headers(&self.api_key),
                ),
                ModelProtocol::Unknown => {
                    return Err(unsupported_protocol_error(model_id, self.profile))
                }
            };
        let _permit = self.throttle.acquire(model_id).await;
        log_request_summary(OpenCodeRequestLog {
            profile: self.profile,
            request_kind,
            api_base,
            model_id,
            max_tokens,
            temperature: OPENCODE_GO_CHAT_TEMPERATURE,
            json_mode: false,
            body: &body,
        });
        let auth = format!("Bearer {}", self.api_key);
        let result = async {
            let response = send_json_request(
                &self.http_client,
                api_base,
                &body,
                Some(&auth),
                &extra_headers,
            )
            .await?;
            let parsed = match protocol {
                ModelProtocol::OpenAiChatCompletions => parse_chat_response(response)?,
                ModelProtocol::AnthropicMessages => parse_anthropic_messages_response(response)?,
                ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
            };
            log_response_summary(self.profile, request_kind, model_id, &parsed);

            parsed.content.ok_or_else(|| {
                LlmError::ApiError(format!(
                    "{} returned no text content for {request_kind}",
                    self.profile.display_name
                ))
            })
        }
        .await;
        self.throttle.record_result(&result);
        result
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(format!(
            "Audio transcription not supported by {}",
            self.profile.display_name
        )))
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        if !discovery::supports_image_input_for_model_id(model_id) {
            return Err(LlmError::ApiError(format!(
                "{} model '{}' is not approved for image input",
                self.profile.display_name,
                normalize_model_id_for_prefix(model_id, self.profile.model_prefix)
            )));
        }

        let protocol = self.resolve_model_protocol(model_id).await;
        let (request_kind, api_base, body, extra_headers): (&str, &str, Value, Vec<(&str, &str)>) =
            match protocol {
                ModelProtocol::OpenAiChatCompletions => (
                    "image_analysis",
                    &self.api_base,
                    build_image_analysis_body(&image_bytes, text_prompt, system_prompt, model_id),
                    Vec::new(),
                ),
                ModelProtocol::AnthropicMessages => {
                    return Err(LlmError::ApiError(format!(
                        "{} image analysis requires OpenAI Chat Completions protocol for model '{}'",
                        self.profile.display_name,
                        normalize_model_id_for_prefix(model_id, self.profile.model_prefix)
                    )));
                }
                ModelProtocol::Unknown => {
                    return Err(unsupported_protocol_error(model_id, self.profile))
                }
            };

        let _permit = self.throttle.acquire(model_id).await;
        log_request_summary(OpenCodeRequestLog {
            profile: self.profile,
            request_kind,
            api_base,
            model_id,
            max_tokens: OPENCODE_GO_IMAGE_ANALYSIS_MAX_TOKENS,
            temperature: OPENCODE_GO_CHAT_TEMPERATURE,
            json_mode: false,
            body: &body,
        });

        let auth = format!("Bearer {}", self.api_key);
        let result = async {
            let response = send_json_request(
                &self.http_client,
                api_base,
                &body,
                Some(&auth),
                &extra_headers,
            )
            .await?;
            let parsed = parse_chat_response(response)?;
            log_response_summary(self.profile, request_kind, model_id, &parsed);

            parsed.content.ok_or_else(|| {
                LlmError::ApiError(format!(
                    "{} returned no text content for image analysis",
                    self.profile.display_name
                ))
            })
        }
        .await;
        self.throttle.record_result(&result);
        result
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
            reasoning_effort,
        } = request;
        let protocol = self.resolve_model_protocol(model_id).await;
        let (request_kind, api_base, body, extra_headers): (&str, &str, Value, Vec<(&str, &str)>) =
            match protocol {
                ModelProtocol::OpenAiChatCompletions => (
                    "chat_with_tools",
                    &self.api_base,
                    build_tool_chat_body(
                        system_prompt,
                        messages,
                        tools,
                        model_id,
                        max_tokens,
                        temperature,
                        json_mode,
                        reasoning_effort,
                    ),
                    Vec::new(),
                ),
                ModelProtocol::AnthropicMessages => (
                    "messages_with_tools",
                    &self.api_base_messages,
                    build_anthropic_messages_body(
                        system_prompt,
                        messages,
                        tools,
                        model_id,
                        max_tokens,
                        temperature,
                        json_mode,
                        reasoning_effort,
                    ),
                    anthropic_extra_headers(&self.api_key),
                ),
                ModelProtocol::Unknown => {
                    return Err(unsupported_protocol_error(model_id, self.profile))
                }
            };
        log_request_summary(OpenCodeRequestLog {
            profile: self.profile,
            request_kind,
            api_base,
            model_id,
            max_tokens,
            temperature: temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
            json_mode,
            body: &body,
        });
        let auth = format!("Bearer {}", self.api_key);
        let _permit = self.throttle.acquire(model_id).await;
        let result = async {
            let response = send_json_request(
                &self.http_client,
                api_base,
                &body,
                Some(&auth),
                &extra_headers,
            )
            .await?;

            let parsed = match protocol {
                ModelProtocol::OpenAiChatCompletions => parse_chat_response(response)?,
                ModelProtocol::AnthropicMessages => parse_anthropic_messages_response(response)?,
                ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
            };
            log_response_summary(self.profile, request_kind, model_id, &parsed);
            Ok(parsed)
        }
        .await;
        self.throttle.record_result(&result);
        result
    }
}

fn opencode_go_should_throttle(error: &LlmError) -> bool {
    match error {
        LlmError::RateLimit { .. } | LlmError::EmptyResponse(_) | LlmError::JsonError(_) => true,
        LlmError::NetworkError(message) => !message.to_ascii_lowercase().contains("builder"),
        LlmError::ApiError(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("429")
                || message.contains("500")
                || message.contains("internal server error")
                || message.contains("502")
                || message.contains("bad gateway")
                || message.contains("503")
                || message.contains("service unavailable")
                || message.contains("504")
                || message.contains("gateway timeout")
                || message.contains("temporarily unavailable")
                || message.contains("timeout")
                || message.contains("overloaded")
        }
        _ => false,
    }
}

fn normalize_model_id(model_id: &str) -> &str {
    let without_go = normalize_model_id_for_prefix(model_id, OPENCODE_GO_PROVIDER_ID);
    normalize_model_id_for_prefix(without_go, OPENCODE_ZEN_PROVIDER_ID)
}

fn normalize_model_id_for_prefix<'a>(model_id: &'a str, model_prefix: &str) -> &'a str {
    let trimmed = model_id.trim();
    let prefix = format!("{}/", model_prefix.trim().trim_end_matches('/'));
    trimmed.strip_prefix(&prefix).unwrap_or(trimmed)
}

/// Check if the model supports reasoning/thinking effort parameters.
///
/// Matches DeepSeek V4 family and MiMo V2 family.
/// Model ID is normalized (prefix stripped) before matching.
fn is_reasoning_model(model_id: &str) -> bool {
    let lower = normalize_model_id(model_id).to_ascii_lowercase();
    lower.starts_with("deepseek-v4") || lower.starts_with("mimo-v2")
}

fn disables_reasoning(reasoning_effort: Option<&str>) -> bool {
    reasoning_effort
        .map(str::trim)
        .map(|effort| {
            effort.eq_ignore_ascii_case("none") || effort.eq_ignore_ascii_case("disabled")
        })
        .unwrap_or(false)
}

fn derive_messages_api_base(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    if trimmed.ends_with("/messages") {
        return trimmed.to_string();
    }
    if let Some(prefix) = trimmed.strip_suffix("/chat/completions") {
        return format!("{prefix}/messages");
    }
    if trimmed.ends_with("/v1") {
        return format!("{trimmed}/messages");
    }
    format!("{trimmed}/messages")
}

fn unsupported_protocol_error(model_id: &str, profile: OpenCodeProviderProfile) -> LlmError {
    LlmError::ApiError(format!(
        "{} model '{}' has unknown wire protocol; configure modules.{}.protocol_overrides for this model",
        profile.display_name,
        normalize_model_id_for_prefix(model_id, profile.model_prefix),
        profile.module_id
    ))
}

struct OpenCodeRequestLog<'a> {
    profile: OpenCodeProviderProfile,
    request_kind: &'a str,
    api_base: &'a str,
    model_id: &'a str,
    max_tokens: u32,
    temperature: f32,
    json_mode: bool,
    body: &'a Value,
}

fn log_request_summary(event: OpenCodeRequestLog<'_>) {
    let (endpoint_host, endpoint_path) = endpoint_parts(event.api_base);
    let message_count = event
        .body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let tool_count = event
        .body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    debug!(
        provider = event.profile.provider_id,
        request_kind = event.request_kind,
        model = normalize_model_id_for_prefix(event.model_id, event.profile.model_prefix),
        endpoint_host = endpoint_host.as_str(),
        endpoint_path = endpoint_path.as_str(),
        json_mode = event.json_mode,
        has_tools = tool_count > 0,
        tool_count,
        message_count,
        max_tokens = event.max_tokens,
        temperature = event.temperature,
        request_body_bytes = json_body_len(event.body),
        "OpenCode request summary"
    );
}

fn log_response_summary(
    profile: OpenCodeProviderProfile,
    request_kind: &str,
    model_id: &str,
    response: &ChatResponse,
) {
    let usage = response.usage.as_ref();
    debug!(
        provider = profile.provider_id,
        request_kind,
        model = normalize_model_id_for_prefix(model_id, profile.model_prefix),
        finish_reason = response.finish_reason.as_str(),
        content_len = response.content.as_ref().map_or(0, String::len),
        reasoning_len = response.reasoning_content.as_ref().map_or(0, String::len),
        tool_call_count = response.tool_calls.len(),
        usage_prompt_tokens = usage.map(|usage| usage.prompt_tokens),
        usage_completion_tokens = usage.map(|usage| usage.completion_tokens),
        usage_total_tokens = usage.map(|usage| usage.total_tokens),
        usage_cached_tokens = usage.and_then(|u| u.cached_tokens),
        usage_cache_creation_tokens = usage.and_then(|u| u.cache_creation_tokens),
        "OpenCode response summary"
    );
}

fn endpoint_parts(url: &str) -> (String, String) {
    match Url::parse(url) {
        Ok(parsed) => (
            parsed.host_str().unwrap_or("unknown").to_string(),
            parsed.path().to_string(),
        ),
        Err(_) => ("invalid-url".to_string(), "invalid-url".to_string()),
    }
}

fn json_body_len(body: &Value) -> usize {
    serde_json::to_vec(body).map_or(0, |bytes| bytes.len())
}

fn build_chat_completion_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let mut messages = prepare_structured_messages(
        system_prompt,
        history,
        discovery::supports_image_input_for_model_id(model_id),
    );
    messages.push(json!({
        "role": "user",
        "content": user_message,
    }));

    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": OPENCODE_GO_CHAT_TEMPERATURE,
        "stream": false,
    });

    if is_reasoning_model(model_id) {
        body["reasoning_effort"] = json!(OPENCODE_GO_REASONING_EFFORT);
    }

    body
}

fn build_image_analysis_body(
    image_bytes: &[u8],
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
) -> Value {
    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": [
            {"role": "system", "content": system_prompt},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text_prompt},
                    {
                        "type": "image_url",
                        "image_url": {"url": image_data_url(image_bytes)}
                    }
                ]
            }
        ],
        "max_tokens": OPENCODE_GO_IMAGE_ANALYSIS_MAX_TOKENS,
        "temperature": OPENCODE_GO_CHAT_TEMPERATURE,
        "stream": false,
    });

    if is_reasoning_model(model_id) {
        body["reasoning_effort"] = json!(OPENCODE_GO_REASONING_EFFORT);
    }

    body
}

fn image_data_url(image_bytes: &[u8]) -> String {
    image_data_url_with_mime(image_bytes, infer_image_mime_type(image_bytes))
}

fn image_data_url_with_mime(image_bytes: &[u8], mime_type: &str) -> String {
    let mime_type = normalized_image_mime_type(mime_type, image_bytes);
    format!("data:{mime_type};base64,{}", BASE64.encode(image_bytes))
}

fn normalized_image_mime_type(mime_type: &str, image_bytes: &[u8]) -> String {
    let trimmed = mime_type.trim();
    if trimmed.starts_with("image/") {
        trimmed.to_string()
    } else {
        infer_image_mime_type(image_bytes).to_string()
    }
}

fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
    if image_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        return "image/png";
    }
    if image_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if image_bytes.starts_with(b"RIFF") && image_bytes.get(8..12) == Some(b"WEBP") {
        return "image/webp";
    }
    "image/jpeg"
}

fn build_anthropic_completion_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let mut messages = prepare_anthropic_messages(history);
    messages.push(anthropic_text_message("user", user_message));
    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": OPENCODE_GO_CHAT_TEMPERATURE,
        "stream": false,
    });
    if let Some(system) = anthropic_system_prompt(system_prompt, history) {
        body["system"] = json!(system);
    }
    if is_reasoning_model(model_id) {
        body["thinking"] = json!({ "type": "enabled" });
    }
    body
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    let messages = prepare_structured_messages(
        system_prompt,
        history,
        discovery::supports_image_input_for_model_id(model_id),
    );
    let openai_tools = prepare_tools_json(tools);
    let has_tools = !openai_tools.is_empty();

    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
        "stream": false,
    });

    if has_tools {
        body["tools"] = json!(openai_tools);
        body["tool_choice"] = json!("auto");
        body["parallel_tool_calls"] = json!(true);
    }

    if should_use_native_json_mode(json_mode, has_tools) {
        body["response_format"] = json!({ "type": "json_object" });
    }

    if is_reasoning_model(model_id) && !disables_reasoning(reasoning_effort) {
        body["reasoning_effort"] = json!(reasoning_effort.unwrap_or(OPENCODE_GO_REASONING_EFFORT));
    }

    body
}

fn build_anthropic_messages_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    _json_mode: bool,
    reasoning_effort: Option<&str>,
) -> Value {
    let messages = prepare_anthropic_messages(history);
    let anthropic_tools = prepare_anthropic_tools_json(tools);
    let has_tools = !anthropic_tools.is_empty();

    let mut body = json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
        "stream": false,
    });

    if let Some(system) = anthropic_system_prompt(system_prompt, history) {
        body["system"] = json!(system);
    }
    if has_tools {
        body["tools"] = json!(anthropic_tools);
        body["tool_choice"] = json!({ "type": "auto" });
    }
    if is_reasoning_model(model_id) && !disables_reasoning(reasoning_effort) {
        body["thinking"] = json!({ "type": "enabled" });
    }

    body
}

fn prepare_structured_messages(
    system_prompt: &str,
    history: &[Message],
    allow_native_image_parts: bool,
) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len() + 1);

    if !system_prompt.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                if !msg.content.trim().is_empty() {
                    messages.push(json!({
                        "role": "system",
                        "content": msg.content,
                    }));
                }
            }
            "assistant" => {
                let mut message = json!({
                    "role": "assistant",
                    "content": msg.content,
                });
                if let Some(reasoning_content) = msg
                    .reasoning_content
                    .as_deref()
                    .filter(|reasoning| !reasoning.trim().is_empty())
                {
                    message["reasoning_content"] = json!(reasoning_content);
                }

                if let Some(tool_calls) = &msg.tool_calls {
                    let encoded_tool_calls: Vec<Value> = tool_calls
                        .iter()
                        .filter_map(|tool_call| {
                            CHAT_LIKE_TOOL_PROFILE
                                .encode_tool_call(tool_call)
                                .and_then(|call| call.into_chat_like())
                                .map(|call| {
                                    json!({
                                        "id": call.id,
                                        "type": "function",
                                        "function": {
                                            "name": call.name,
                                            "arguments": call.arguments,
                                        },
                                    })
                                })
                        })
                        .collect();

                    if !encoded_tool_calls.is_empty() {
                        message["tool_calls"] = json!(encoded_tool_calls);
                    }
                }

                messages.push(message);
            }
            "tool" => {
                if let Some(result) = CHAT_LIKE_TOOL_PROFILE
                    .encode_tool_result(msg)
                    .and_then(|result| result.into_chat_like())
                {
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": result.tool_call_id,
                        "content": result.content,
                    }));
                }
            }
            _ => {
                messages.push(json!({
                    "role": "user",
                    "content": openai_user_message_content(msg, allow_native_image_parts),
                }));
            }
        }
    }

    messages
}

fn openai_user_message_content(message: &Message, allow_native_image_parts: bool) -> Value {
    if !allow_native_image_parts || message.content_parts.is_empty() {
        return json!(message.content);
    }

    let mut parts = Vec::new();
    if !message.content.is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }

    for part in &message.content_parts {
        match part {
            MessageContentPart::Image { mime_type, bytes } if !bytes.is_empty() => {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": image_data_url_with_mime(bytes, mime_type),
                    },
                }));
            }
            MessageContentPart::Image { .. } => {}
        }
    }

    if parts.is_empty() {
        json!(message.content)
    } else {
        json!(parts)
    }
}

fn anthropic_system_prompt(system_prompt: &str, history: &[Message]) -> Option<String> {
    let mut parts = Vec::new();
    if !system_prompt.trim().is_empty() {
        parts.push(system_prompt.trim().to_string());
    }
    parts.extend(
        history
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.trim())
            .filter(|content| !content.is_empty())
            .map(ToString::to_string),
    );
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn prepare_anthropic_messages(history: &[Message]) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len());
    let mut index = 0;
    while index < history.len() {
        let message = &history[index];
        if message.role == "system" {
            index += 1;
            continue;
        }
        if message.role == "tool" {
            let mut blocks = Vec::new();
            let mut cursor = index;
            while cursor < history.len() && history[cursor].role == "tool" {
                if let Some(block) = anthropic_tool_result_block(&history[cursor]) {
                    blocks.push(block);
                }
                cursor += 1;
            }
            if !blocks.is_empty() {
                messages.push(json!({
                    "role": "user",
                    "content": blocks,
                }));
            }
            index = cursor;
            continue;
        }

        messages.push(match message.role.as_str() {
            "assistant" => anthropic_assistant_message(message),
            "user" => anthropic_text_message("user", &message.content),
            _ => anthropic_text_message("user", &message.content),
        });
        index += 1;
    }
    messages
}

fn anthropic_text_message(role: &str, text: &str) -> Value {
    json!({
        "role": role,
        "content": [{
            "type": "text",
            "text": text,
        }],
    })
}

fn anthropic_assistant_message(message: &Message) -> Value {
    let mut blocks = Vec::new();
    if !message.content.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }
    if let Some(tool_calls) = &message.tool_calls {
        blocks.extend(tool_calls.iter().filter_map(|tool_call| {
            ANTHROPIC_CLIENT_TOOL_PROFILE
                .encode_tool_call(tool_call)
                .and_then(|call| call.into_anthropic())
                .map(|call| {
                    json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.input,
                    })
                })
        }));
    }
    if blocks.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": "",
        }));
    }
    json!({
        "role": "assistant",
        "content": blocks,
    })
}

fn anthropic_tool_result_block(message: &Message) -> Option<Value> {
    ANTHROPIC_CLIENT_TOOL_PROFILE
        .encode_tool_result(message)
        .and_then(|result| result.into_anthropic())
        .map(|result| {
            let mut block = json!({
                "type": "tool_result",
                "tool_use_id": result.tool_use_id,
                "content": result.content,
            });
            if let Some(is_error) = result.is_error {
                block["is_error"] = json!(is_error);
            }
            block
        })
}

fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                },
            })
        })
        .collect()
}

fn prepare_anthropic_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters,
            })
        })
        .collect()
}

fn anthropic_extra_headers(api_key: &str) -> Vec<(&str, &str)> {
    vec![
        ("anthropic-version", ANTHROPIC_VERSION_HEADER),
        ("x-api-key", api_key),
    ]
}

fn should_use_native_json_mode(json_mode: bool, has_tools: bool) -> bool {
    json_mode && !has_tools
}

fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
    if let Some(error) = extract_opencode_error_response(&response) {
        return Err(LlmError::ApiError(error));
    }

    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::ApiError(format!(
                "Missing choices[0] in OpenCode Go response{}",
                response_shape_suffix(&response)
            ))
        })?;
    let message = choice
        .get("message")
        .ok_or_else(|| LlmError::ApiError("Missing message in OpenCode Go response".to_string()))?;

    let content = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string);
    let reasoning_content = parse_reasoning_content(message);
    let tool_calls = match message.get("tool_calls") {
        Some(value) if value.is_null() => Vec::new(),
        Some(value) if value.is_array() => parse_tool_calls(value)?,
        Some(_) => {
            return Err(LlmError::JsonError(
                "Invalid tool_calls format from OpenCode Go".to_string(),
            ))
        }
        None => Vec::new(),
    };

    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::ApiError("Empty OpenCode Go response".to_string()));
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let usage = response.get("usage").and_then(parse_usage);

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content,
        usage,
    })
}

fn parse_anthropic_messages_response(response: Value) -> Result<ChatResponse, LlmError> {
    if let Some(error) = extract_opencode_error_response(&response) {
        return Err(LlmError::ApiError(error));
    }

    let blocks = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            LlmError::ApiError(
                "Missing content blocks in OpenCode Go messages response".to_string(),
            )
        })?;
    let mut content_parts = Vec::new();
    let mut reasoning_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for (index, block) in blocks.iter().enumerate() {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    content_parts.push(text.to_string());
                }
            }
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let input = block.get("input").unwrap_or(&Value::Null);
                let arguments = if input.is_null() {
                    "{}".to_string()
                } else {
                    serde_json::to_string(input).unwrap_or_default()
                };
                let wire_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("opencode_go_tool_use_{index}"));
                tool_calls.push(ANTHROPIC_CLIENT_TOOL_PROFILE.inbound_provider_tool_call(
                    wire_id.as_str(),
                    None,
                    name.to_string(),
                    arguments,
                ));
            }
            Some("thinking") => {
                if let Some(thinking) = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|thinking| !thinking.is_empty())
                {
                    reasoning_parts.push(thinking.to_string());
                }
            }
            Some("redacted_thinking") => {
                if let Some(data) = block
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|data| !data.is_empty())
                {
                    reasoning_parts.push(data.to_string());
                }
            }
            _ => {}
        }
    }

    let content = (!content_parts.is_empty()).then(|| content_parts.join("\n"));
    let reasoning_content = (!reasoning_parts.is_empty()).then(|| reasoning_parts.join("\n"));
    if content.is_none() && reasoning_content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::ApiError(
            "Empty OpenCode Go messages response".to_string(),
        ));
    }

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason: response
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(map_anthropic_stop_reason)
            .unwrap_or_else(|| "unknown".to_string()),
        reasoning_content,
        usage: response.get("usage").and_then(parse_anthropic_usage),
    })
}

fn extract_opencode_error_response(response: &Value) -> Option<String> {
    if let Some(error) = response.get("error") {
        if let Some(message) = non_empty_str(error.get("message")) {
            return Some(format_opencode_error_message(error, message));
        }
        if let Some(message) = non_empty_str(Some(error)) {
            return Some(format!("OpenCode Go returned error response: {message}"));
        }
    }

    non_empty_str(response.get("message"))
        .or_else(|| non_empty_str(response.get("detail")))
        .map(|message| format!("OpenCode Go returned error response: {message}"))
}

fn format_opencode_error_message(error: &Value, message: &str) -> String {
    let label = non_empty_str(error.get("code")).or_else(|| non_empty_str(error.get("type")));
    match label {
        Some(label) => format!("OpenCode Go returned error response ({label}): {message}"),
        None => format!("OpenCode Go returned error response: {message}"),
    }
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn response_shape_suffix(response: &Value) -> String {
    let Some(object) = response.as_object() else {
        return format!("; response_type={}", value_type_name(response));
    };
    if object.is_empty() {
        return "; top_level_keys=[]".to_string();
    }
    let keys = object
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    format!("; top_level_keys=[{keys}]")
}

fn value_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn map_anthropic_stop_reason(stop_reason: &str) -> String {
    match stop_reason {
        "end_turn" => "stop".to_string(),
        "tool_use" => "tool_calls".to_string(),
        "stop_sequence" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        other => other.to_string(),
    }
}

fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from OpenCode Go".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for call in array {
        let Some(function) = call.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        let arguments = function
            .get("arguments")
            .map(normalize_tool_arguments)
            .unwrap_or_else(|| "{}".to_string());
        let wire_id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());

        tool_calls.push(match wire_id {
            Some(wire_id) => CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
                wire_id,
                None,
                name.to_string(),
                arguments,
            ),
            None => {
                CHAT_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name.to_string(), arguments)
            }
        });
    }

    Ok(tool_calls)
}

fn normalize_tool_arguments(value: &Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .or_else(|| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn parse_reasoning_content(message: &Value) -> Option<String> {
    message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .or_else(|| message.get("reasoning").and_then(Value::as_str))
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string)
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: value.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
        cached_tokens: value
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_creation_tokens: None,
    })
}

fn parse_anthropic_usage(value: &Value) -> Option<TokenUsage> {
    let prompt_tokens = value.get("input_tokens")?.as_u64()? as u32;
    let completion_tokens = value.get("output_tokens")?.as_u64()? as u32;
    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens.saturating_add(completion_tokens),
        cached_tokens: value
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        cache_creation_tokens: value
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_anthropic_completion_body, build_anthropic_messages_body, build_chat_completion_body,
        build_tool_chat_body, derive_messages_api_base, is_reasoning_model, normalize_model_id,
        opencode_go_should_throttle, parse_anthropic_messages_response, parse_anthropic_usage,
        parse_chat_response, parse_tool_calls, parse_usage, prepare_anthropic_messages,
        prepare_structured_messages, prepare_tools_json, unsupported_protocol_error,
        OpenCodeGoAdaptiveThrottle, OpenCodeProviderProfile,
    };
    use crate::llm::{
        LlmError, Message, MessageContentPart, ToolCall, ToolCallCorrelation, ToolCallFunction,
        ToolDefinition,
    };
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    fn read_file_tool() -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }
    }

    #[test]
    fn normalizes_opencode_go_prefixed_model_id() {
        assert_eq!(
            normalize_model_id("opencode-go/deepseek-v4-flash"),
            "deepseek-v4-flash"
        );
        assert_eq!(
            normalize_model_id("opencode-zen/deepseek-v4-flash-free"),
            "deepseek-v4-flash-free"
        );
        assert_eq!(
            normalize_model_id(" deepseek-v4-flash "),
            "deepseek-v4-flash"
        );
    }

    #[test]
    fn chat_completion_body_uses_raw_model_id() {
        let body = build_chat_completion_body(
            "system",
            &[Message::user("history")],
            "hello",
            "opencode-go/deepseek-v4-flash",
            32000,
        );

        assert_eq!(body["model"], json!("deepseek-v4-flash"));
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][2]["content"], json!("hello"));
    }

    #[test]
    fn reasoning_model_detection() {
        // DeepSeek V4 family
        assert!(is_reasoning_model("deepseek-v4-flash"));
        assert!(is_reasoning_model("deepseek-v4-pro"));
        assert!(is_reasoning_model("opencode-go/deepseek-v4-flash"));
        assert!(is_reasoning_model(" DEEPSEEK-V4-FLASH "));

        // MiMo V2 family
        assert!(is_reasoning_model("mimo-v2.5"));
        assert!(is_reasoning_model("mimo-v2.5-pro"));
        assert!(is_reasoning_model("opencode-go/mimo-v2.5-pro"));

        // Non-reasoning models
        assert!(!is_reasoning_model("deepseek-v3"));
        assert!(!is_reasoning_model("deepseek-chat"));
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("qwen3-235b-a22b"));
    }

    #[test]
    fn reasoning_effort_in_openai_text_body() {
        let body = build_chat_completion_body("system", &[], "hello", "deepseek-v4-flash", 32000);
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn reasoning_effort_in_openai_tool_body() {
        let tools = vec![read_file_tool()];
        let body =
            build_tool_chat_body("system", &[], &tools, "mimo-v2.5", 32000, None, false, None);
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn disabled_reasoning_omits_openai_reasoning_effort() {
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "deepseek-v4-flash",
            32000,
            None,
            false,
            Some("none"),
        );
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn thinking_enabled_in_anthropic_text_body() {
        let body =
            build_anthropic_completion_body("system", &[], "hello", "deepseek-v4-flash", 32000);
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
    }

    #[test]
    fn thinking_enabled_in_anthropic_tool_body() {
        let tools = vec![read_file_tool()];
        let body = build_anthropic_messages_body(
            "system",
            &[],
            &tools,
            "mimo-v2.5-pro",
            32000,
            None,
            false,
            None,
        );
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
    }

    #[test]
    fn disabled_reasoning_omits_anthropic_thinking() {
        let body = build_anthropic_messages_body(
            "system",
            &[],
            &[],
            "mimo-v2.5-pro",
            32000,
            None,
            false,
            Some("disabled"),
        );
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn no_reasoning_params_for_non_reasoning_models() {
        let body = build_chat_completion_body("system", &[], "hello", "gpt-4o", 32000);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn tool_request_body_includes_function_names() {
        let tools = vec![read_file_tool()];
        let body = build_tool_chat_body(
            "system",
            &[],
            &tools,
            "deepseek-v4-flash",
            32000,
            Some(0.2),
            false,
            None,
        );

        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert_eq!(body["reasoning_effort"], json!("high"));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn json_mode_without_tools_sets_response_format() {
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "deepseek-v4-flash",
            32000,
            None,
            true,
            None,
        );

        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn json_mode_with_tools_does_not_set_response_format() {
        let tools = vec![read_file_tool()];
        let body = build_tool_chat_body(
            "system",
            &[],
            &tools,
            "deepseek-v4-flash",
            32000,
            None,
            true,
            None,
        );

        assert!(body.get("response_format").is_none());
        assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
    }

    #[test]
    fn structured_history_preserves_wire_tool_ids() {
        let history = vec![
            Message::assistant_with_tools_and_reasoning(
                "Calling tools",
                Some("provider thinking trace".to_string()),
                vec![ToolCall::new(
                    "invoke-opencode-1",
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-opencode-1")
                        .with_provider_tool_call_id("call-opencode-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-opencode-1",
                ToolCallCorrelation::new("invoke-opencode-1")
                    .with_provider_tool_call_id("call-opencode-1"),
                "read_file",
                "contents",
            ),
        ];

        let messages = prepare_structured_messages("system", &history, false);

        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("call-opencode-1"));
        assert_eq!(
            messages[1]["reasoning_content"],
            json!("provider thinking trace")
        );
        assert_eq!(messages[2]["tool_call_id"], json!("call-opencode-1"));
    }

    #[test]
    fn tool_chat_body_serializes_user_image_parts_for_image_models_only() {
        let user = Message::user("What is written here?").with_user_content_parts(vec![
            MessageContentPart::image("image/png", b"png".to_vec()),
        ]);
        let tool_call = ToolCall::new(
            "invoke-opencode-1",
            ToolCallFunction {
                name: "read_file".to_string(),
                arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new("invoke-opencode-1")
                .with_provider_tool_call_id("call-opencode-1"),
        );
        let mut tool_result = Message::tool_with_correlation(
            "invoke-opencode-1",
            ToolCallCorrelation::new("invoke-opencode-1")
                .with_provider_tool_call_id("call-opencode-1"),
            "read_file",
            "contents",
        );
        tool_result
            .content_parts
            .push(MessageContentPart::image("image/png", b"ignored".to_vec()));
        let history = vec![
            user,
            Message::assistant_with_tools("Calling tools", vec![tool_call]),
            tool_result,
        ];

        let vision_body = build_tool_chat_body(
            "system",
            &history,
            &[read_file_tool()],
            "mimo-v2.5",
            32000,
            None,
            false,
            None,
        );
        let user_content = vision_body["messages"][1]["content"]
            .as_array()
            .expect("vision user content should be an array");
        assert_eq!(user_content[0]["type"], json!("text"));
        assert_eq!(user_content[0]["text"], json!("What is written here?"));
        assert_eq!(user_content[1]["type"], json!("image_url"));
        assert_eq!(
            user_content[1]["image_url"]["url"],
            json!("data:image/png;base64,cG5n")
        );
        assert_eq!(
            vision_body["messages"][2]["content"],
            json!("Calling tools")
        );
        assert_eq!(vision_body["messages"][3]["content"], json!("contents"));
        assert!(vision_body["messages"][3]["content"].is_string());

        let text_only_body = build_tool_chat_body(
            "system",
            &history,
            &[read_file_tool()],
            "mimo-v2.5-pro",
            32000,
            None,
            false,
            None,
        );
        assert_eq!(
            text_only_body["messages"][1]["content"],
            json!("What is written here?")
        );
    }

    #[test]
    fn anthropic_messages_body_uses_messages_endpoint_shape() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![ToolCall::new(
                    "invoke-opencode-1",
                    ToolCallFunction {
                        name: "read_file".to_string(),
                        arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-opencode-1")
                        .with_provider_tool_call_id("toolu-opencode-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-opencode-1",
                ToolCallCorrelation::new("invoke-opencode-1")
                    .with_provider_tool_call_id("toolu-opencode-1"),
                "read_file",
                "contents",
            ),
        ];
        let body = build_anthropic_messages_body(
            "system",
            &history,
            &[read_file_tool()],
            "opencode-go/minimax-m2.7",
            32000,
            Some(0.2),
            true,
            None,
        );

        assert_eq!(body["model"], json!("minimax-m2.7"));
        assert_eq!(body["system"], json!("system"));
        assert_eq!(body["messages"][0]["role"], json!("assistant"));
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("tool_use"));
        assert_eq!(
            body["messages"][0]["content"][1]["id"],
            json!("toolu-opencode-1")
        );
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("tool_result")
        );
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        assert_eq!(
            body["tools"][0]["input_schema"]["properties"]["path"]["type"],
            json!("string")
        );
        assert_eq!(body["tool_choice"], json!({ "type": "auto" }));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn anthropic_message_conversion_groups_tool_results() {
        let history = vec![
            Message::tool_with_correlation(
                "invoke-a",
                ToolCallCorrelation::new("invoke-a").with_provider_tool_call_id("toolu-a"),
                "read_file",
                "a",
            ),
            Message::tool_with_correlation(
                "invoke-b",
                ToolCallCorrelation::new("invoke-b").with_provider_tool_call_id("toolu-b"),
                "read_file",
                "b",
            ),
        ];

        let messages = prepare_anthropic_messages(&history);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], json!("user"));
        assert_eq!(messages[0]["content"].as_array().expect("blocks").len(), 2);
        assert_eq!(messages[0]["content"][0]["tool_use_id"], json!("toolu-a"));
        assert_eq!(messages[0]["content"][1]["tool_use_id"], json!("toolu-b"));
    }

    #[test]
    fn derives_messages_endpoint_from_chat_completions_endpoint() {
        assert_eq!(
            derive_messages_api_base("https://opencode.ai/zen/go/v1/chat/completions"),
            "https://opencode.ai/zen/go/v1/messages"
        );
        assert_eq!(
            derive_messages_api_base("https://opencode.ai/zen/go/v1"),
            "https://opencode.ai/zen/go/v1/messages"
        );
    }

    #[test]
    fn parse_tool_calls_preserves_provider_wire_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-opencode-2",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": "{\"path\":\"Cargo.toml\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_ne!(tool_calls[0].invocation_id().as_str(), "call-opencode-2");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call-opencode-2");
    }

    #[test]
    fn parse_tool_calls_accepts_object_arguments() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-opencode-3",
                "type": "function",
                "function": {
                    "name": "read_file",
                    "arguments": { "path": "Cargo.toml" }
                }
            }
        ]))
        .expect("tool calls parse");

        assert_eq!(tool_calls[0].function.arguments, r#"{"path":"Cargo.toml"}"#);
    }

    #[test]
    fn parse_chat_response_extracts_content_reasoning_tool_calls_and_usage() {
        let response = parse_chat_response(json!({
            "choices": [{
                "message": {
                    "content": null,
                    "reasoning_content": "internal reasoning",
                    "tool_calls": [{
                        "id": "call-opencode-4",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }))
        .expect("response parses");

        assert_eq!(response.content, None);
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("internal reasoning")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.usage.expect("usage").total_tokens, 15);
    }

    #[test]
    fn mimo_v25_error_envelope_is_not_masked_as_missing_choices() {
        let body = build_tool_chat_body(
            "system",
            &[Message::user("Use tools if needed")],
            &[read_file_tool()],
            "opencode-go/mimo-v2.5",
            32000,
            None,
            false,
            None,
        );

        assert_eq!(body["model"], json!("mimo-v2.5"));
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["reasoning_effort"], json!("high"));

        let error = parse_chat_response(json!({
            "error": {
                "message": "reasoning_effort is not supported for this model",
                "type": "invalid_request_error"
            }
        }))
        .expect_err("provider error envelope should not parse as a successful chat response");

        assert!(
            error
                .to_string()
                .contains("reasoning_effort is not supported for this model"),
            "unexpected error: {error}"
        );
        assert!(
            !error.to_string().contains("Missing choices[0]"),
            "provider error envelope was masked as a response-shape error: {error}"
        );
    }

    #[test]
    fn parse_usage_extracts_cached_tokens_from_prompt_tokens_details() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 3840,
            "completion_tokens": 512,
            "total_tokens": 4352,
            "prompt_tokens_details": {
                "cached_tokens": 2560
            }
        }))
        .expect("usage should parse");

        assert_eq!(usage.prompt_tokens, 3840);
        assert_eq!(usage.cached_tokens, Some(2560));
        assert_eq!(usage.cache_creation_tokens, None);
    }

    #[test]
    fn parse_usage_returns_none_cached_when_no_details() {
        let usage = parse_usage(&json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150
        }))
        .expect("usage should parse");

        assert_eq!(usage.cached_tokens, None);
    }

    #[test]
    fn parse_anthropic_usage_extracts_cache_fields() {
        let usage = parse_anthropic_usage(&json!({
            "input_tokens": 3840,
            "output_tokens": 512,
            "cache_read_input_tokens": 2560,
            "cache_creation_input_tokens": 128
        }))
        .expect("anthropic usage should parse");

        assert_eq!(usage.prompt_tokens, 3840);
        assert_eq!(usage.total_tokens, 4352);
        assert_eq!(usage.cached_tokens, Some(2560));
        assert_eq!(usage.cache_creation_tokens, Some(128));
    }

    #[test]
    fn parse_anthropic_usage_returns_none_when_no_cache_fields() {
        let usage = parse_anthropic_usage(&json!({
            "input_tokens": 10,
            "output_tokens": 5
        }))
        .expect("anthropic usage should parse");

        assert_eq!(usage.cached_tokens, None);
        assert_eq!(usage.cache_creation_tokens, None);
    }

    #[test]
    fn parse_chat_response_extracts_text_tool_calls_reasoning_and_usage() {
        let response = parse_anthropic_messages_response(json!({
            "content": [
                { "type": "thinking", "thinking": "internal reasoning" },
                { "type": "text", "text": "Use a tool" },
                {
                    "type": "tool_use",
                    "id": "toolu-opencode-2",
                    "name": "read_file",
                    "input": { "path": "Cargo.toml" }
                }
            ],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5
            }
        }))
        .expect("anthropic response parses");

        assert_eq!(response.content.as_deref(), Some("Use a tool"));
        assert_eq!(
            response.reasoning_content.as_deref(),
            Some("internal reasoning")
        );
        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].wire_tool_call_id(),
            "toolu-opencode-2"
        );
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"path":"Cargo.toml"}"#
        );
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.usage.expect("usage").total_tokens, 15);
    }

    #[test]
    fn unknown_protocol_error_mentions_override_path() {
        let error =
            unsupported_protocol_error("opencode-go/hy3-preview", OpenCodeProviderProfile::go());

        assert!(error.to_string().contains("protocol_overrides"));
        assert!(error.to_string().contains("hy3-preview"));
    }

    #[test]
    fn unknown_protocol_error_uses_zen_override_path() {
        let error =
            unsupported_protocol_error("opencode-zen/custom-free", OpenCodeProviderProfile::zen());

        assert!(error.to_string().contains("OpenCode Zen"));
        assert!(error
            .to_string()
            .contains("modules.llm-provider/opencode-zen.protocol_overrides"));
        assert!(error.to_string().contains("custom-free"));
    }

    #[test]
    fn prepare_tools_json_uses_nested_function_schema() {
        let tools = prepare_tools_json(&[read_file_tool()]);

        assert_eq!(tools[0]["function"]["name"], json!("read_file"));
        assert!(tools[0].get("name").is_none());
    }

    #[tokio::test]
    async fn adaptive_throttle_blocks_until_capacity_is_released() {
        let throttle = Arc::new(OpenCodeGoAdaptiveThrottle::new(1));
        let permit = throttle.acquire("deepseek-v4-flash").await;

        let blocked = tokio::time::timeout(
            Duration::from_millis(30),
            throttle.acquire("deepseek-v4-flash"),
        )
        .await;
        assert!(blocked.is_err());

        drop(permit);

        let second = tokio::time::timeout(
            Duration::from_millis(200),
            throttle.acquire("deepseek-v4-flash"),
        )
        .await;
        assert!(second.is_ok());
    }

    #[test]
    fn adaptive_throttle_enters_incremental_cooldown_after_failure_bursts() {
        let throttle = OpenCodeGoAdaptiveThrottle::new(5);
        let rate_limit_error = || LlmError::RateLimit {
            wait_secs: None,
            message: "too many requests".to_string(),
        };

        for _ in 0..3 {
            throttle.record_result::<()>(&Err(rate_limit_error()));
        }
        {
            let state = throttle.lock_state();
            assert_eq!(state.current_limit, 4);
            assert_eq!(state.cooldown_secs, 5);
            assert!(state.cooldown_until.is_some());
        }

        for _ in 0..3 {
            throttle.record_result::<()>(&Err(rate_limit_error()));
        }
        let state = throttle.lock_state();
        assert_eq!(state.current_limit, 3);
        assert_eq!(state.cooldown_secs, 10);
    }

    #[test]
    fn adaptive_throttle_recovers_concurrency_after_success_streak() {
        let throttle = OpenCodeGoAdaptiveThrottle::new(3);
        for _ in 0..3 {
            throttle.record_result::<()>(&Err(LlmError::ApiError(
                "500 Internal Server Error".to_string(),
            )));
        }

        for _ in 0..3 {
            throttle.record_result::<()>(&Ok(()));
        }

        let state = throttle.lock_state();
        assert_eq!(state.current_limit, 3);
        assert_eq!(state.cooldown_secs, 0);
    }

    #[test]
    fn opencode_go_throttle_classifies_retryable_provider_errors_only() {
        assert!(opencode_go_should_throttle(&LlmError::ApiError(
            "500 Internal Server Error".to_string()
        )));
        assert!(opencode_go_should_throttle(&LlmError::NetworkError(
            "connection reset".to_string()
        )));
        assert!(!opencode_go_should_throttle(&LlmError::ApiError(
            "invalid API key".to_string()
        )));
    }
}
