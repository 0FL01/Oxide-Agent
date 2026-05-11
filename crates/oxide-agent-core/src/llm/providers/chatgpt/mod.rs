mod auth;

pub use auth::{
    resolve_auth_file_path, ChatGptAuthFlow, ChatGptAuthRecord, ChatGptAuthStatus,
    ChatGptDeviceAuthorization,
};

use self::auth::ChatGptAuthManager;
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::http::{create_http_client, parse_retry_after};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolDefinition,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{header::HeaderMap, Client as HttpClient};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf};

const CHATGPT_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const JSON_OBJECT_INSTRUCTIONS_SUFFIX: &str =
    "Return valid JSON only. The response must be a single JSON object with no markdown or extra text.";

/// ChatGPT headless OAuth provider.
#[derive(Debug, Clone)]
pub struct ChatGptProvider {
    http_client: HttpClient,
    auth: ChatGptAuthManager,
}

#[derive(Debug, Default)]
struct StreamedChatGptResponse {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
    finish_reason: String,
    usage: Option<TokenUsage>,
}

#[derive(Debug, Clone)]
struct ChatGptResponseMetadata {
    status: reqwest::StatusCode,
    request_id: Option<String>,
    trace_id: Option<String>,
}

#[derive(Debug, Default)]
struct ChatGptStreamDiagnostics {
    event_count: usize,
    text_delta_count: usize,
    tool_call_count: usize,
    saw_completion: bool,
    ignored_event_types: BTreeMap<String, usize>,
}

impl ChatGptStreamDiagnostics {
    fn record_ignored_event(&mut self, event_type: impl Into<String>) {
        *self
            .ignored_event_types
            .entry(event_type.into())
            .or_insert(0) += 1;
    }
}

impl ChatGptProvider {
    #[must_use]
    pub fn new(auth_path: impl Into<PathBuf>) -> Self {
        let http_client = create_http_client();
        Self::new_with_client(auth_path, http_client)
    }

    #[must_use]
    pub fn new_with_client(auth_path: impl Into<PathBuf>, http_client: HttpClient) -> Self {
        let auth_path = auth_path.into();
        let auth_path = auth_path
            .to_str()
            .and_then(|path| auth::resolve_auth_file_path(Some(path)).ok())
            .unwrap_or(auth_path);
        let auth = ChatGptAuthManager::new(auth_path, http_client.clone());
        Self { http_client, auth }
    }

    async fn chat_request(&self, body: Value) -> Result<StreamedChatGptResponse, LlmError> {
        let session = self.auth.get_valid_session().await?;

        let response = self.send_chat_request(&session, &body).await?;
        match response {
            ChatRequestOutcome::Success(response) => parse_streaming_response(response).await,
            ChatRequestOutcome::UnsupportedParameter {
                parameter,
                status,
                response_body,
                request_body,
            } => {
                let Some(retried_body) = remove_unsupported_parameter(request_body, &parameter)
                else {
                    return Err(LlmError::ApiError(format!(
                        "ChatGPT API error: {status} - {response_body}"
                    )));
                };

                match self.send_chat_request(&session, &retried_body).await? {
                    ChatRequestOutcome::Success(response) => {
                        parse_streaming_response(response).await
                    }
                    ChatRequestOutcome::UnsupportedParameter {
                        status,
                        response_body,
                        ..
                    } => Err(LlmError::ApiError(format!(
                        "ChatGPT API error: {status} - {response_body}"
                    ))),
                }
            }
        }
    }

    async fn send_chat_request(
        &self,
        session: &auth::ChatGptSession,
        body: &Value,
    ) -> Result<ChatRequestOutcome, LlmError> {
        let mut request = self
            .http_client
            .post(CHATGPT_CODEX_API_ENDPOINT)
            .header("Authorization", format!("Bearer {}", session.access_token))
            .header("Content-Type", "application/json");

        if !session.account_id.is_empty() {
            request = request.header("ChatGPT-Account-Id", session.account_id.clone());
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait_secs = parse_retry_after(response.headers());
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: body,
                });
            }

            let response_body = response.text().await.unwrap_or_default();
            if let Some(parameter) = parse_unsupported_parameter(&response_body) {
                return Ok(ChatRequestOutcome::UnsupportedParameter {
                    parameter,
                    status,
                    response_body,
                    request_body: body.clone(),
                });
            }

            return Err(LlmError::ApiError(format!(
                "ChatGPT API error: {status} - {response_body}"
            )));
        }

        Ok(ChatRequestOutcome::Success(response))
    }
}

enum ChatRequestOutcome {
    Success(reqwest::Response),
    UnsupportedParameter {
        parameter: String,
        status: reqwest::StatusCode,
        response_body: String,
        request_body: Value,
    },
}

#[async_trait]
impl LlmProvider for ChatGptProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let (instructions, mut input) = prepare_responses_request(system_prompt, history);
        input.push(user_input_item(user_message));

        let body =
            build_chat_request_body(&instructions, input, &[], model_id, max_tokens, None, false);
        let response = self.chat_request(body).await?;

        response
            .content
            .ok_or_else(|| empty_chatgpt_response_error(model_id, "chat_completion_post_parse"))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not implemented for ChatGPT OAuth".to_string(),
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
            "Image analysis not implemented for ChatGPT OAuth".to_string(),
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

        let (instructions, input) = prepare_responses_request(system_prompt, messages);
        let body = build_chat_request_body(
            &instructions,
            input,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        );
        let response = self.chat_request(body).await?;

        let content = response.content;
        let tool_calls = response.tool_calls;

        if content.is_none() && tool_calls.is_empty() {
            return Err(empty_chatgpt_response_error(
                model_id,
                "chat_with_tools_post_parse",
            ));
        }

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason: response.finish_reason,
            reasoning_content: None,
            usage: response.usage,
        })
    }
}

fn build_chat_request_body(
    instructions: &str,
    input: Vec<Value>,
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
) -> Value {
    let input = if json_mode && tools.is_empty() {
        ensure_json_input_marker(input)
    } else {
        input
    };

    let instructions = if json_mode && tools.is_empty() {
        ensure_json_instructions(instructions)
    } else {
        instructions.to_string()
    };

    let mut body = json!({
        "model": model_id,
        "instructions": instructions,
        "input": input,
        "stream": true,
        "store": false,
    });

    let _ = max_tokens;

    if let Some(temperature) = temperature.filter(|_| !model_id.starts_with("gpt-5")) {
        body["temperature"] = json!(temperature);
    }

    if !tools.is_empty() {
        body["tools"] = json!(prepare_tools_json(tools));
        body["tool_choice"] = json!("auto");
    }

    if json_mode && tools.is_empty() {
        body["text"] = json!({
            "format": {
                "type": "json_object"
            }
        });
    }

    if model_id.starts_with("gpt-5") {
        body["reasoning"] = json!({ "effort": "medium" });
        body["truncation"] = json!("auto");
    }

    body
}

fn ensure_json_instructions(instructions: &str) -> String {
    if instructions.to_ascii_lowercase().contains("json") {
        return instructions.to_string();
    }

    if instructions.trim().is_empty() {
        JSON_OBJECT_INSTRUCTIONS_SUFFIX.to_string()
    } else {
        format!("{instructions}\n\n{JSON_OBJECT_INSTRUCTIONS_SUFFIX}")
    }
}

fn ensure_json_input_marker(mut input: Vec<Value>) -> Vec<Value> {
    if input_contains_json_word(&input) {
        return input;
    }

    input.insert(
        0,
        json!({
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "Return valid JSON only.",
            }],
        }),
    );
    input
}

fn input_contains_json_word(input: &[Value]) -> bool {
    input.iter().any(value_contains_json_word)
}

fn value_contains_json_word(value: &Value) -> bool {
    match value {
        Value::String(text) => text.to_ascii_lowercase().contains("json"),
        Value::Array(items) => items.iter().any(value_contains_json_word),
        Value::Object(map) => map.values().any(value_contains_json_word),
        _ => false,
    }
}

fn parse_unsupported_parameter(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(body).ok()?;
    let detail = parsed.get("detail")?.as_str()?;
    detail
        .strip_prefix("Unsupported parameter: ")
        .map(ToString::to_string)
}

fn remove_unsupported_parameter(mut body: Value, parameter: &str) -> Option<Value> {
    let object = body.as_object_mut()?;
    object.remove(parameter)?;
    Some(body)
}

fn prepare_responses_request(system_prompt: &str, history: &[Message]) -> (String, Vec<Value>) {
    let mut instructions = system_prompt.trim().to_string();
    let mut input = Vec::new();

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                if !msg.content.trim().is_empty() {
                    if !instructions.is_empty() {
                        instructions.push_str("\n\n");
                    }
                    instructions.push_str(msg.content.trim());
                }
            }
            "assistant" => {
                if !msg.content.trim().is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": msg.content,
                        }],
                    }));
                }

                if let Some(tool_calls) = &msg.tool_calls {
                    input.extend(tool_calls.iter().filter_map(|tool_call| {
                        CHAT_LIKE_TOOL_PROFILE
                            .encode_tool_call(tool_call)
                            .and_then(|call| call.into_chat_like())
                            .map(|call| {
                                json!({
                                    "type": "function_call",
                                    "call_id": call.id,
                                    "name": call.name,
                                    "arguments": call.arguments,
                                })
                            })
                    }));
                }
            }
            "tool" => {
                if let Some(result) = msg
                    .resolved_tool_call_correlation()
                    .map(|correlation| correlation.wire_tool_call_id().to_string())
                    .or_else(|| msg.tool_call_id.clone())
                {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": result,
                        "output": msg.content,
                    }));
                }
            }
            _ => {
                input.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": msg.content,
                    }],
                }));
            }
        }
    }

    (instructions, input)
}

fn user_input_item(content: &str) -> Value {
    json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": content,
        }],
    })
}

fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect()
}

async fn parse_streaming_response(
    response: reqwest::Response,
) -> Result<StreamedChatGptResponse, LlmError> {
    let metadata = ChatGptResponseMetadata::from_headers(response.status(), response.headers());
    let mut state = StreamedChatGptResponse {
        finish_reason: "unknown".to_string(),
        ..StreamedChatGptResponse::default()
    };
    let mut diagnostics = ChatGptStreamDiagnostics::default();
    let mut buffer = String::new();
    let mut pending_bytes = Vec::new();
    let mut current_text_item_id: Option<String> = None;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| LlmError::NetworkError(error.to_string()))?;
        pending_bytes.extend_from_slice(&chunk);
        if let Some(decoded) = decode_utf8_prefix(&mut pending_bytes)? {
            buffer.push_str(&decoded);
        }
        normalize_newlines_in_place(&mut buffer);

        while let Some(boundary) = buffer.find("\n\n") {
            let raw_event = buffer[..boundary].to_string();
            buffer = buffer[(boundary + 2)..].to_string();
            process_sse_event(
                &raw_event,
                &mut state,
                &mut current_text_item_id,
                &mut diagnostics,
            )?;
        }
    }

    if !pending_bytes.is_empty() {
        let tail = String::from_utf8(pending_bytes)
            .map_err(|error| LlmError::JsonError(error.to_string()))?;
        buffer.push_str(&tail);
        normalize_newlines_in_place(&mut buffer);
    }

    if !buffer.trim().is_empty() {
        process_sse_event(
            &buffer,
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )?;
    }

    finish_streaming_response(state, diagnostics, metadata)
}

fn finish_streaming_response(
    mut state: StreamedChatGptResponse,
    diagnostics: ChatGptStreamDiagnostics,
    metadata: ChatGptResponseMetadata,
) -> Result<StreamedChatGptResponse, LlmError> {
    if state.finish_reason == "unknown" {
        state.finish_reason = if state.tool_calls.is_empty() {
            "stop".to_string()
        } else {
            "tool_calls".to_string()
        };
    }

    if state.content.is_none() && state.tool_calls.is_empty() {
        return Err(LlmError::EmptyResponse(
            format_chatgpt_empty_response_details(&state, &diagnostics, &metadata),
        ));
    }

    Ok(state)
}

fn process_sse_event(
    raw_event: &str,
    state: &mut StreamedChatGptResponse,
    current_text_item_id: &mut Option<String>,
    diagnostics: &mut ChatGptStreamDiagnostics,
) -> Result<(), LlmError> {
    let payload = raw_event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if payload.is_empty() || payload == "[DONE]" {
        return Ok(());
    }

    diagnostics.event_count += 1;

    let value: Value =
        serde_json::from_str(&payload).map_err(|error| LlmError::JsonError(error.to_string()))?;
    match value.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => {
            let delta = value
                .get("delta")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if delta.is_empty() {
                return Ok(());
            }
            diagnostics.text_delta_count += 1;
            let item_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let content = state.content.get_or_insert_with(String::new);
            if !content.is_empty()
                && item_id.is_some()
                && current_text_item_id.as_ref() != item_id.as_ref()
            {
                content.push('\n');
            }
            content.push_str(delta);
            *current_text_item_id = item_id;
        }
        Some("response.output_item.done") => {
            if let Some(item) = value.get("item") {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let Some(name) = item.get("name").and_then(Value::as_str) else {
                        return Ok(());
                    };
                    let arguments = item
                        .get("arguments")
                        .and_then(|value| {
                            value
                                .as_str()
                                .map(ToString::to_string)
                                .or_else(|| serde_json::to_string(value).ok())
                        })
                        .unwrap_or_default();
                    let wire_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .filter(|id| !id.trim().is_empty());
                    state.tool_calls.push(match wire_id {
                        Some(wire_id) => CHAT_LIKE_TOOL_PROFILE.inbound_provider_tool_call(
                            wire_id,
                            None,
                            name.to_string(),
                            arguments,
                        ),
                        None => CHAT_LIKE_TOOL_PROFILE
                            .inbound_uncorrelated_tool_call(name.to_string(), arguments),
                    });
                    diagnostics.tool_call_count += 1;
                } else {
                    diagnostics.record_ignored_event("response.output_item.done");
                }
            }
        }
        Some("response.completed") | Some("response.incomplete") => {
            diagnostics.saw_completion = true;
            let reason = value
                .get("response")
                .and_then(|response| response.get("incomplete_details"))
                .and_then(|details| details.get("reason"))
                .and_then(Value::as_str);
            state.finish_reason = reason
                .unwrap_or(if state.tool_calls.is_empty() {
                    "stop"
                } else {
                    "tool_calls"
                })
                .to_string();
            state.usage = value
                .get("response")
                .and_then(|response| response.get("usage"))
                .and_then(parse_usage);
        }
        Some("error") => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown ChatGPT stream error");
            return Err(LlmError::ApiError(format!(
                "ChatGPT stream error: {message}"
            )));
        }
        Some(event_type) => diagnostics.record_ignored_event(event_type),
        None => diagnostics.record_ignored_event("<missing_type>"),
    }

    Ok(())
}

fn empty_chatgpt_response_error(model_id: &str, stage: &str) -> LlmError {
    LlmError::EmptyResponse(format!(
        " (provider=chatgpt, model={model_id}, stage={stage})"
    ))
}

fn format_chatgpt_empty_response_details(
    state: &StreamedChatGptResponse,
    diagnostics: &ChatGptStreamDiagnostics,
    metadata: &ChatGptResponseMetadata,
) -> String {
    let mut parts = vec![
        "provider=chatgpt".to_string(),
        format!("status={}", metadata.status),
        format!("finish_reason={}", state.finish_reason),
        format!("events={}", diagnostics.event_count),
        format!("text_deltas={}", diagnostics.text_delta_count),
        format!("tool_calls={}", diagnostics.tool_call_count),
        format!("saw_completion={}", diagnostics.saw_completion),
    ];

    if let Some(request_id) = &metadata.request_id {
        parts.push(format!("request_id={request_id}"));
    }
    if let Some(trace_id) = &metadata.trace_id {
        parts.push(format!("trace_id={trace_id}"));
    }
    if !diagnostics.ignored_event_types.is_empty() {
        let ignored = diagnostics
            .ignored_event_types
            .iter()
            .map(|(event_type, count)| {
                if *count == 1 {
                    event_type.clone()
                } else {
                    format!("{event_type}×{count}")
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("ignored_event_types={ignored}"));
    }

    format!(" ({})", parts.join(", "))
}

fn response_header_value(headers: &HeaderMap, candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    })
}

impl ChatGptResponseMetadata {
    fn from_headers(status: reqwest::StatusCode, headers: &HeaderMap) -> Self {
        Self {
            status,
            request_id: response_header_value(headers, &["x-request-id", "request-id"]),
            trace_id: response_header_value(headers, &["traceparent", "x-trace-id", "cf-ray"]),
        }
    }
}

fn decode_utf8_prefix(pending_bytes: &mut Vec<u8>) -> Result<Option<String>, LlmError> {
    match std::str::from_utf8(pending_bytes) {
        Ok(valid) => {
            let decoded = valid.to_string();
            pending_bytes.clear();
            Ok((!decoded.is_empty()).then_some(decoded))
        }
        Err(error) => {
            let valid_up_to = error.valid_up_to();
            if let Some(error_len) = error.error_len() {
                return Err(LlmError::JsonError(format!(
                    "invalid utf-8 in ChatGPT stream at {valid_up_to} (len {error_len})"
                )));
            }

            if valid_up_to == 0 {
                return Ok(None);
            }

            let decoded = String::from_utf8(pending_bytes[..valid_up_to].to_vec())
                .map_err(|error| LlmError::JsonError(error.to_string()))?;
            pending_bytes.drain(..valid_up_to);
            Ok(Some(decoded))
        }
    }
}

fn normalize_newlines_in_place(buffer: &mut String) {
    if buffer.contains('\r') {
        *buffer = buffer.replace("\r\n", "\n");
    }
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value
            .get("input_tokens")
            .or_else(|| value.get("prompt_tokens"))?
            .as_u64()? as u32,
        completion_tokens: value
            .get("output_tokens")
            .or_else(|| value.get("completion_tokens"))?
            .as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_chat_request_body, decode_utf8_prefix, finish_streaming_response,
        prepare_responses_request, process_sse_event, ChatGptResponseMetadata,
        ChatGptStreamDiagnostics, StreamedChatGptResponse,
    };
    use crate::llm::{LlmError, Message, ToolCall, ToolCallCorrelation, ToolCallFunction};
    use reqwest::StatusCode;
    use serde_json::json;

    #[test]
    fn json_mode_uses_json_object_without_tools() {
        let body = build_chat_request_body(
            "system",
            vec![json!({"role":"user","content":[{"type":"input_text","text":"hi"}]})],
            &[],
            "gpt-5.4",
            10,
            None,
            true,
        );

        assert!(body["instructions"]
            .as_str()
            .is_some_and(|value| value.contains("JSON")));
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            json!("Return valid JSON only.")
        );
        assert_eq!(body["stream"], json!(true));
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["text"]["format"]["type"], json!("json_object"));
        assert_eq!(body["reasoning"]["effort"], json!("medium"));
        assert_eq!(body["truncation"], json!("auto"));
    }

    #[test]
    fn json_mode_preserves_existing_json_instructions() {
        let body = build_chat_request_body(
            "Return JSON only.",
            vec![json!({"role":"user","content":[{"type":"input_text","text":"hi"}]})],
            &[],
            "gpt-5.4",
            10,
            None,
            true,
        );

        assert_eq!(body["instructions"], json!("Return JSON only."));
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            json!("Return valid JSON only.")
        );
    }

    #[test]
    fn json_mode_preserves_existing_json_word_in_input() {
        let body = build_chat_request_body(
            "system",
            vec![
                json!({"role":"user","content":[{"type":"input_text","text":"Please answer in JSON."}]}),
            ],
            &[],
            "gpt-5.4",
            10,
            None,
            true,
        );

        assert_eq!(body["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            body["input"][0]["content"][0]["text"],
            json!("Please answer in JSON.")
        );
    }

    #[test]
    fn structured_history_preserves_wire_tool_ids() {
        let history = vec![
            Message::assistant_with_tools(
                "Calling tools",
                vec![ToolCall::new(
                    "invoke-chatgpt-1",
                    ToolCallFunction {
                        name: "search".to_string(),
                        arguments: r#"{"query":"oxide"}"#.to_string(),
                    },
                    false,
                )
                .with_correlation(
                    ToolCallCorrelation::new("invoke-chatgpt-1")
                        .with_provider_tool_call_id("call-chatgpt-1"),
                )],
            ),
            Message::tool_with_correlation(
                "invoke-chatgpt-1",
                ToolCallCorrelation::new("invoke-chatgpt-1")
                    .with_provider_tool_call_id("call-chatgpt-1"),
                "search",
                "result",
            ),
        ];

        let (instructions, input) = prepare_responses_request("system", &history);

        assert_eq!(instructions, "system");
        assert_eq!(input[1]["call_id"], json!("call-chatgpt-1"));
        assert_eq!(input[2]["call_id"], json!("call-chatgpt-1"));
    }

    #[test]
    fn sse_function_call_preserves_provider_ids() {
        let mut state = StreamedChatGptResponse::default();
        let mut current_text_item_id = None;
        let mut diagnostics = ChatGptStreamDiagnostics::default();
        process_sse_event(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call-chatgpt-2\",\"name\":\"search\",\"arguments\":\"{\\\"query\\\":\\\"oxide\\\"}\",\"status\":\"completed\"}}",
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )
        .expect("tool calls parse");

        assert_ne!(
            state.tool_calls[0].invocation_id().as_str(),
            "call-chatgpt-2"
        );
        assert_eq!(state.tool_calls[0].wire_tool_call_id(), "call-chatgpt-2");
    }

    #[test]
    fn sse_text_and_finish_are_assembled() {
        let mut state = StreamedChatGptResponse::default();
        let mut current_text_item_id = None;
        let mut diagnostics = ChatGptStreamDiagnostics::default();
        process_sse_event(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"hello\"}",
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )
        .expect("text delta");
        process_sse_event(
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\" world\"}",
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )
        .expect("text delta");
        process_sse_event(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":4,\"total_tokens\":14}}}",
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )
        .expect("finish");

        assert_eq!(state.content.as_deref(), Some("hello world"));
        assert_eq!(state.finish_reason, "stop");
        assert_eq!(
            state.usage.as_ref().map(|usage| usage.total_tokens),
            Some(14)
        );
    }

    #[test]
    fn sse_ignored_event_types_are_tracked() {
        let mut state = StreamedChatGptResponse::default();
        let mut current_text_item_id = None;
        let mut diagnostics = ChatGptStreamDiagnostics::default();

        process_sse_event(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}",
            &mut state,
            &mut current_text_item_id,
            &mut diagnostics,
        )
        .expect("created event parses");

        assert_eq!(diagnostics.event_count, 1);
        assert_eq!(
            diagnostics.ignored_event_types.get("response.created"),
            Some(&1)
        );
    }

    #[test]
    fn empty_stream_error_contains_diagnostics() {
        let mut diagnostics = ChatGptStreamDiagnostics {
            event_count: 2,
            ..ChatGptStreamDiagnostics::default()
        };
        diagnostics.record_ignored_event("response.created");
        diagnostics.record_ignored_event("response.output_item.added");

        let error = finish_streaming_response(
            StreamedChatGptResponse::default(),
            diagnostics,
            ChatGptResponseMetadata {
                status: StatusCode::OK,
                request_id: Some("req_123".to_string()),
                trace_id: Some("trace_456".to_string()),
            },
        )
        .expect_err("empty stream should fail");

        match error {
            LlmError::EmptyResponse(details) => {
                assert!(details.contains("provider=chatgpt"));
                assert!(details.contains("status=200 OK"));
                assert!(details.contains("request_id=req_123"));
                assert!(details.contains("trace_id=trace_456"));
                assert!(details.contains("events=2"));
                assert!(details.contains("saw_completion=false"));
                assert!(details
                    .contains("ignored_event_types=response.created,response.output_item.added"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn unsupported_parameter_parser_extracts_field_name() {
        assert_eq!(
            super::parse_unsupported_parameter(
                r#"{"detail":"Unsupported parameter: max_output_tokens"}"#
            )
            .as_deref(),
            Some("max_output_tokens")
        );
    }

    #[test]
    fn unsupported_parameter_removal_drops_requested_key() {
        let body = json!({
            "model": "gpt-5.4-mini",
            "max_output_tokens": 1024,
            "stream": true,
        });

        let updated =
            super::remove_unsupported_parameter(body, "max_output_tokens").expect("updated body");

        assert!(updated.get("max_output_tokens").is_none());
        assert_eq!(updated["stream"], json!(true));
    }

    #[test]
    fn decode_utf8_prefix_handles_split_multibyte_sequences() {
        let mut pending = vec![0xF0, 0x9F];
        assert!(decode_utf8_prefix(&mut pending)
            .expect("partial utf8")
            .is_none());

        pending.extend_from_slice(&[0x99, 0x82]);
        assert_eq!(
            decode_utf8_prefix(&mut pending)
                .expect("completed utf8")
                .as_deref(),
            Some("🙂")
        );
        assert!(pending.is_empty());
    }
}
