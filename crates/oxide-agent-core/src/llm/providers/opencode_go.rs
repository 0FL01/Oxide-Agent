use crate::config::{get_opencode_go_max_concurrent, OPENCODE_GO_CHAT_TEMPERATURE};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{create_http_client, send_json_request};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use reqwest::{Client as HttpClient, Url};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tracing::{debug, trace, warn};

pub(crate) mod module;
pub(crate) use module::OpenCodeGoProviderModule;

const OPENCODE_GO_FAILURES_BEFORE_COOLDOWN: usize = 3;
const OPENCODE_GO_COOLDOWN_STEP_SECS: u64 = 5;
const OPENCODE_GO_MAX_COOLDOWN_SECS: u64 = 60;
const OPENCODE_GO_SUCCESS_STREAK_TO_INCREASE: usize = 3;

/// LLM provider implementation for OpenCode Go's OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenCodeGoProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
    throttle: Arc<OpenCodeGoAdaptiveThrottle>,
}

impl OpenCodeGoProvider {
    /// Create a new OpenCode Go provider instance.
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: create_http_client(),
            api_key,
            api_base,
            throttle: OpenCodeGoAdaptiveThrottle::from_env(),
        }
    }

    /// Create a new OpenCode Go provider with a shared HTTP client.
    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        Self {
            http_client,
            api_key,
            api_base,
            throttle: OpenCodeGoAdaptiveThrottle::from_env(),
        }
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
        let _permit = self.throttle.acquire(model_id).await;
        let body =
            build_chat_completion_body(system_prompt, history, user_message, model_id, max_tokens);
        log_request_summary(
            "chat_completion",
            &self.api_base,
            model_id,
            max_tokens,
            OPENCODE_GO_CHAT_TEMPERATURE,
            false,
            &body,
        );
        let auth = format!("Bearer {}", self.api_key);
        let result = async {
            let response =
                send_json_request(&self.http_client, &self.api_base, &body, Some(&auth), &[])
                    .await?;
            let parsed = parse_chat_response(response)?;
            log_response_summary("chat_completion", model_id, &parsed);

            parsed.content.ok_or_else(|| {
                LlmError::ApiError(
                    "OpenCode Go returned no text content for chat_completion".to_string(),
                )
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
        Err(LlmError::Unknown(
            "Audio transcription not supported by OpenCode Go".to_string(),
        ))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Image analysis not supported by OpenCode Go".to_string(),
        ))
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
        } = request;
        let body = build_tool_chat_body(
            system_prompt,
            messages,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        );
        log_request_summary(
            "chat_with_tools",
            &self.api_base,
            model_id,
            max_tokens,
            temperature.unwrap_or(OPENCODE_GO_CHAT_TEMPERATURE),
            json_mode,
            &body,
        );
        let auth = format!("Bearer {}", self.api_key);
        let _permit = self.throttle.acquire(model_id).await;
        let result = async {
            let response =
                send_json_request(&self.http_client, &self.api_base, &body, Some(&auth), &[])
                    .await?;

            let parsed = parse_chat_response(response)?;
            log_response_summary("chat_with_tools", model_id, &parsed);
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
    let trimmed = model_id.trim();
    trimmed.strip_prefix("opencode-go/").unwrap_or(trimmed)
}

fn log_request_summary(
    request_kind: &str,
    api_base: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
    json_mode: bool,
    body: &Value,
) {
    let (endpoint_host, endpoint_path) = endpoint_parts(api_base);
    let message_count = body
        .get("messages")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let tool_count = body
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    debug!(
        provider = "opencode-go",
        request_kind,
        model = normalize_model_id(model_id),
        endpoint_host = endpoint_host.as_str(),
        endpoint_path = endpoint_path.as_str(),
        json_mode,
        has_tools = tool_count > 0,
        tool_count,
        message_count,
        max_tokens,
        temperature,
        request_body_bytes = json_body_len(body),
        "OpenCode Go request summary"
    );
}

fn log_response_summary(request_kind: &str, model_id: &str, response: &ChatResponse) {
    let usage = response.usage.as_ref();
    debug!(
        provider = "opencode-go",
        request_kind,
        model = normalize_model_id(model_id),
        finish_reason = response.finish_reason.as_str(),
        content_len = response.content.as_ref().map_or(0, String::len),
        reasoning_len = response.reasoning_content.as_ref().map_or(0, String::len),
        tool_call_count = response.tool_calls.len(),
        usage_prompt_tokens = usage.map(|usage| usage.prompt_tokens),
        usage_completion_tokens = usage.map(|usage| usage.completion_tokens),
        usage_total_tokens = usage.map(|usage| usage.total_tokens),
        "OpenCode Go response summary"
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
    let mut messages = prepare_structured_messages(system_prompt, history);
    messages.push(json!({
        "role": "user",
        "content": user_message,
    }));

    json!({
        "model": normalize_model_id(model_id),
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": OPENCODE_GO_CHAT_TEMPERATURE,
        "stream": false,
    })
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
) -> Value {
    let messages = prepare_structured_messages(system_prompt, history);
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

    body
}

fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
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
                    "content": msg.content,
                }));
            }
        }
    }

    messages
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

fn should_use_native_json_mode(json_mode: bool, has_tools: bool) -> bool {
    json_mode && !has_tools
}

fn parse_chat_response(response: Value) -> Result<ChatResponse, LlmError> {
    let choice = response
        .get("choices")
        .and_then(|choices| choices.get(0))
        .ok_or_else(|| {
            LlmError::ApiError("Missing choices[0] in OpenCode Go response".to_string())
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
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_chat_completion_body, build_tool_chat_body, normalize_model_id,
        opencode_go_should_throttle, parse_chat_response, parse_tool_calls,
        prepare_structured_messages, prepare_tools_json, OpenCodeGoAdaptiveThrottle,
    };
    use crate::llm::{
        LlmError, Message, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition,
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
    fn tool_request_body_includes_function_names() {
        let tools = vec![read_file_tool()];
        let body = build_tool_chat_body(
            "system",
            &[],
            &tools,
            "deepseek-v4-flash",
            32000,
            Some(0.2),
            true,
        );

        assert_eq!(body["tools"][0]["type"], json!("function"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("read_file"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn json_mode_without_tools_sets_response_format() {
        let body = build_tool_chat_body("system", &[], &[], "deepseek-v4-flash", 32000, None, true);

        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
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

        let messages = prepare_structured_messages("system", &history);

        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("call-opencode-1"));
        assert_eq!(
            messages[1]["reasoning_content"],
            json!("provider thinking trace")
        );
        assert_eq!(messages[2]["tool_call_id"], json!("call-opencode-1"));
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
