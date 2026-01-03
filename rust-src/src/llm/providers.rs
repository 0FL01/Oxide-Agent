use super::http_utils::{extract_text_content, send_json_request};
use super::openai_compat;
use super::{LlmError, LlmProvider, Message};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client as HttpClient;
use serde_json::json;
use tracing::debug;

pub struct GroqProvider {
    client: Client<OpenAIConfig>,
}

impl GroqProvider {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base("https://api.groq.com/openai/v1");
        Self {
            client: Client::with_config(config),
        }
    }
}

#[async_trait]
impl LlmProvider for GroqProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            0.7,
        )
        .await
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }
}

pub struct MistralProvider {
    client: Client<OpenAIConfig>,
    http_client: HttpClient,
    api_key: String,
}

impl MistralProvider {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key.clone())
            .with_api_base("https://api.mistral.ai/v1");
        Self {
            client: Client::with_config(config),
            http_client: HttpClient::new(),
            api_key,
        }
    }

    /// Chat completion with tool calling support for agent mode
    pub async fn chat_with_tools(
        &self,
        system_prompt: &str,
        history: &[super::Message],
        tools: &[super::ToolDefinition],
        model_id: &str,
        max_tokens: u32,
    ) -> Result<super::ChatResponse, super::LlmError> {
        // Manually implement request to handle missing "type" field in tool_calls
        // which async-openai strictly requires but Mistral/OpenRouter might omit.

        let url = "https://api.mistral.ai/v1/chat/completions";

        let mut messages = vec![json!({
            "role": "system",
            "content": system_prompt
        })];

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
                        // Map internal ToolCall to the structure expected by API (if needed)
                        // Our ToolCall struct matches what we want to send usually,
                        // but we need to make sure it has "type": "function" if Mistral requires it.
                        // Mistral API usually expects:
                        // "tool_calls": [ { "id": "...", "type": "function", "function": { "name": "...", "arguments": "..." } } ]

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
                    let m = json!({
                        "role": msg.role,
                        "content": msg.content
                    });
                    messages.push(m);
                }
            }
        }

        // Add tool definitions
        let openai_tools: Vec<serde_json::Value> = tools
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
            .collect();

        let body = json!({
            "model": model_id,
            "messages": messages,
            "tools": openai_tools,
            "max_tokens": max_tokens,
            "temperature": 0.7
        });

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| super::LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(super::LlmError::ApiError(format!(
                "Mistral API error: {} - {}",
                status, error_text
            )));
        }

        // Lenient parsing logic
        #[derive(serde::Deserialize, Debug)]
        struct LenientToolCallFunction {
            name: String,
            arguments: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct LenientToolCall {
            id: String,
            #[serde(rename = "type")]
            _type: Option<String>, // We don't care if it's missing
            function: LenientToolCallFunction,
        }

        #[derive(serde::Deserialize, Debug)]
        struct LenientMessage {
            content: Option<String>,
            tool_calls: Option<Vec<LenientToolCall>>,
        }

        #[derive(serde::Deserialize, Debug)]
        struct LenientChoice {
            message: LenientMessage,
            finish_reason: Option<String>,
        }

        #[derive(serde::Deserialize, Debug)]
        struct LenientResponse {
            choices: Vec<LenientChoice>,
        }

        let res_json: LenientResponse = response
            .json()
            .await
            .map_err(|e| super::LlmError::JsonError(e.to_string()))?;

        let choice = res_json
            .choices
            .first()
            .ok_or_else(|| super::LlmError::ApiError("Empty response".to_string()))?;

        let content = choice.message.content.clone();
        let finish_reason = choice
            .finish_reason
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let tool_calls = choice
            .message
            .tool_calls
            .as_ref()
            .map(|calls| {
                calls
                    .iter()
                    .map(|tc| super::ToolCall {
                        id: tc.id.clone(),
                        function: super::ToolCallFunction {
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(super::ChatResponse {
            content,
            tool_calls,
            finish_reason,
        })
    }
}

#[async_trait]
impl LlmProvider for MistralProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            0.9,
        )
        .await
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
    }
}

pub struct ZaiProvider {
    client: Client<OpenAIConfig>,
}

impl ZaiProvider {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base("https://api.z.ai/api/paas/v4");
        Self {
            client: Client::with_config(config),
        }
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
            "ZAI: Starting chat completion request (model: {}, max_tokens: {}, history_size: {})",
            model_id,
            max_tokens,
            history.len()
        );
        openai_compat::chat_completion(
            &self.client,
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            0.95,
        )
        .await
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
}

pub struct GeminiProvider {
    http_client: HttpClient,
    api_key: String,
}

impl GeminiProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            http_client: HttpClient::new(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_id, self.api_key
        );

        let mut contents = Vec::new();
        for msg in history {
            if msg.role != "system" {
                let role = if msg.role == "user" { "user" } else { "model" };
                contents.push(json!({
                    "role": role,
                    "parts": [{"text": msg.content}]
                }));
            }
        }
        contents.push(json!({
            "role": "user",
            "parts": [{"text": user_message}]
        }));

        let body = json!({
            "contents": contents,
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "generationConfig": {
                "temperature": 1.0,
                "maxOutputTokens": max_tokens
            },
            "safetySettings": [
                {"category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_NONE"},
                {"category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE"},
                {"category": "HARM_CATEGORY_SEXUALLY_EXPLICIT", "threshold": "BLOCK_NONE"},
                {"category": "HARM_CATEGORY_DANGEROUS_CONTENT", "threshold": "BLOCK_NONE"}
            ]
        });

        let res_json = send_json_request(&self.http_client, &url, &body, None, &[]).await?;
        extract_text_content(
            &res_json,
            &["candidates", "0", "content", "parts", "0", "text"],
        )
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_id, self.api_key
        );

        let prompt = "Сделай точную транскрипцию речи из этого аудио/видео файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это.";

        let body = json!({
            "contents": [{
                "parts": [
                    {"text": prompt},
                    {
                        "inline_data": {
                            "mime_type": mime_type,
                            "data": BASE64.encode(&audio_bytes)
                        }
                    }
                ]
            }],
            "generationConfig": {
                "temperature": 0.4
            }
        });

        let res_json = send_json_request(&self.http_client, &url, &body, None, &[]).await?;
        extract_text_content(
            &res_json,
            &["candidates", "0", "content", "parts", "0", "text"],
        )
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            model_id, self.api_key
        );

        let body = json!({
            "contents": [{
                "parts": [
                    {"text": text_prompt},
                    {
                        "inline_data": {
                            "mime_type": "image/jpeg",
                            "data": BASE64.encode(&image_bytes)
                        }
                    }
                ]
            }],
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 4000
            }
        });

        let res_json = send_json_request(&self.http_client, &url, &body, None, &[]).await?;
        extract_text_content(
            &res_json,
            &["candidates", "0", "content", "parts", "0", "text"],
        )
    }
}

pub struct OpenRouterProvider {
    http_client: HttpClient,
    api_key: String,
    site_url: String,
    site_name: String,
}

impl OpenRouterProvider {
    pub fn new(api_key: String, site_url: String, site_name: String) -> Self {
        Self {
            http_client: HttpClient::new(),
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
            "temperature": 0.7
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
                "OpenRouter API error: {} - {}",
                status, error_text
            )));
        }

        let res_json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        _mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let prompt = "Сделай точную транскрипцию речи из этого аудио файла на русском языке. Если в файле нет речи, язык не русский или файл не содержит аудиодорожку, укажи это.";
        let audio_base64 = BASE64.encode(&audio_bytes);

        let body = json!({
            "model": model_id,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": prompt},
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
            "temperature": 0.4
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
        let data_url = format!("data:image/jpeg;base64,{}", image_base64);

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
            "temperature": 0.7
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, url, &body, Some(&auth), &[]).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }
}
