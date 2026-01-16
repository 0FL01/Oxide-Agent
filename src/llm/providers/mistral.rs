use crate::config::{MISTRAL_CHAT_TEMPERATURE, MISTRAL_TOOL_TEMPERATURE};
use crate::llm::{
    http_utils, openai_compat, ChatResponse, LlmError, LlmProvider, Message, TokenUsage,
    ToolDefinition,
};
use async_openai::{config::OpenAIConfig, Client};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::json;

#[derive(serde::Deserialize, Debug)]
struct LenientMessage {
    content: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct LenientChoice {
    message: LenientMessage,
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
struct MistralUsage {
    #[serde(rename = "prompt_tokens")]
    prompt: u32,
    #[serde(rename = "completion_tokens")]
    completion: u32,
    #[serde(rename = "total_tokens")]
    total: u32,
}

#[derive(serde::Deserialize, Debug)]
struct LenientResponse {
    choices: Vec<LenientChoice>,
    usage: Option<MistralUsage>,
}

/// LLM provider implementation for Mistral AI
pub struct MistralProvider {
    client: Client<OpenAIConfig>,
    http_client: HttpClient,
    api_key: String,
}

impl MistralProvider {
    /// Create a new Mistral provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key.clone())
            .with_api_base("https://api.mistral.ai/v1");
        Self {
            client: Client::with_config(config),
            http_client: http_utils::create_http_client(),
            api_key,
        }
    }

    fn prepare_structured_messages(
        system_prompt: &str,
        history: &[Message],
    ) -> Vec<serde_json::Value> {
        let mut messages = vec![json!({
            "role": "system",
            "content": system_prompt
        })];

        for msg in history {
            match msg.role.as_str() {
                "system" => {
                    messages.push(json!({
                        "role": "system",
                        "content": msg.content
                    }));
                }
                "assistant" => {
                    let mut content = msg.content.clone();
                    if let Some(tool_calls) = &msg.tool_calls {
                        if !tool_calls.is_empty() {
                            let tool_calls_json = json!({ "tool_calls": tool_calls });
                            let tool_calls_str =
                                serde_json::to_string(&tool_calls_json).unwrap_or_default();
                            if content.is_empty() {
                                content = tool_calls_str;
                            } else {
                                content = format!("{content}\n\n{tool_calls_str}");
                            }
                        }
                    }
                    messages.push(json!({
                        "role": "assistant",
                        "content": content
                    }));
                }
                "tool" => {
                    // [Tool Output] <content>
                    messages.push(json!({
                        "role": "user",
                        "content": format!("[Tool Output] {}", msg.content)
                    }));
                }
                _ => {
                    messages.push(json!({
                        "role": "user",
                        "content": msg.content
                    }));
                }
            }
        }
        messages
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
            MISTRAL_CHAT_TEMPERATURE,
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

    /// Chat completion with tool calling support for agent mode
    ///
    /// # Errors
    ///
    /// Returns `LlmError::NetworkError` on connectivity issues, `LlmError::ApiError` on non-success status codes,
    /// or `LlmError::JsonError` if parsing fails.
    async fn chat_with_tools(
        &self,
        system_prompt: &str,
        history: &[Message],
        _tools: &[ToolDefinition],
        model_id: &str,
        max_tokens: u32,
    ) -> Result<ChatResponse, LlmError> {
        let url = "https://api.mistral.ai/v1/chat/completions";

        let messages = Self::prepare_structured_messages(system_prompt, history);

        let body = json!({
            "model": model_id,
            "messages": messages,
            "response_format": { "type": "json_object" },
            "max_tokens": max_tokens,
            "temperature": MISTRAL_TOOL_TEMPERATURE
        });

        let response = self
            .http_client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError(format!(
                "Mistral API error: {status} - {error_text}"
            )));
        }

        let res_json: LenientResponse = response
            .json()
            .await
            .map_err(|e| LlmError::JsonError(e.to_string()))?;

        let choice = res_json
            .choices
            .first()
            .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))?;

        let content = choice.message.content.clone();
        let finish_reason = choice
            .finish_reason
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let usage = res_json.usage.as_ref().map(|u| TokenUsage {
            prompt_tokens: u.prompt,
            completion_tokens: u.completion,
            total_tokens: u.total,
        });

        Ok(ChatResponse {
            content,
            tool_calls: vec![],
            finish_reason,
            reasoning_content: None,
            usage,
        })
    }
}
