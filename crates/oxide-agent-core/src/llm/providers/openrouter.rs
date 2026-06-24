#[cfg(test)]
mod helpers;
pub(crate) mod module;

pub(crate) use module::OpenRouterProviderModule;

use crate::config::{
    OPENROUTER_AUDIO_TRANSCRIBE_PROMPT, OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE,
    OPENROUTER_IMAGE_TEMPERATURE,
};
use crate::llm::providers::chat_completions::client::ChatCompletionsClient;
use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;
use crate::llm::providers::chat_completions::request::{self as chat_request, ChatRequestOptions};
use crate::llm::providers::chat_completions::response as chat_response;
use crate::llm::support::http::extract_text_content;
#[cfg(test)]
use crate::llm::support::media;
use crate::llm::{ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message};
use async_trait::async_trait;
use reqwest::Client as HttpClient;

/// LLM provider implementation for `OpenRouter`
pub struct OpenRouterProvider {
    client: ChatCompletionsClient,
}

impl OpenRouterProvider {
    /// Create a new `OpenRouter` provider instance
    #[must_use]
    pub fn new(api_key: String) -> Self {
        Self::new_with_client(api_key, crate::llm::support::http::create_http_client())
    }

    /// Create a new `OpenRouter` provider with a shared HTTP client
    ///
    /// This allows connection reuse across multiple providers,
    /// significantly reducing latency for sequential requests.
    #[must_use]
    pub fn new_with_client(api_key: String, http_client: HttpClient) -> Self {
        Self::new_with_endpoint(
            api_key,
            ChatCompletionsProfile::openrouter().default_endpoint,
            http_client,
        )
    }

    #[must_use]
    fn new_with_endpoint(
        api_key: String,
        endpoint: impl Into<String>,
        http_client: HttpClient,
    ) -> Self {
        Self {
            client: ChatCompletionsClient::new(
                http_client,
                endpoint,
                Some(api_key),
                "",
                ChatCompletionsProfile::openrouter(),
            ),
        }
    }

    fn profile(&self) -> ChatCompletionsProfile {
        self.client.profile()
    }

    #[cfg(test)]
    fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
        media::infer_image_mime_type(image_bytes)
    }

    #[cfg(test)]
    fn audio_input_format(mime_type: &str) -> &'static str {
        media::audio_input_format(mime_type)
    }

    fn build_video_request_body(
        model_id: &str,
        video_bytes: &[u8],
        mime_type: &str,
        text_prompt: &str,
        system_prompt: &str,
    ) -> serde_json::Value {
        chat_request::build_video_body(
            video_bytes,
            mime_type,
            text_prompt,
            system_prompt,
            model_id,
            4000,
            OPENROUTER_IMAGE_TEMPERATURE,
        )
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
    chat_response::parse_openrouter_rate_limit(body)
}

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let body = chat_request::build_text_body(
            system_prompt,
            history,
            user_message,
            model_id,
            max_tokens,
            ChatRequestOptions::new(self.profile()).with_native_image_parts(false),
        );

        let res_json = self.client.post_json(&body).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
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
        let body = chat_request::build_audio_body(
            &audio_bytes,
            mime_type,
            text_prompt,
            model_id,
            8000,
            OPENROUTER_AUDIO_TRANSCRIBE_TEMPERATURE,
        );

        let res_json = self.client.post_json(&body).await?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let body = chat_request::build_image_body(
            &image_bytes,
            None,
            text_prompt,
            system_prompt,
            model_id,
            4000,
            OPENROUTER_IMAGE_TEMPERATURE,
            ChatRequestOptions::new(self.profile()),
        );

        let res_json = self.client.post_json(&body).await?;
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
        let body = Self::build_video_request_body(
            model_id,
            &video_bytes,
            mime_type,
            text_prompt,
            system_prompt,
        );

        let res_json = self.client.post_json(&body).await?;
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
            json_mode,
            reasoning_effort: _,
        } = request;
        let body = chat_request::build_tool_body(
            system_prompt,
            history,
            tools,
            model_id,
            max_tokens,
            temperature,
            json_mode,
            ChatRequestOptions::new(self.profile()).with_native_image_parts(false),
        );

        let res_json = self.client.post_json(&body).await?;

        chat_response::parse_chat_response(res_json, self.profile())
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenRouterProvider, parse_openrouter_rate_limit};
    use crate::llm::providers::chat_completions::profile::ChatCompletionsProfile;
    use crate::llm::providers::chat_completions::request::{
        self as chat_request, ChatRequestOptions,
    };
    use crate::llm::{ChatWithToolsRequest, LlmProvider, ToolDefinition};
    use base64::Engine;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn run_capture_server(
        body: impl Into<String>,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 8192];
            let bytes_read = socket.read(&mut buffer).await.expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
            let _ = sender.send(request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        (format!("http://{addr}/api/v1/chat/completions"), receiver)
    }

    fn request_body(request: &str) -> serde_json::Value {
        let (_, body) = request
            .split_once("\r\n\r\n")
            .expect("request contains body separator");
        serde_json::from_str(body).expect("request body is json")
    }

    fn sample_tool() -> ToolDefinition {
        ToolDefinition {
            name: "search".to_string(),
            description: "Search".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}}
            }),
        }
    }

    #[tokio::test]
    async fn openrouter_text_request_uses_headers_and_exact_endpoint() {
        let (endpoint, request_rx) = run_capture_server(
            r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
        )
        .await;
        let provider = OpenRouterProvider::new_with_endpoint(
            " token ".to_string(),
            endpoint,
            reqwest::Client::new(),
        );

        let response = provider
            .complete_internal_text("system", &[], "hello", "openai/gpt-4o", 32)
            .await
            .expect("text response succeeds");
        let request = request_rx.await.expect("request captured");
        let lowercase = request.to_ascii_lowercase();

        assert_eq!(response, "ok");
        assert!(request.starts_with("POST /api/v1/chat/completions HTTP/1.1"));
        assert!(lowercase.contains("authorization: bearer token"));
        assert!(lowercase.contains("http-referer: https://github.com/0fl01/oxide-agent"));
        assert!(lowercase.contains("x-title: oxide agent"));
        assert!(lowercase.contains("x-openrouter-title: oxide agent"));
    }

    #[tokio::test]
    async fn openrouter_tool_request_sets_require_parameters() {
        let (endpoint, request_rx) = run_capture_server(
            r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
        )
        .await;
        let provider = OpenRouterProvider::new_with_endpoint(
            "token".to_string(),
            endpoint,
            reqwest::Client::new(),
        );
        let tools = vec![sample_tool()];

        provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &tools,
                model_id: "openai/gpt-4o",
                max_tokens: 32,
                temperature: Some(0.2),
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect("tool response succeeds");

        let body = request_body(&request_rx.await.expect("request captured"));
        assert_eq!(body["provider"], json!({"require_parameters": true}));
        assert!(body.get("tool_choice").is_none());
        // OpenRouter now has JsonModePolicy::Standard; json_mode=true with
        // tools sets response_format (P0.5 probes confirm support).
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

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
    fn openrouter_image_audio_video_requests_keep_content_part_shapes() {
        let profile = ChatCompletionsProfile::openrouter();
        let image = chat_request::build_image_body(
            b"image-bytes",
            Some("image/png"),
            "Describe",
            "System",
            "google/gemini-3.1-flash-lite-preview",
            4000,
            0.2,
            ChatRequestOptions::new(profile),
        );
        let audio = chat_request::build_audio_body(
            b"audio-bytes",
            "audio/mpeg",
            "Transcribe",
            "google/gemini-3.1-flash-lite-preview",
            8000,
            0.2,
        );
        let video = OpenRouterProvider::build_video_request_body(
            "google/gemini-3.1-flash-lite-preview",
            b"video-bytes",
            "video/mp4",
            "Describe video",
            "System",
        );

        assert_eq!(
            image["messages"][1]["content"][1]["type"],
            json!("image_url")
        );
        assert_eq!(
            audio["messages"][0]["content"][1]["type"],
            json!("input_audio")
        );
        assert_eq!(
            audio["messages"][0]["content"][1]["input_audio"]["format"],
            json!("mp3")
        );
        assert_eq!(
            video["messages"][1]["content"][1]["type"],
            json!("video_url")
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

    #[test]
    fn openrouter_rate_limit_metadata_reset_is_preserved() {
        let reset_ms = chrono::Utc::now().timestamp_millis() + 120_000;
        let body = json!({
            "error": {
                "message": "rate limited",
                "code": 429,
                "metadata": {
                    "headers": {
                        "X-RateLimit-Reset": reset_ms.to_string()
                    }
                }
            }
        })
        .to_string();

        let wait_secs = parse_openrouter_rate_limit(&body).expect("reset parses");
        assert!((115..=120).contains(&wait_secs));
        assert!(parse_openrouter_rate_limit(r#"{"error":{"message":"rate limited"}}"#).is_none());
    }
}
