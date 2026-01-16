use crate::config::{
    GEMINI_AUDIO_TRANSCRIBE_PROMPT, GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE, GEMINI_CHAT_TEMPERATURE,
    GEMINI_IMAGE_TEMPERATURE,
};
use crate::llm::http_utils::{extract_text_content, send_json_request};
use crate::llm::{LlmError, LlmProvider, Message};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client as HttpClient;
use serde_json::json;

/// LLM provider implementation for Google Gemini
pub struct GeminiProvider {
    http_client: HttpClient,
    api_key: String,
}

impl GeminiProvider {
    /// Create a new Gemini provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self {
            http_client: crate::llm::http_utils::create_http_client(),
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
            "https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent?key={}",
            self.api_key
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
                "temperature": GEMINI_CHAT_TEMPERATURE,
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
            "https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent?key={}",
            self.api_key
        );

        let body = json!({
            "contents": [{
                "parts": [
                    {"text": GEMINI_AUDIO_TRANSCRIBE_PROMPT},
                    {
                        "inline_data": {
                            "mime_type": mime_type,
                            "data": BASE64.encode(&audio_bytes)
                        }
                    }
                ]
            }],
            "generationConfig": {
                "temperature": GEMINI_AUDIO_TRANSCRIBE_TEMPERATURE
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
            "https://generativelanguage.googleapis.com/v1beta/models/{model_id}:generateContent?key={}",
            self.api_key
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
                "temperature": GEMINI_IMAGE_TEMPERATURE,
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
