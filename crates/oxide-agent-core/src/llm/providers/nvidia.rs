use crate::config::NVIDIA_CHAT_TEMPERATURE;
use crate::llm::http_utils::{extract_text_content, send_json_request};
use crate::llm::{LlmError, LlmProvider, Message};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::json;

pub struct NvidiaProvider {
    http_client: HttpClient,
    api_key: String,
    api_base: String,
}

impl NvidiaProvider {
    #[must_use]
    pub fn new(api_key: String, api_base: String) -> Self {
        Self {
            http_client: crate::llm::http_utils::create_http_client(),
            api_key,
            api_base,
        }
    }

    #[must_use]
    pub fn new_with_client(api_key: String, api_base: String, http_client: HttpClient) -> Self {
        Self {
            http_client,
            api_key,
            api_base,
        }
    }

    fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.api_base.trim_end_matches('/'))
    }
}

#[async_trait]
impl LlmProvider for NvidiaProvider {
    async fn chat_completion(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let url = self.chat_completions_url();

        let mut messages = vec![json!({"role": "system", "content": system_prompt})];
        for msg in history {
            messages.push(json!({"role": msg.role, "content": msg.content}));
        }
        messages.push(json!({"role": "user", "content": user_message}));

        let body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": NVIDIA_CHAT_TEMPERATURE,
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(&self.http_client, &url, &body, Some(&auth), &[]).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "Audio transcription not supported by NVIDIA NIM provider".to_string(),
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
            "Image analysis not supported by NVIDIA NIM provider".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::NvidiaProvider;

    #[test]
    fn trims_trailing_slash_when_building_chat_url() {
        let provider = NvidiaProvider::new(
            "test-key".to_string(),
            "https://integrate.api.nvidia.com/v1/".to_string(),
        );

        assert_eq!(
            provider.chat_completions_url(),
            "https://integrate.api.nvidia.com/v1/chat/completions"
        );
    }
}
