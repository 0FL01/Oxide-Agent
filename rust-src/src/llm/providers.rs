use async_trait::async_trait;
use super::{LlmError, LlmProvider, Message};
use async_openai::{
    config::OpenAIConfig,
    types::chat::{
        ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};
use reqwest::Client as HttpClient;
use serde_json::json;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

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
        let mut messages = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(system_prompt)
                .build()
                .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                .into(),
        ];

        for msg in history {
            let m = match msg.role.as_str() {
                "user" => ChatCompletionRequestUserMessageArgs::default()
                    .content(msg.content.clone())
                    .build()
                    .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                    .into(),
                _ => ChatCompletionRequestAssistantMessageArgs::default()
                    .content(msg.content.clone())
                    .build()
                    .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                    .into(),
            };
            messages.push(m);
        }

        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(user_message)
                .build()
                .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                .into(),
        );

        let request = CreateChatCompletionRequestArgs::default()
            .model(model_id)
            .messages(messages)
            .max_tokens(max_tokens)
            .temperature(0.7)
            .build()
            .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?;

        let response = self.client.chat().create(request).await
            .map_err(|e| LlmError::ApiError(e.to_string()))?;

        response.choices.first()
            .and_then(|c| c.message.content.clone())
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(&self, _audio_bytes: Vec<u8>, _mime_type: &str, _model_id: &str) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }

    async fn analyze_image(&self, _image_bytes: Vec<u8>, _text_prompt: &str, _system_prompt: &str, _model_id: &str) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Groq".to_string()))
    }
}

pub struct MistralProvider {
    client: Client<OpenAIConfig>,
}

impl MistralProvider {
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base("https://api.mistral.ai/v1");
        Self {
            client: Client::with_config(config),
        }
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
        // Implementation is identical to Groq due to OpenAI compatibility
        let mut messages = vec![
            ChatCompletionRequestSystemMessageArgs::default()
                .content(system_prompt)
                .build()
                .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                .into(),
        ];

        for msg in history {
            let m = match msg.role.as_str() {
                "user" => ChatCompletionRequestUserMessageArgs::default()
                    .content(msg.content.clone())
                    .build()
                    .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                    .into(),
                _ => ChatCompletionRequestAssistantMessageArgs::default()
                    .content(msg.content.clone())
                    .build()
                    .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                    .into(),
            };
            messages.push(m);
        }

        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(user_message)
                .build()
                .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?
                .into(),
        );

        let request = CreateChatCompletionRequestArgs::default()
            .model(model_id)
            .messages(messages)
            .max_tokens(max_tokens)
            .temperature(0.9)
            .build()
            .map_err(|e: async_openai::error::OpenAIError| LlmError::Unknown(e.to_string()))?;

        let response = self.client.chat().create(request).await
            .map_err(|e| LlmError::ApiError(e.to_string()))?;

        response.choices.first()
            .and_then(|c| c.message.content.clone())
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))
    }

    async fn transcribe_audio(&self, _audio_bytes: Vec<u8>, _mime_type: &str, _model_id: &str) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
    }

    async fn analyze_image(&self, _image_bytes: Vec<u8>, _text_prompt: &str, _system_prompt: &str, _model_id: &str) -> Result<String, LlmError> {
        Err(LlmError::Unknown("Not implemented for Mistral".to_string()))
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

        let response = self.http_client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!("Gemini API error: {} - {}", status, error_text)));
        }

        let res_json: serde_json::Value = response.json().await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError(format!("Invalid response format: {:?}", res_json)))
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

        let response = self.http_client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!("Gemini transcription error: {} - {}", status, error_text)));
        }

        let res_json: serde_json::Value = response.json().await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError("Failed to get transcription".to_string()))
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

        let response = self.http_client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!("Gemini vision error: {} - {}", status, error_text)));
        }

        let res_json: serde_json::Value = response.json().await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError("Failed to get vision analysis".to_string()))
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

        let mut request = self.http_client.post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json");

        if !self.site_url.is_empty() {
            request = request.header("HTTP-Referer", &self.site_url);
        }
        if !self.site_name.is_empty() {
            request = request.header("X-Title", &self.site_name);
        }

        let response = request.json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!("OpenRouter API error: {} - {}", status, error_text)));
        }

        let res_json: serde_json::Value = response.json().await
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
                                "format": "wav" // Default to wav as in Python
                            }
                        }
                    ]
                }
            ],
            "max_tokens": 8000,
            "temperature": 0.4
        });

        let response = self.http_client.post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(LlmError::ApiError(format!("OpenRouter transcription error: {}", response.status())));
        }

        let res_json: serde_json::Value = response.json().await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError("Failed to get transcription".to_string()))
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

        let response = self.http_client.post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(LlmError::ApiError(format!("OpenRouter vision error: {}", response.status())));
        }

        let res_json: serde_json::Value = response.json().await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        res_json["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| LlmError::ApiError("Failed to get vision analysis".to_string()))
    }
}
