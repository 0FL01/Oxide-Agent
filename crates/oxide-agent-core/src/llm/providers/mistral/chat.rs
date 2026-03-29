//! Chat completion functionality for Mistral provider

use crate::config::{
    MISTRAL_CHAT_TEMPERATURE, MISTRAL_REASONING_TEMPERATURE, MISTRAL_TOOL_TEMPERATURE,
};
use crate::llm::providers::mistral::{
    id_mapper::ToolCallIdMapper,
    messages::{prepare_chat_messages, prepare_structured_messages},
    parsing::parse_chat_response,
    types::{MISTRAL_REASONING_EFFORT, MISTRAL_REASONING_MODEL_ID},
};
use crate::llm::{
    support::{http_utils::parse_retry_after, openai_compat},
    ChatResponse, ChatWithToolsRequest, LlmError, Message, ToolDefinition,
};
use async_openai::Client;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Build chat completion request body
pub fn build_chat_completion_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let messages = prepare_chat_messages(system_prompt, history, user_message);
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": chat_temperature(model_id)
    });

    if is_reasoning_model(model_id) {
        body["reasoning_effort"] = json!(MISTRAL_REASONING_EFFORT);
    }

    body
}

/// Build tool chat request body
///
/// Maps tool call IDs to Mistral-compatible format using the provided mapper.
pub fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    id_mapper: &mut ToolCallIdMapper,
) -> Value {
    let messages = prepare_structured_messages(system_prompt, history, id_mapper);
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": if is_reasoning_model(model_id) {
            MISTRAL_REASONING_TEMPERATURE
        } else {
            MISTRAL_TOOL_TEMPERATURE
        },
        "tool_choice": "auto",
        "parallel_tool_calls": true
    });

    // Add tools array if provided
    if !tools.is_empty() {
        let mistral_tools: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    }
                })
            })
            .collect();
        body["tools"] = json!(mistral_tools);
    }

    if is_reasoning_model(model_id) {
        body["reasoning_effort"] = json!(MISTRAL_REASONING_EFFORT);
    }

    body
}

/// Check if model is a reasoning model
pub fn is_reasoning_model(model_id: &str) -> bool {
    model_id
        .trim()
        .eq_ignore_ascii_case(MISTRAL_REASONING_MODEL_ID)
}

/// Get appropriate temperature for chat
fn chat_temperature(model_id: &str) -> f32 {
    if is_reasoning_model(model_id) {
        MISTRAL_REASONING_TEMPERATURE
    } else {
        MISTRAL_CHAT_TEMPERATURE
    }
}

/// Parameters for plain chat completion.
pub struct ChatCompletionRequest<'a> {
    pub system_prompt: &'a str,
    pub history: &'a [Message],
    pub user_message: &'a str,
    pub model_id: &'a str,
    pub max_tokens: u32,
}

/// Send chat request to Mistral API
///
/// Legacy version without ID mapping. Use `send_chat_request_with_mapping` for tool calling.
pub async fn send_chat_request(
    http_client: &HttpClient,
    api_key: &str,
    body: Value,
) -> Result<ChatResponse, LlmError> {
    send_chat_request_with_mapping(
        http_client,
        api_key,
        body,
        &Arc::new(Mutex::new(ToolCallIdMapper::new())),
    )
    .await
}

/// Send chat request to Mistral API with ID mapping
///
/// Maps tool call IDs from Mistral format back to original format in the response.
pub async fn send_chat_request_with_mapping(
    http_client: &HttpClient,
    api_key: &str,
    body: Value,
    id_mapper: &Arc<Mutex<ToolCallIdMapper>>,
) -> Result<ChatResponse, LlmError> {
    let url = "https://api.mistral.ai/v1/chat/completions";

    let response = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        let status = response.status();

        // Handle 429 Too Many Requests with Retry-After support
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait_secs = parse_retry_after(response.headers());
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::RateLimit {
                wait_secs,
                message: error_text,
            });
        }

        let error_text = response.text().await.unwrap_or_default();
        return Err(LlmError::ApiError(format!(
            "Mistral API error: {status} - {error_text}"
        )));
    }

    let response_json = response
        .json::<Value>()
        .await
        .map_err(|e| LlmError::JsonError(e.to_string()))?;

    // Take lock for parsing (maps Mistral IDs back to original)
    let mapper = id_mapper.lock().expect("ID mapper lock poisoned");
    parse_chat_response(response_json, &mapper)
}

/// Chat completion implementation
pub async fn chat_completion(
    client: &Client<async_openai::config::OpenAIConfig>,
    http_client: &HttpClient,
    api_key: &str,
    request: ChatCompletionRequest<'_>,
) -> Result<String, LlmError> {
    let ChatCompletionRequest {
        system_prompt,
        history,
        user_message,
        model_id,
        max_tokens,
    } = request;

    if is_reasoning_model(model_id) {
        let body =
            build_chat_completion_body(system_prompt, history, user_message, model_id, max_tokens);
        let response = send_chat_request(http_client, api_key, body).await?;
        return response
            .content
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()));
    }

    openai_compat::chat_completion(
        client,
        system_prompt,
        history,
        user_message,
        model_id,
        max_tokens,
        MISTRAL_CHAT_TEMPERATURE,
    )
    .await
}

/// Chat with tools implementation
///
/// Maps tool call IDs to/from Mistral-compatible format using the provided mapper.
pub async fn chat_with_tools(
    http_client: &HttpClient,
    api_key: &str,
    request: ChatWithToolsRequest<'_>,
    id_mapper: &Arc<Mutex<ToolCallIdMapper>>,
) -> Result<ChatResponse, LlmError> {
    let ChatWithToolsRequest {
        system_prompt,
        messages: history,
        tools,
        model_id,
        max_tokens,
        json_mode: _,
    } = request;

    // Build request body with ID mapping (takes lock for message preparation)
    let body = {
        let mut mapper = id_mapper.lock().expect("ID mapper lock poisoned");
        build_tool_chat_body(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            &mut mapper,
        )
    };

    // Send request and parse response with ID mapping
    send_chat_request_with_mapping(http_client, api_key, body, id_mapper).await
}
