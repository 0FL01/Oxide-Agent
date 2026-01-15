mod stream;

use crate::config::ZAI_CHAT_TEMPERATURE;
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
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": msg.tool_call_id,
                        "content": msg.content
                    }));
                }
                "assistant" => {
                    let mut m = json!({
                        "role": "assistant",
                        "content": msg.content
                    });

                    // If we have tool calls, include them
                    if let Some(tool_calls) = &msg.tool_calls {
                        let api_tool_calls: Vec<serde_json::Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.function.name,
                                        "arguments": tc.function.arguments
                                    }
                                })
                            })
                            .collect();

                        m["tool_calls"] = json!(api_tool_calls);
                    }

                    messages.push(m);
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

    async fn send_zai_request(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, LlmError> {
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
            .json(body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();

            // Handle 429 Too Many Requests specifically
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait_secs = crate::llm::http_utils::parse_retry_after(response.headers());
                let error_text = response.text().await.unwrap_or_default();
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: error_text,
                });
            }

            let error_text = response.text().await.unwrap_or_default();

            // Detect HTML error pages from Nginx/proxies
            let is_html = error_text.trim_start().starts_with("<!DOCTYPE")
                || error_text.trim_start().starts_with("<html")
                || error_text.trim_start().starts_with("<HTML");

            let clean_message = if is_html {
                // Don't include raw HTML in error message
                format!("ZAI API error: {status} (Server returned HTML error page)")
            } else {
                // Truncate very long error messages to avoid token bloat
                let truncated = if error_text.len() > 500 {
                    format!("{}... (truncated)", &error_text[..500])
                } else {
                    error_text
                };
                format!("ZAI API error: {status} - {truncated}")
            };

            return Err(LlmError::ApiError(clean_message));
        }

        Ok(response)
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
            "temperature": ZAI_CHAT_TEMPERATURE
        });

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
    ) -> Result<ChatResponse, LlmError> {
        use eventsource_stream::Eventsource;

        debug!(
            "ZAI: Starting tool-enabled chat completion (model: {model_id}, tools: {}, history: {})",
            tools.len(),
            history.len()
        );

        let url = "https://api.z.ai/api/paas/v4/chat/completions";

        // Prepare messages and tools
        let messages = Self::prepare_zai_messages(system_prompt, history);
        let openai_tools = Self::prepare_tools_json(tools);

        let body = json!({
            "model": model_id,
            "messages": messages,
            "tools": openai_tools,
            "max_tokens": max_tokens,
            "temperature": ZAI_CHAT_TEMPERATURE,
            "stream": true
        });

        let response = self.send_zai_request(url, &body).await?;

        // Process streaming response
        let stream = response.bytes_stream().eventsource();
        process_zai_stream(stream).await
    }
}
