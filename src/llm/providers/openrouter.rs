mod helpers;

use crate::config::{
    OPENROUTER_AUDIO_TRANSCRIBE_PROMPT, OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE,
    OPENROUTER_CHAT_TEMPERATURE, OPENROUTER_IMAGE_TEMPERATURE,
};
use crate::llm::http_utils::{extract_text_content, send_json_request};
use crate::llm::{ChatResponse, LlmError, LlmProvider, Message, TokenUsage, ToolDefinition};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client as HttpClient;
use serde_json::json;

use helpers::{prepare_structured_messages, prepare_tools_json};

/// LLM provider implementation for `OpenRouter`
pub struct OpenRouterProvider {
    http_client: HttpClient,
    api_key: String,
    site_url: String,
    site_name: String,
}

impl OpenRouterProvider {
    /// Create a new `OpenRouter` provider instance
    #[must_use]
    pub fn new(api_key: String, site_url: String, site_name: String) -> Self {
        Self {
            http_client: crate::llm::http_utils::create_http_client(),
            api_key,
            site_url,
            site_name,
        }
    }
}

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history {
            messages.push(json!({"role": msg.role, "content": msg.content}));
        }
        messages.push(json!({"role": "user", "content": user_message}));

        let body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": OPENROUTER_CHAT_TEMPERATURE
        });

        let mut request = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        if !self.site_url.is_empty() {
            request = request.header("HTTP-Referer", &self.site_url);
        }
        if !self.site_name.is_empty() {
            request = request.header("X-Title", &self.site_name);
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "OpenRouter API error: {status} - {error_text}"
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
        audio_bytes: Vec<u8>,
        _mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let audio_base64 = BASE64.encode(&audio_bytes);

        let body = json!({
            "model": model_id,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": OPENROUTER_AUDIO_TRANSCRIBE_PROMPT},
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": audio_base64,
                                "format": "wav"
                            }
                        }
                    ]
                }
            ],
            "max_tokens": 8000,
            "temperature": OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, url, &body, Some(&auth), &[]).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let image_base64 = BASE64.encode(&image_bytes);
        let data_url = format!("data:image/jpeg;base64,{image_base64}");

        let body = json!({
            "model": model_id,
            "messages": [
                {"role": "system", "content": system_prompt},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": text_prompt},
                        {
                            "type": "image_url",
                            "image_url": {"url": data_url}
                        }
                    ]
                }
            ],
            "max_tokens": 4000,
            "temperature": OPENROUTER_IMAGE_TEMPERATURE
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, url, &body, Some(&auth), &[]).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn chat_with_tools(
        &self,
        system_prompt: &str,
        history: &[Message],
        tools: &[ToolDefinition],
        model_id: &str,
        max_tokens: u32,
    ) -> Result<ChatResponse, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";

        let messages = prepare_structured_messages(system_prompt, history);
        let openai_tools = prepare_tools_json(tools);

        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": OPENROUTER_CHAT_TEMPERATURE
        });

        if !openai_tools.is_empty() {
            body["tools"] = json!(openai_tools);
        }

        let mut extra_headers = Vec::new();
        if !self.site_url.is_empty() {
            extra_headers.push(("HTTP-Referer", self.site_url.as_str()));
        }
        if !self.site_name.is_empty() {
            extra_headers.push(("X-Title", self.site_name.as_str()));
        }

        let auth = format!("Bearer {}", self.api_key);
        let res_json =
            send_json_request(&self.http_client, url, &body, Some(&auth), &extra_headers).await?;

        let content = res_json
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string);

        let tool_calls_value = res_json
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("tool_calls"));

        let tool_calls = match tool_calls_value {
            Some(value) if value.is_null() => Vec::new(),
            Some(value) if value.is_array() => serde_json::from_value(value.clone())
                .map_err(|e| LlmError::JsonError(e.to_string()))?,
            Some(_) => {
                return Err(LlmError::JsonError(
                    "Invalid tool_calls format from OpenRouter".to_string(),
                ))
            }
            None => Vec::new(),
        };

        if content.is_none() && tool_calls.is_empty() {
            return Err(LlmError::ApiError("Empty response".to_string()));
        }

        let finish_reason = res_json["choices"][0]["finish_reason"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        let usage = res_json.get("usage").and_then(|u| {
            Some(TokenUsage {
                prompt_tokens: u.get("prompt_tokens")?.as_u64()? as u32,
                completion_tokens: u.get("completion_tokens")?.as_u64()? as u32,
                total_tokens: u.get("total_tokens")?.as_u64()? as u32,
            })
        });

        Ok(ChatResponse {
            content,
            tool_calls,
            finish_reason,
            reasoning_content: None,
            usage,
        })
    }
}
