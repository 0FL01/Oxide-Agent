mod stream;

use crate::llm::{http_utils, ChatResponse, LlmError, LlmProvider, Message, ToolDefinition};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::json;
use tracing::debug;

use stream::process_zai_stream;

/// LLM provider implementation for Zai (`ZeroAI`)
pub struct ZaiProvider {
    http_client: HttpClient,
    api_key: String,
}

impl ZaiProvider {
    /// Create a new Zai provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self {
            http_client: http_utils::create_http_client(),
            api_key,
        }
    }

    fn prepare_zai_messages(system_prompt: &str, history: &[Message]) -> Vec<serde_json::Value> {
        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history {
            match msg.role.as_str() {
                "tool" => {
                    // Convert tool outputs to user messages to avoid "tool call without tools" errors
                    // since we are disabling native tools.
                    messages.push(json!({
                        "role": "user",
                        "content": format!("[Tool Output] {}", msg.content)
                    }));
                }
                "assistant" => {
                    let mut content = msg.content.clone();

                    // If we have tool calls, convert them to the expected JSON schema format
                    // because we are treating ZAI as a text-only model now.
                    if let Some(tool_calls) = &msg.tool_calls {
                        if content.trim().is_empty() {
                            // Synthesize a structured response for history
                            if let Some(first_tool) = tool_calls.first() {
                                let arguments: serde_json::Value =
                                    serde_json::from_str(&first_tool.function.arguments)
                                        .unwrap_or(json!({}));

                                let structured = json!({
                                    "thought": "Delegating to tool",
                                    "tool_call": {
                                        "name": first_tool.function.name,
                                        "arguments": arguments
                                    },
                                    "final_answer": serde_json::Value::Null
                                });
                                content = structured.to_string();
                            }
                        }
                    }

                    messages.push(json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                _ => {
                    messages.push(json!({
                        "role": msg.role,
                        "content": msg.content
                    }));
                }
            }
        }
        messages
    }

    // TODO: Временно отключено для тестов, удалить после проверки.
    #[allow(dead_code)]
    fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            })
            .collect()
    }
}

#[async_trait]
impl LlmProvider for ZaiProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        debug!(
            "ZAI: Starting chat completion request (model: {model_id}, max_tokens: {max_tokens}, history_size: {})",
            history.len()
        );

        let url = "https://api.z.ai/api/paas/v4/chat/completions";

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history {
            messages.push(json!({"role": msg.role, "content": msg.content}));
        }
        messages.push(json!({"role": "user", "content": user_message}));

        let body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            // Hardcoded to 0.95 as officially recommended by ZAI.
            // DO NOT change to f32 constant to avoid serialization issues.
            "temperature": 0.95
        });

        debug!(
            "ZAI: Sending chat request body (model: {}): {}",
            model_id,
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
        );

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://opencode.ai/")
            .header("X-Title", "opencode")
            .header(
                "User-Agent",
                "Opencode/0.1.0 (compatible; ai-sdk/openai-compatible)",
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "ZAI API error: {status} - {error_text}"
            )));
        }

        let res_json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["choices"][0]["message"]["content"]
            .as_str()
            .map(ToString::to_string)
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("ZAI_FALLBACK_TO_GEMINI".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Image analysis not supported by ZAI. GLM-4.7 is text-only.".to_string(),
        ))
    }

    /// Chat completion with tool calling support for agent mode.
    /// Supports streaming tool calls.
    ///
    /// # Errors
    ///
    /// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
    /// or `LlmError::JsonError` if parsing fails.
    async fn chat_with_tools(
        &self,
        system_prompt: &str,
        history: &[Message],
        tools: &[ToolDefinition],
        model_id: &str,
        max_tokens: u32,
        json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        use eventsource_stream::Eventsource;

        debug!(
            "ZAI: *** CHAT_WITH_TOOLS ENTRY *** model={model_id} tools_count={} history_size={} json_mode={}",
            tools.len(),
            history.len(),
            json_mode
        );

        debug!(
            "ZAI: Starting tool-enabled chat completion (model: {model_id}, tools: {}, history: {})",
            tools.len(),
            history.len()
        );

        let url = "https://api.z.ai/api/paas/v4/chat/completions";

        let messages = Self::prepare_zai_messages(system_prompt, history);

        // DISABLE NATIVE TOOLS for ZAI to prevent conflict with JSON schema in prompt.
        // We force the model to use the structured JSON output format defined in the system prompt.
        // let openai_tools = Self::prepare_tools_json(tools);
        let openai_tools: Vec<serde_json::Value> = vec![];

        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            // Hardcoded to 0.95 as officially recommended by ZAI.
            // DO NOT change to f32 constant to avoid serialization issues.
            "temperature": 0.95,
            "stream": true
        });

        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }

        // Native tools disabled
        // if !openai_tools.is_empty() {
        //     body["tools"] = json!(openai_tools);
        // }

        debug!(
            "ZAI: tools array: {}",
            if !openai_tools.is_empty() {
                "EXISTS in body"
            } else {
                "OMITTED (empty)"
            }
        );

        debug!(
            "ZAI: Sending request body (model: {}, tools_count: {}): {}",
            model_id,
            openai_tools.len(),
            serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string())
        );

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://opencode.ai/")
            .header("X-Title", "opencode")
            .header(
                "User-Agent",
                "Opencode/0.1.0 (compatible; ai-sdk/openai-compatible)",
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "ZAI API error: {status} - {error_text}"
            )));
        }

        let stream = response.bytes_stream().eventsource();
        process_zai_stream(stream).await
    }
}
