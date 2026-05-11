mod helpers;

use crate::config::{
    OPENROUTER_AUDIO_TRANSCRIBE_PROMPT, OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE,
    OPENROUTER_CHAT_TEMPERATURE, OPENROUTER_IMAGE_TEMPERATURE,
};
use crate::llm::support::http::{extract_text_content, send_json_request};
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::Client as HttpClient;
use serde_json::json;

use helpers::{parse_tool_calls, prepare_structured_messages, prepare_tools_json};

/// Hardcoded OpenRouter app attribution headers
const OPENROUTER_HEADERS: [(&str, &str); 3] = [
    ("HTTP-Referer", "https://github.com/0FL01/Oxide-Agent"),
    ("X-Title", "Oxide Agent"),
    ("X-OpenRouter-Title", "Oxide Agent"),
];

/// LLM provider implementation for `OpenRouter`
pub struct OpenRouterProvider {
    http_client: HttpClient,
    api_key: String,
    // Deprecated: App attribution headers are now hardcoded
    #[allow(dead_code)]
    site_url: String,
    #[allow(dead_code)]
    site_name: String,
}

impl OpenRouterProvider {
    /// Create a new `OpenRouter` provider instance
    #[must_use]
    pub fn new(api_key: String, site_url: String, site_name: String) -> Self {
        Self {
            http_client: crate::llm::support::http::create_http_client(),
            api_key,
            site_url,
            site_name,
        }
    }

    /// Create a new `OpenRouter` provider with a shared HTTP client
    ///
    /// This allows connection reuse across multiple providers,
    /// significantly reducing latency for sequential requests.
    #[must_use]
    pub fn new_with_client(
        api_key: String,
        site_url: String,
        site_name: String,
        http_client: HttpClient,
    ) -> Self {
        Self {
            http_client,
            api_key,
            site_url,
            site_name,
        }
    }

    fn data_url(mime_type: &str, bytes: &[u8]) -> String {
        format!("data:{mime_type};base64,{}", BASE64.encode(bytes))
    }

    fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
        if image_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
            return "image/png";
        }

        if image_bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return "image/jpeg";
        }

        if image_bytes.starts_with(b"GIF87a") || image_bytes.starts_with(b"GIF89a") {
            return "image/gif";
        }

        if image_bytes.starts_with(b"RIFF") && image_bytes.get(8..12) == Some(b"WEBP") {
            return "image/webp";
        }

        "image/jpeg"
    }

    fn audio_input_format(mime_type: &str) -> &'static str {
        let normalized = mime_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();

        match normalized.as_str() {
            "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/ogg" | "audio/opus" | "audio/vorbis" => "ogg",
            "audio/flac" => "flac",
            "audio/mp4" | "audio/x-m4a" => "m4a",
            "audio/webm" => "webm",
            _ => "wav",
        }
    }

    fn build_video_request_body(
        model_id: &str,
        video_bytes: &[u8],
        mime_type: &str,
        text_prompt: &str,
        system_prompt: &str,
    ) -> serde_json::Value {
        let data_url = Self::data_url(mime_type, video_bytes);

        json!({
            "model": model_id,
            "messages": [
                {"role": "system", "content": system_prompt},
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": text_prompt},
                        {
                            "type": "video_url",
                            "video_url": {"url": data_url}
                        }
                    ]
                }
            ],
            "max_tokens": 4000,
            "temperature": OPENROUTER_IMAGE_TEMPERATURE
        })
    }
}

/// Parse OpenRouter rate limit reset time from error body.
///
/// OpenRouter returns rate limit info in the error body metadata:
/// ```json
/// {
///   "error": {
///     "message": "...",
///     "code": 429,
///     "metadata": {
///       "headers": {
///         "X-RateLimit-Reset": "1741305600000"  // milliseconds since epoch
///       }
///     }
///   }
/// }
/// ```
///
/// Returns seconds to wait, or None if parsing fails.
pub fn parse_openrouter_rate_limit(body: &str) -> Option<u64> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let reset_ms = json
        .pointer("/error/metadata/headers/X-RateLimit-Reset")?
        .as_str()?
        .parse::<i64>()
        .ok()?;

    let now_ms = chrono::Utc::now().timestamp_millis();
    let wait_secs = (reset_ms - now_ms) / 1000;

    if wait_secs > 0 {
        Some(wait_secs as u64)
    } else {
        None
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

        // Hardcoded app attribution headers for OpenRouter identification
        for (key, value) in &OPENROUTER_HEADERS {
            request = request.header(*key, *value);
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();

            // Handle 429 Too Many Requests
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let error_text = response.text().await.unwrap_or_default();
                let wait_secs = parse_openrouter_rate_limit(&error_text);
                return Err(LlmError::RateLimit {
                    wait_secs,
                    message: error_text,
                });
            }

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
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        self.transcribe_audio_with_prompt(
            audio_bytes,
            mime_type,
            OPENROUTER_AUDIO_TRANSCRIBE_PROMPT,
            model_id,
        )
        .await
    }

    async fn transcribe_audio_with_prompt(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let audio_base64 = BASE64.encode(&audio_bytes);
        let audio_format = Self::audio_input_format(mime_type);

        let body = json!({
            "model": model_id,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": text_prompt},
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": audio_base64,
                                "format": audio_format
                            }
                        }
                    ]
                }
            ],
            "max_tokens": 8000,
            "temperature": OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE
        });

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(
            &self.http_client,
            url,
            &body,
            Some(&auth),
            &OPENROUTER_HEADERS,
        )
        .await?;
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
        let data_url = Self::data_url(Self::infer_image_mime_type(&image_bytes), &image_bytes);

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
        let res_json = send_json_request(
            &self.http_client,
            url,
            &body,
            Some(&auth),
            &OPENROUTER_HEADERS,
        )
        .await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn analyze_video(
        &self,
        video_bytes: Vec<u8>,
        mime_type: &str,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let url = "https://openrouter.ai/api/v1/chat/completions";
        let body = Self::build_video_request_body(
            model_id,
            &video_bytes,
            mime_type,
            text_prompt,
            system_prompt,
        );

        let auth = format!("Bearer {}", self.api_key);
        let res_json = send_json_request(
            &self.http_client,
            url,
            &body,
            Some(&auth),
            &OPENROUTER_HEADERS,
        )
        .await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let ChatWithToolsRequest {
            system_prompt,
            messages: history,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode: _,
        } = request;
        let url = "https://openrouter.ai/api/v1/chat/completions";

        let messages = prepare_structured_messages(system_prompt, history);
        let openai_tools = prepare_tools_json(tools);

        let mut body = json!({
            "model": model_id,
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature.unwrap_or(OPENROUTER_CHAT_TEMPERATURE)
        });

        if !openai_tools.is_empty() {
            body["tools"] = json!(openai_tools);
        }

        // Hardcoded app attribution headers for OpenRouter identification
        let extra_headers: Vec<(&str, &str)> = OPENROUTER_HEADERS.to_vec();

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
            Some(value) if value.is_array() => parse_tool_calls(value)?,
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

#[cfg(test)]
mod tests {
    use super::OpenRouterProvider;
    use base64::Engine;
    use serde_json::json;

    #[test]
    fn build_video_request_body_uses_video_url_data_part() {
        let body = OpenRouterProvider::build_video_request_body(
            "google/gemini-3.1-flash-lite-preview",
            b"video-bytes",
            "video/mp4",
            "Describe this clip",
            "System",
        );

        assert_eq!(body["model"], json!("google/gemini-3.1-flash-lite-preview"));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["messages"][0]["content"], json!("System"));
        assert_eq!(body["messages"][1]["content"][0]["type"], json!("text"));
        assert_eq!(
            body["messages"][1]["content"][1]["type"],
            json!("video_url")
        );
        assert_eq!(
            body["messages"][1]["content"][1]["video_url"]["url"],
            json!("data:video/mp4;base64,dmlkZW8tYnl0ZXM=")
        );
    }

    #[test]
    fn audio_transcription_prompt_is_embedded_in_request() {
        let audio_base64 = base64::prelude::BASE64_STANDARD.encode(b"audio-bytes");
        let body = json!({
            "model": "google/gemini-3.1-flash-lite-preview",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Extract timestamps and speakers"},
                        {
                            "type": "input_audio",
                            "input_audio": {
                                "data": audio_base64,
                                "format": "wav"
                            }
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            json!("Extract timestamps and speakers")
        );
    }

    #[test]
    fn audio_input_format_tracks_common_mime_types() {
        assert_eq!(OpenRouterProvider::audio_input_format("audio/wav"), "wav");
        assert_eq!(OpenRouterProvider::audio_input_format("audio/mpeg"), "mp3");
        assert_eq!(OpenRouterProvider::audio_input_format("audio/ogg"), "ogg");
        assert_eq!(OpenRouterProvider::audio_input_format("audio/flac"), "flac");
        assert_eq!(
            OpenRouterProvider::audio_input_format("audio/wav; codecs=1"),
            "wav"
        );
        assert_eq!(OpenRouterProvider::audio_input_format("unknown"), "wav");
    }

    #[test]
    fn infer_image_mime_type_from_magic_bytes() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0x00];
        let jpeg = [0xFF, 0xD8, 0xFF, 0xDB];
        let gif = *b"GIF89a";
        let webp = [b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];
        let unknown = [0x00, 0x11, 0x22, 0x33];

        assert_eq!(OpenRouterProvider::infer_image_mime_type(&png), "image/png");
        assert_eq!(
            OpenRouterProvider::infer_image_mime_type(&jpeg),
            "image/jpeg"
        );
        assert_eq!(OpenRouterProvider::infer_image_mime_type(&gif), "image/gif");
        assert_eq!(
            OpenRouterProvider::infer_image_mime_type(&webp),
            "image/webp"
        );
        assert_eq!(
            OpenRouterProvider::infer_image_mime_type(&unknown),
            "image/jpeg"
        );
    }
}
