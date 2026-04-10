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
use reqwest::Client as HttpClient;
use serde_json::{json, Value};
use std::path::PathBuf;

const CHATGPT_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

/// ChatGPT headless OAuth provider.
#[derive(Debug, Clone)]
pub struct ChatGptProvider {
    http_client: HttpClient,
    auth: ChatGptAuthManager,
}

impl ChatGptProvider {
    #[must_use]
    pub fn new(auth_path: impl Into<PathBuf>) -> Self {
        let http_client = create_http_client();
        Self::new_with_client(auth_path, http_client)
    }

    #[must_use]
    pub fn new_with_client(auth_path: impl Into<PathBuf>, http_client: HttpClient) -> Self {
        let auth = ChatGptAuthManager::new(auth_path.into(), http_client.clone());
        Self { http_client, auth }
    }

    async fn chat_request(&self, body: Value) -> Result<Value, LlmError> {
        let session = self.auth.get_valid_session().await?;

        let mut request = self
            .http_client
            .post(CHATGPT_CODEX_API_ENDPOINT)
            .header("Authorization", format!("Bearer {}", session.access_token))
            .header("Content-Type", "application/json");

        if !session.account_id.is_empty() {
            request = request.header("ChatGPT-Account-Id", session.account_id);
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| LlmError::NetworkError(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait_secs = parse_retry_after(response.headers());
                let body = response.text().await.unwrap_or_default();
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: body,
                });
            }

            let body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "ChatGPT API error: {status} - {body}"
            )));
        }

        response
            .json()
            .await
            .map_err(|error| LlmError::JsonError(error.to_string()))
    }
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
        let mut messages = prepare_structured_messages(system_prompt, history);
        messages.push(json!({
            "role": "user",
            "content": user_message,
        }));

        let body = build_chat_request_body(messages, &[], model_id, max_tokens, None, false);
        let response = self.chat_request(body).await?;

        response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
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

        let body = build_chat_request_body(
            prepare_structured_messages(system_prompt, messages),
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
        );
        let response = self.chat_request(body).await?;

        let content = response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string);

        let tool_calls_value = response
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"));
        let tool_calls = match tool_calls_value {
            Some(value) if value.is_null() => Vec::new(),
            Some(value) if value.is_array() => parse_tool_calls(value)?,
            Some(_) => {
                return Err(LlmError::JsonError(
                    "Invalid tool_calls format from ChatGPT OAuth".to_string(),
                ))
            }
            None => Vec::new(),
        };

        if content.is_none() && tool_calls.is_empty() {
            return Err(LlmError::ApiError("Empty response".to_string()));
        }

        let finish_reason = response["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content: None,
            usage: response.get("usage").and_then(parse_usage),
        })
    }
}

fn build_chat_request_body(
    messages: Vec<Value>,
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "stream": false,
    });

    if let Some(temperature) = temperature {
        body["temperature"] = json!(temperature);
    }

    if !tools.is_empty() {
        body["tools"] = json!(prepare_tools_json(tools));
        body["tool_choice"] = json!("auto");
    }

    if json_mode && tools.is_empty() {
        body["response_format"] = json!({ "type": "json_object" });
    }

    if model_id.starts_with("gpt-5") {
        body["reasoning"] = json!({ "effort": "medium" });
    }

    body
}

fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let mut messages = vec![json!({
        "role": "system",
        "content": system_prompt,
    })];

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                messages.push(json!({
                    "role": "system",
                    "content": msg.content,
                }));
            }
            "assistant" => {
                let mut message = json!({
                    "role": "assistant",
                    "content": msg.content,
                });

                if let Some(tool_calls) = &msg.tool_calls {
                    let api_tool_calls: Vec<Value> = tool_calls
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
                                        }
                                    })
                                })
                        })
                        .collect();

                    if !api_tool_calls.is_empty() {
                        message["tool_calls"] = json!(api_tool_calls);
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
                }
            })
        })
        .collect()
}

fn parse_tool_calls(value: &Value) -> Result<Vec<ToolCall>, LlmError> {
    let Some(array) = value.as_array() else {
        return Err(LlmError::JsonError(
            "Invalid tool_calls format from ChatGPT OAuth".to_string(),
        ));
    };

    let mut tool_calls = Vec::with_capacity(array.len());
    for call in array {
        let Some(function) = call.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let arguments = function
            .get("arguments")
            .and_then(|value| {
                value
                    .as_str()
                    .map(ToString::to_string)
                    .or_else(|| serde_json::to_string(value).ok())
            })
            .unwrap_or_default();
        let wire_id = call
            .get("id")
            .and_then(|value| value.as_str())
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

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value.get("prompt_tokens")?.as_u64()? as u32,
        completion_tokens: value.get("completion_tokens")?.as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{build_chat_request_body, parse_tool_calls, prepare_structured_messages};
    use crate::llm::{Message, ToolCall, ToolCallCorrelation, ToolCallFunction};
    use serde_json::json;

    #[test]
    fn json_mode_uses_json_object_without_tools() {
        let body = build_chat_request_body(
            vec![json!({"role":"user","content":"hi"})],
            &[],
            "gpt-5.4",
            10,
            None,
            true,
        );

        assert_eq!(body["response_format"]["type"], json!("json_object"));
        assert_eq!(body["reasoning"]["effort"], json!("medium"));
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

        let messages = prepare_structured_messages("system", &history);

        assert_eq!(messages[1]["tool_calls"][0]["id"], json!("call-chatgpt-1"));
        assert_eq!(messages[2]["tool_call_id"], json!("call-chatgpt-1"));
    }

    #[test]
    fn parse_tool_calls_preserves_provider_ids() {
        let tool_calls = parse_tool_calls(&json!([
            {
                "id": "call-chatgpt-2",
                "type": "function",
                "function": {
                    "name": "search",
                    "arguments": "{\"query\":\"oxide\"}"
                }
            }
        ]))
        .expect("tool calls parse");

        assert_ne!(tool_calls[0].invocation_id().as_str(), "call-chatgpt-2");
        assert_eq!(tool_calls[0].wire_tool_call_id(), "call-chatgpt-2");
    }
}
