use super::messages;
use crate::config::{OPENCODE_GO_CHAT_TEMPERATURE, get_opencode_go_max_concurrent};
#[cfg(test)]
use crate::llm::ToolCall;
use crate::llm::providers::chat_completions::client::ChatCompletionsClient;
use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;
use crate::llm::providers::chat_completions::request as chat_completions_request;
use crate::llm::providers::chat_completions::response as chat_completions_response;
use crate::llm::support::http::create_http_client;
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, ToolDefinition,
};
use async_trait::async_trait;
use discovery::{
    ModelProtocol, OPENCODE_GO_PROVIDER_ID, OPENCODE_ZEN_PROVIDER_ID, OpenCodeGoDiscoveryConfig,
    OpenCodeGoModelCatalog,
};
use reqwest::{Client as HttpClient, Url};
use serde_json::{Value, json};
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
const OPENCODE_GO_IMAGE_ANALYSIS_MAX_TOKENS: u32 = 4000;

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

    fn chat_completions_profile(self) -> ChatCompletionsProfile {
        match self.provider_id {
            OPENCODE_ZEN_PROVIDER_ID => ChatCompletionsProfile::opencode_zen(),
            _ => ChatCompletionsProfile::opencode_go(),
        }
    }

    const fn messages_profile(self) -> messages::MessagesProfile {
        messages::MessagesProfile::opencode_go()
    }
}

/// LLM provider implementation for OpenCode Go's OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenCodeGoProvider {
    chat_client: ChatCompletionsClient,
    messages_client: messages::MessagesClient,
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
            Some(api_key.clone()),
            discovery_config,
        ));
        let messages_client = messages::MessagesClient::new(
            http_client.clone(),
            api_base_messages.clone(),
            api_key.clone(),
            profile.messages_profile(),
        );
        let chat_client = ChatCompletionsClient::new(
            http_client,
            api_base,
            Some(api_key),
            "",
            profile.chat_completions_profile(),
        );
        Arc::clone(&model_catalog).spawn_background_refresh();
        Self {
            chat_client,
            messages_client,
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
        let request_kind = match protocol {
            ModelProtocol::OpenAiChatCompletions => "chat_completion",
            ModelProtocol::AnthropicMessages => "messages",
            ModelProtocol::Unknown => {
                return Err(unsupported_protocol_error(model_id, self.profile));
            }
        };
        let body = match protocol {
            ModelProtocol::OpenAiChatCompletions => build_chat_completion_body(
                system_prompt,
                history,
                user_message,
                model_id,
                max_tokens,
            ),
            ModelProtocol::AnthropicMessages => {
                let thinking = messages::response::is_reasoning_model(model_id)
                    .then(|| json!({ "type": "enabled" }));
                messages::request::build_completion_body(
                    system_prompt,
                    history,
                    user_message,
                    normalize_model_id(model_id),
                    max_tokens,
                    OPENCODE_GO_CHAT_TEMPERATURE,
                    thinking,
                )
            }
            ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
        };
        let api_base = match protocol {
            ModelProtocol::OpenAiChatCompletions => self.chat_client.endpoint(),
            ModelProtocol::AnthropicMessages => self.api_base_messages.as_str(),
            ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
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
        let result = async {
            let response = match protocol {
                ModelProtocol::OpenAiChatCompletions => self.chat_client.post_json(&body).await?,
                ModelProtocol::AnthropicMessages => self.messages_client.post_json(&body).await?,
                ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
            };
            let parsed = match protocol {
                ModelProtocol::OpenAiChatCompletions => parse_chat_response(response)?,
                ModelProtocol::AnthropicMessages => messages::response::parse_response(
                    response,
                    messages::MessagesProfile::opencode_go(),
                )?,
                ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
            };
            log_response_summary(self.profile, request_kind, model_id, &parsed);

            parsed.content.ok_or_else(|| {
                LlmError::api_error(format!(
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
        self.analyze_image_with_usage(image_bytes, text_prompt, system_prompt, model_id)
            .await
            .map(|(text, _)| text)
    }

    async fn analyze_image_with_usage(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<(String, Option<crate::llm::TokenUsage>), LlmError> {
        if !discovery::supports_image_input_for_model_id(model_id) {
            return Err(LlmError::api_error(format!(
                "{} model '{}' is not approved for image input",
                self.profile.display_name,
                normalize_model_id_for_prefix(model_id, self.profile.model_prefix)
            )));
        }

        let protocol = self.resolve_model_protocol(model_id).await;
        let (request_kind, api_base, body) = match protocol {
            ModelProtocol::OpenAiChatCompletions => (
                "image_analysis",
                self.chat_client.endpoint(),
                build_image_analysis_body(&image_bytes, text_prompt, system_prompt, model_id),
            ),
            ModelProtocol::AnthropicMessages => {
                return Err(LlmError::api_error(format!(
                    "{} image analysis requires OpenAI Chat Completions protocol for model '{}'",
                    self.profile.display_name,
                    normalize_model_id_for_prefix(model_id, self.profile.model_prefix)
                )));
            }
            ModelProtocol::Unknown => {
                return Err(unsupported_protocol_error(model_id, self.profile));
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

        let result = async {
            let response = self.chat_client.post_json(&body).await?;
            let parsed = parse_chat_response(response.clone())?;
            log_response_summary(self.profile, request_kind, model_id, &parsed);
            let usage = parsed.usage.clone();
            let text = parsed.content.ok_or_else(|| {
                tracing::warn!(
                    provider = self.profile.provider_id,
                    request_kind,
                    model = normalize_model_id_for_prefix(model_id, self.profile.model_prefix),
                    raw_response = %response,
                    "{} returned no text content for image analysis; raw response logged for diagnosis",
                    self.profile.display_name
                );
                LlmError::EmptyResponse(format!(
                    "{} returned no text content for image analysis",
                    self.profile.display_name
                ))
            })?;
            Ok::<(String, Option<crate::llm::TokenUsage>), LlmError>((text, usage))
        }
        .await;
        let text_result = result
            .as_ref()
            .map(|(text, _)| text.clone())
            .map_err(Clone::clone);
        self.throttle.record_result(&text_result);
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
        let request_kind = match protocol {
            ModelProtocol::OpenAiChatCompletions => "chat_with_tools",
            ModelProtocol::AnthropicMessages => "messages_with_tools",
            ModelProtocol::Unknown => {
                return Err(unsupported_protocol_error(model_id, self.profile));
            }
        };
        let body = match protocol {
            ModelProtocol::OpenAiChatCompletions => build_tool_chat_body(
                system_prompt,
                messages,
                tools,
                model_id,
                max_tokens,
                temperature,
                json_mode,
                reasoning_effort,
            ),
            ModelProtocol::AnthropicMessages => {
                let thinking = (messages::response::is_reasoning_model(model_id)
                    && !messages::response::disables_reasoning(reasoning_effort))
                .then(|| json!({ "type": "enabled" }));
                messages::request::build_messages_body(
                    system_prompt,
                    messages,
                    tools,
                    normalize_model_id(model_id),
                    max_tokens,
                    temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
                    thinking,
                )
            }
            ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
        };
        let api_base = match protocol {
            ModelProtocol::OpenAiChatCompletions => self.chat_client.endpoint(),
            ModelProtocol::AnthropicMessages => self.api_base_messages.as_str(),
            ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
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
        let _permit = self.throttle.acquire(model_id).await;
        let result = async {
            let response = match protocol {
                ModelProtocol::OpenAiChatCompletions => self.chat_client.post_json(&body).await?,
                ModelProtocol::AnthropicMessages => self.messages_client.post_json(&body).await?,
                ModelProtocol::Unknown => unreachable!("unknown protocol returned before request"),
            };

            let parsed = match protocol {
                ModelProtocol::OpenAiChatCompletions => parse_chat_response(response)?,
                ModelProtocol::AnthropicMessages => messages::response::parse_response(
                    response,
                    messages::MessagesProfile::opencode_go(),
                )?,
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
        LlmError::RequestBuilder(_) => false,
        LlmError::NetworkError(_) => true,
        LlmError::ApiError {
            status: Some(status),
            ..
        } if *status == 429 || crate::llm::is_transient_server_status(*status) => true,
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
    LlmError::api_error(format!(
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

    trace!(
        provider = event.profile.provider_id,
        request_kind = event.request_kind,
        model = normalize_model_id_for_prefix(event.model_id, event.profile.model_prefix),
        request_body = %event.body,
        "OpenCode request body"
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
    chat_completions_request::build_text_body(
        system_prompt,
        history,
        user_message,
        normalize_model_id(model_id),
        max_tokens,
        opencode_chat_request_options(model_id, None),
        None,
    )
}

fn build_image_analysis_body(
    image_bytes: &[u8],
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
) -> Value {
    chat_completions_request::build_image_body(
        image_bytes,
        None,
        text_prompt,
        system_prompt,
        normalize_model_id(model_id),
        OPENCODE_GO_IMAGE_ANALYSIS_MAX_TOKENS,
        OPENCODE_GO_CHAT_TEMPERATURE,
        opencode_chat_request_options(model_id, None),
    )
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
    chat_completions_request::build_tool_body(
        system_prompt,
        history,
        tools,
        normalize_model_id(model_id),
        max_tokens,
        temperature,
        json_mode,
        opencode_chat_request_options(model_id, reasoning_effort),
        None,
    )
}

#[cfg(test)]
fn prepare_structured_messages(
    system_prompt: &str,
    history: &[Message],
    allow_native_image_parts: bool,
) -> Vec<Value> {
    chat_completions_request::prepare_messages(
        system_prompt,
        history,
        chat_completions_request::ChatRequestOptions::new(ChatCompletionsProfile::opencode_go())
            .with_native_image_parts(allow_native_image_parts),
        None,
    )
}

#[cfg(test)]
fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    chat_completions_request::prepare_tools_json(tools)
}

fn opencode_chat_request_options<'a>(
    model_id: &str,
    reasoning_effort: Option<&'a str>,
) -> chat_completions_request::ChatRequestOptions<'a> {
    chat_completions_request::ChatRequestOptions::new(ChatCompletionsProfile::opencode_go())
        .with_native_image_parts(discovery::supports_image_input_for_model_id(model_id))
        .with_native_image_parts_for_tool_results(false)
        .with_non_empty_tool_result_content(true)
        .with_model_supports_reasoning(messages::response::is_reasoning_model(normalize_model_id(
            model_id,
        )))
        .with_reasoning_disabled(messages::response::disables_reasoning(reasoning_effort))
        .with_reasoning_effort(reasoning_effort)
}

fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
    chat_completions_response::parse_chat_response(
        response,
        ChatCompletionsProfile::opencode_go(),
        None,
    )
}

#[cfg(test)]
fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    chat_completions_response::parse_tool_calls(value, ChatCompletionsProfile::opencode_go(), None)
}

#[cfg(test)]
fn parse_usage(value: &Value) -> Option<crate::llm::TokenUsage> {
    chat_completions_response::parse_usage(value)
}

#[cfg(test)]
mod tests {
    use super::{
        OpenCodeGoAdaptiveThrottle, OpenCodeGoProvider, OpenCodeProviderProfile,
        build_chat_completion_body, build_tool_chat_body, derive_messages_api_base,
        normalize_model_id, opencode_go_should_throttle, parse_chat_response, parse_tool_calls,
        parse_usage, prepare_structured_messages, prepare_tools_json, unsupported_protocol_error,
    };
    use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;
    use crate::llm::providers::messages::MessagesProfile;
    use crate::llm::providers::messages::response::is_reasoning_model;
    use crate::llm::providers::opencode_go::discovery::OpenCodeGoDiscoveryConfig;
    use crate::llm::support::http::create_http_client;
    use crate::llm::{
        LlmError, LlmProvider, Message, MessageContentPart, ToolCall, ToolCallCorrelation,
        ToolCallFunction, ToolDefinition,
    };
    use base64::Engine as _;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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

    fn red_left_blue_right_png() -> Vec<u8> {
        const PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAIAAAABACAIAAABdtOgoAAAAiElEQVR42u3RAQkAAAzDsPo3ves4BCqgkFave76/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgHZHSuHSU0/t5QAAAABJRU5ErkJggg==";
        base64::engine::general_purpose::STANDARD
            .decode(PNG_BASE64)
            .expect("embedded PNG decodes")
    }

    fn opencode_go_smoke_api_key() -> String {
        std::env::var("OPENCODE_GO_API_KEY")
            .or_else(|_| std::env::var("OPENCODE_API_KEY"))
            .ok()
            .filter(|value| !value.trim().is_empty() && value.trim() != "dummy")
            .expect("set OPENCODE_GO_API_KEY or OPENCODE_API_KEY for MiMo vision smoke test")
    }

    async fn run_static_json_server(body: impl Into<String>, max_requests: usize) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        tokio::spawn(async move {
            for _ in 0..max_requests {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 8192];
                let _ = socket.read(&mut buffer).await.expect("read request");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write response");
            }
        });
        format!("http://{addr}/models")
    }

    async fn run_capture_server(
        path: &'static str,
        body: impl Into<String>,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 8192];
            let bytes_read = socket.read(&mut buffer).await.expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            let _ = sender.send(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{addr}{path}"), receiver)
    }

    fn request_body(request: &str) -> serde_json::Value {
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("request contains body separator");
        serde_json::from_str(body).expect("request body is json")
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
    fn opencode_go_openai_branch_delegates_to_chat_completions_profile() {
        let provider = OpenCodeGoProvider::new_with_profile_and_client_and_discovery(
            " token ".to_string(),
            "https://local.test/v1/chat/completions".to_string(),
            "https://local.test/v1/messages".to_string(),
            create_http_client(),
            OpenCodeGoDiscoveryConfig::new_openai_base(
                "https://local.test/v1/models",
                Duration::from_secs(600),
            ),
            OpenCodeProviderProfile::go(),
        );

        assert_eq!(
            provider.chat_client.endpoint(),
            "https://local.test/v1/chat/completions"
        );
        assert_eq!(
            provider.chat_client.profile(),
            ChatCompletionsProfile::opencode_go()
        );
        assert_eq!(
            provider.chat_client.auth_header().as_deref(),
            Some("Bearer token")
        );
        assert_eq!(provider.api_base_messages, "https://local.test/v1/messages");
        assert_eq!(
            provider.messages_client.endpoint(),
            provider.api_base_messages
        );
        assert_eq!(
            provider.messages_client.profile(),
            MessagesProfile::opencode_go()
        );

        let zen_profile = OpenCodeProviderProfile::zen().chat_completions_profile();
        assert_eq!(zen_profile, ChatCompletionsProfile::opencode_zen());
        assert_eq!(
            zen_profile.default_endpoint,
            "https://opencode.ai/zen/v1/chat/completions"
        );
    }

    #[tokio::test]
    async fn opencode_go_anthropic_branch_uses_messages_api_base() {
        let models_url =
            run_static_json_server(r#"{"data":[{"id":"minimax-m2","object":"model"}]}"#, 4).await;
        let (messages_endpoint, request_rx) = run_capture_server(
            "/v1/messages",
            r#"{"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#,
        )
        .await;
        let provider = OpenCodeGoProvider::new_with_profile_and_client_and_discovery(
            " token ".to_string(),
            "http://127.0.0.1:9/v1/chat/completions".to_string(),
            messages_endpoint,
            reqwest::Client::new(),
            OpenCodeGoDiscoveryConfig::new(models_url, Duration::from_secs(600), BTreeMap::new()),
            OpenCodeProviderProfile::go(),
        );

        let response = provider
            .complete_internal_text("system", &[], "hello", "opencode-go/minimax-m2", 32)
            .await
            .expect("messages branch succeeds");
        let request = request_rx.await.expect("request captured");
        let lowercase = request.to_ascii_lowercase();
        let body = request_body(&request);

        assert_eq!(response, "ok");
        assert!(request.starts_with("POST /v1/messages HTTP/1.1"));
        assert!(lowercase.contains("authorization: bearer token"));
        assert!(lowercase.contains("x-api-key:  token "));
        assert!(lowercase.contains("anthropic-version: 2023-06-01"));
        assert_eq!(body["model"], json!("minimax-m2"));
        assert_eq!(body["system"], json!("system"));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn opencode_go_anthropic_branch_preserves_fallback_tool_use_prefix() {
        let response = crate::llm::providers::messages::response::parse_response(
            json!({
                "content": [{
                    "type": "tool_use",
                    "id": "",
                    "name": "read_file",
                    "input": {"path":"Cargo.toml"}
                }],
                "stop_reason": "tool_use"
            }),
            MessagesProfile::opencode_go(),
        )
        .expect("messages response parses");

        assert_eq!(response.tool_calls.len(), 1);
        assert_eq!(
            response.tool_calls[0].wire_tool_call_id(),
            "opencode_go_tool_use_0"
        );
        assert_eq!(response.finish_reason, "tool_calls");
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
        // Shared is_reasoning_model does not normalize (no prefix stripping);
        // callers must normalize_model_id() first.
        // DeepSeek V4 family
        assert!(is_reasoning_model("deepseek-v4-flash"));
        assert!(is_reasoning_model("deepseek-v4-pro"));
        assert!(is_reasoning_model(" DEEPSEEK-V4-FLASH "));

        // MiMo V2 family
        assert!(is_reasoning_model("mimo-v2.5"));
        assert!(is_reasoning_model("mimo-v2.5-pro"));

        // Non-reasoning models
        assert!(!is_reasoning_model("deepseek-v3"));
        assert!(!is_reasoning_model("deepseek-chat"));
        assert!(!is_reasoning_model("gpt-4o"));
        assert!(!is_reasoning_model("qwen3-235b-a22b"));
        // Prefixed IDs must be normalized first
        assert!(!is_reasoning_model("opencode-go/deepseek-v4-flash"));
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
                vec![
                    ToolCall::new(
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
                    ),
                ],
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
        assert_eq!(text_only_body["messages"][3]["content"], json!("contents"));
        assert!(text_only_body["messages"][3]["content"].is_string());
    }

    #[tokio::test]
    async fn smoke_opencode_go_mimo_v25_accepts_image_input() {
        if !matches!(
            std::env::var("RUN_OPENCODE_GO_MIMO_VISION_SMOKE").as_deref(),
            Ok("1")
        ) {
            return;
        }

        let api_key = opencode_go_smoke_api_key();
        let api_base = std::env::var("OPENCODE_GO_API_BASE")
            .unwrap_or_else(|_| "https://opencode.ai/zen/go/v1/chat/completions".to_string());
        let provider = OpenCodeGoProvider::new(api_key, api_base);

        let response = provider
            .analyze_image(
                red_left_blue_right_png(),
                "Look at the image. What color is the left half, and what color is the right half? Answer in English using only the two color names and their positions.",
                "You are validating whether an image input reached the model. Do not guess from the prompt; answer only from the visible image.",
                "mimo-v2.5",
            )
            .await
            .expect("OpenCode Go mimo-v2.5 should accept image_url data URL image input");

        let normalized = response.to_ascii_lowercase();
        assert!(
            normalized.contains("red"),
            "MiMo smoke response did not identify the red half: {response}"
        );
        assert!(
            normalized.contains("blue"),
            "MiMo smoke response did not identify the blue half: {response}"
        );
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
        assert!(
            error
                .to_string()
                .contains("modules.llm-provider/opencode-zen.protocol_overrides")
        );
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
            throttle.record_result::<()>(&Err(LlmError::api_error_status(
                500,
                "500 Internal Server Error",
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
        assert!(opencode_go_should_throttle(&LlmError::api_error_status(
            500,
            "500 Internal Server Error"
        )));
        assert!(opencode_go_should_throttle(&LlmError::NetworkError(
            "connection reset".to_string()
        )));
        assert!(!opencode_go_should_throttle(&LlmError::api_error(
            "invalid API key"
        )));
    }
}
