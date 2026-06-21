pub(crate) mod module;
pub(crate) mod profile;
pub(crate) mod tool_ids;
pub(crate) mod transcription;

#[cfg(feature = "llm-mistral")]
pub(crate) use module::MistralProviderModule;
pub(crate) use module::OpenAIBaseProviderModule;
pub(crate) use profile::OpenAICompatibleProfile;
pub(crate) use tool_ids::ToolCallIdMapper;

use std::sync::{Arc, Mutex};

use crate::config::OPENAI_BASE_CHAT_TEMPERATURE;
#[cfg(test)]
use crate::llm::ToolCall;
use crate::llm::providers::chat_completions::client::ChatCompletionsClient;
use crate::llm::providers::chat_completions::request as chat_completions_request;
use crate::llm::providers::chat_completions::response as chat_completions_response;
use crate::llm::providers::chat_completions::streaming as chat_completions_streaming;
use crate::llm::support::http::{
    APP_USER_AGENT, extract_text_content, parse_retry_after, send_json_request,
};
use crate::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, ToolDefinition,
};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde_json::Value;

/// LLM provider for generic OpenAI-compatible Chat Completions endpoints.
pub struct OpenAIBaseProvider {
    client: ChatCompletionsClient,
    transcription_base: String,
    tool_id_mapper: Arc<Mutex<ToolCallIdMapper>>,
}

impl OpenAIBaseProvider {
    #[must_use]
    pub fn new(api_key: Option<String>, api_base: String) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            api_base,
            crate::llm::support::http::create_http_client(),
            OpenAICompatibleProfile::generic(),
        )
    }

    #[must_use]
    pub fn new_with_client(
        api_key: Option<String>,
        api_base: String,
        http_client: HttpClient,
    ) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            api_base,
            http_client,
            OpenAICompatibleProfile::generic(),
        )
    }

    /// Convenience constructor for a Mistral-profiled provider.
    #[must_use]
    pub fn new_mistral(api_key: Option<String>, http_client: HttpClient) -> Self {
        Self::new_with_client_and_profile(
            api_key,
            "https://api.mistral.ai/v1".to_string(),
            http_client,
            OpenAICompatibleProfile::mistral(),
        )
    }

    #[must_use]
    pub fn new_with_client_and_profile(
        api_key: Option<String>,
        api_base: String,
        http_client: HttpClient,
        profile: OpenAICompatibleProfile,
    ) -> Self {
        let endpoint = chat_completions_url(&api_base);
        Self {
            client: ChatCompletionsClient::new(http_client, endpoint, api_key, "", profile),
            transcription_base: api_base,
            tool_id_mapper: Arc::new(Mutex::new(ToolCallIdMapper::new())),
        }
    }

    fn chat_completions_url(&self) -> &str {
        self.client.endpoint()
    }

    fn auth_header(&self) -> Option<String> {
        self.client.auth_header()
    }

    fn profile(&self) -> OpenAICompatibleProfile {
        self.client.profile()
    }
}

fn chat_completions_url(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/chat/completions")
    }
}

#[cfg(test)]
fn prepare_structured_messages(system_prompt: &str, history: &[Message]) -> Vec<Value> {
    let options =
        chat_completions_request::ChatRequestOptions::new(OpenAICompatibleProfile::generic());
    chat_completions_request::prepare_messages(system_prompt, history, options, None)
}

/// Mistral-strict message layout: history system messages are collected and
/// prepended before the main system prompt. Tool-call IDs are mapped through
/// the [`ToolCallIdMapper`] to 9-character alphanumeric format.
#[cfg(test)]
fn prepare_structured_messages_mistral(
    system_prompt: &str,
    history: &[Message],
    id_mapper: &mut ToolCallIdMapper,
) -> Vec<Value> {
    let options =
        chat_completions_request::ChatRequestOptions::new(OpenAICompatibleProfile::mistral());
    chat_completions_request::prepare_messages(system_prompt, history, options, Some(id_mapper))
}

fn build_tool_chat_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
    options: chat_completions_request::ChatRequestOptions<'_>,
    tool_id_mapper: &mut ToolCallIdMapper,
) -> Value {
    chat_completions_request::build_tool_body(
        system_prompt,
        history,
        tools,
        model_id,
        max_tokens,
        temperature,
        json_mode,
        options,
        Some(tool_id_mapper),
    )
}

fn build_image_analysis_body(
    image_bytes: &[u8],
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
) -> Value {
    chat_completions_request::build_image_body(
        image_bytes,
        None,
        text_prompt,
        system_prompt,
        model_id,
        4000,
        OPENAI_BASE_CHAT_TEMPERATURE,
        chat_completions_request::ChatRequestOptions::new(OpenAICompatibleProfile::generic()),
    )
}

#[cfg(test)]
fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
    chat_completions_request::infer_image_mime_type(image_bytes)
}

fn chat_request_options<'a>(
    profile: &OpenAICompatibleProfile,
) -> chat_completions_request::ChatRequestOptions<'a> {
    chat_completions_request::ChatRequestOptions::new(*profile)
}

#[cfg(test)]
fn normalize_tool_arguments_str(raw: &str) -> String {
    chat_completions_response::normalize_tool_arguments_str(raw)
}

fn parse_chat_response(
    response: Value,
    profile: &OpenAICompatibleProfile,
    id_mapper: &ToolCallIdMapper,
) -> Result<ChatResponse, LlmError> {
    chat_completions_response::parse_chat_response(response, *profile, Some(id_mapper))
}

fn should_stream_chat_response(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

/// Parse ZAI flush time from a rate-limit error message or JSON error body.
///
/// ZAI returns reset time as `next_flush_time` in text such as
/// `Usage limit reached. Your limit will reset at 1710000000`. The timestamp can
/// be Unix seconds, Unix milliseconds, or an RFC3339 datetime string.
#[must_use]
pub fn parse_zai_flush_time(message: &str) -> Option<u64> {
    chat_completions_response::parse_zai_flush_time(message)
}

fn apply_profile_rate_limit_wait(error: LlmError, profile: &OpenAICompatibleProfile) -> LlmError {
    match error {
        LlmError::RateLimit { wait_secs, message } if profile.label == "zai" => {
            LlmError::RateLimit {
                wait_secs: parse_zai_flush_time(&message).or(wait_secs),
                message,
            }
        }
        other => other,
    }
}

fn profile_rate_limit_wait_secs(
    profile: &OpenAICompatibleProfile,
    message: &str,
    fallback: Option<u64>,
) -> Option<u64> {
    chat_completions_response::parse_rate_limit_wait_secs(*profile, message, fallback)
}

#[cfg(test)]
type StreamingChatAccumulator = chat_completions_streaming::StreamingChatAccumulator;
#[cfg(test)]
type PendingStreamingToolCall = chat_completions_streaming::PendingStreamingToolCall;

async fn send_streaming_chat_request(
    client: &HttpClient,
    url: &str,
    body: &Value,
    auth_header: Option<&str>,
    profile: &OpenAICompatibleProfile,
) -> Result<ChatResponse, LlmError> {
    let mut request = client
        .post(url)
        .json(body)
        .header("User-Agent", APP_USER_AGENT);
    if let Some(auth) = auth_header {
        request = request.header("Authorization", auth);
    }

    let response = request.send().await.map_err(LlmError::from_reqwest_error)?;
    let status = response.status();
    if !status.is_success() {
        let retry_after_secs = (status == reqwest::StatusCode::TOO_MANY_REQUESTS)
            .then(|| parse_retry_after(response.headers()))
            .flatten();
        let error_text = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(LlmError::RateLimit {
                wait_secs: profile_rate_limit_wait_secs(profile, &error_text, retry_after_secs),
                message: error_text,
            });
        }
        return Err(LlmError::api_error_status(
            status.as_u16(),
            format!("API error: {status} - {error_text}"),
        ));
    }

    parse_streaming_chat_response(response).await
}

async fn parse_streaming_chat_response(
    response: reqwest::Response,
) -> Result<ChatResponse, LlmError> {
    chat_completions_streaming::parse_streaming_chat_response(response).await
}

#[cfg(test)]
fn process_chat_sse_event(
    raw_event: &str,
    state: &mut StreamingChatAccumulator,
) -> Result<(), LlmError> {
    chat_completions_streaming::process_chat_sse_event(raw_event, state)
}

#[cfg(test)]
fn finish_streaming_chat_response(
    state: StreamingChatAccumulator,
) -> Result<ChatResponse, LlmError> {
    chat_completions_streaming::finish_streaming_chat_response(state)
}

#[cfg(test)]
fn finalize_streaming_tool_calls(pending: Vec<PendingStreamingToolCall>) -> Vec<ToolCall> {
    chat_completions_streaming::finalize_streaming_tool_calls(pending)
}

#[cfg(test)]
fn decode_utf8_prefix(pending_bytes: &mut Vec<u8>) -> Result<Option<String>, LlmError> {
    chat_completions_streaming::decode_utf8_prefix(pending_bytes)
}

#[cfg(test)]
fn normalize_newlines_in_place(buffer: &mut String) {
    chat_completions_streaming::normalize_newlines_in_place(buffer);
}

#[async_trait]
impl LlmProvider for OpenAIBaseProvider {
    async fn complete_internal_text(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let body = {
            let mut mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
            let profile = self.profile();
            let body = chat_completions_request::build_text_body(
                system_prompt,
                history,
                user_message,
                model_id,
                max_tokens,
                chat_request_options(&profile),
                Some(&mut *mapper),
            );
            drop(mapper);
            body
        };
        let auth = self.auth_header();
        let res_json = send_json_request(
            self.client.http_client(),
            self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &self.profile()))?;
        extract_text_content(&res_json, &["choices", "0", "message", "content"])
    }

    async fn transcribe_audio(
        &self,
        audio_bytes: Vec<u8>,
        mime_type: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let profile = self.profile();
        let Some(audio_profile) = profile.audio_transcription else {
            return Err(LlmError::Unknown(format!(
                "Audio transcription not supported by {} profile",
                profile.label,
            )));
        };

        transcription::transcribe_audio(
            self.client.http_client(),
            self.client.api_key(),
            &self.transcription_base,
            audio_bytes,
            mime_type,
            model_id,
            &audio_profile,
            profile.label,
        )
        .await
    }

    async fn analyze_image(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        model_id: &str,
    ) -> Result<String, LlmError> {
        let body = build_image_analysis_body(&image_bytes, text_prompt, system_prompt, model_id);
        let auth = self.auth_header();
        let res_json = send_json_request(
            self.client.http_client(),
            self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &self.profile()))?;
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
            reasoning_effort,
        } = request;
        let body = {
            let mut mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
            let profile = self.profile();
            let body = build_tool_chat_body(
                system_prompt,
                history,
                tools,
                model_id,
                max_tokens,
                temperature,
                json_mode,
                chat_request_options(&profile).with_reasoning_effort(reasoning_effort),
                &mut mapper,
            );
            drop(mapper);
            body
        };
        let auth = self.auth_header();
        let profile = self.profile();
        if should_stream_chat_response(&body) {
            return send_streaming_chat_request(
                self.client.http_client(),
                self.chat_completions_url(),
                &body,
                auth.as_deref(),
                &profile,
            )
            .await;
        }

        let res_json = send_json_request(
            self.client.http_client(),
            self.chat_completions_url(),
            &body,
            auth.as_deref(),
            &[],
        )
        .await
        .map_err(|error| apply_profile_rate_limit_wait(error, &profile))?;
        let mapper = self.tool_id_mapper.lock().expect("mapper lock poisoned");
        parse_chat_response(res_json, &profile, &mapper)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OpenAIBaseProvider, OpenAICompatibleProfile, StreamingChatAccumulator, ToolCallIdMapper,
        build_image_analysis_body, build_tool_chat_body, chat_completions_url,
        chat_request_options, decode_utf8_prefix, finalize_streaming_tool_calls,
        finish_streaming_chat_response, infer_image_mime_type, normalize_newlines_in_place,
        normalize_tool_arguments_str, parse_chat_response, parse_zai_flush_time,
        process_chat_sse_event, send_streaming_chat_request,
    };
    use crate::llm::{
        ChatWithToolsRequest, LlmError, LlmProvider, Message, MessageContentPart, ToolCall,
        ToolCallFunction, ToolDefinition,
    };
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sample_tool() -> ToolDefinition {
        ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"]
            }),
        }
    }

    fn generic_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::generic()
    }

    fn mistral_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::mistral()
    }

    fn zai_profile() -> OpenAICompatibleProfile {
        OpenAICompatibleProfile::zai()
    }

    async fn run_single_response_server(
        body: impl Into<String>,
        content_type: &'static str,
    ) -> String {
        run_single_status_response_server("200 OK", body, content_type, &[]).await
    }

    async fn run_single_status_response_server(
        status: &'static str,
        body: impl Into<String>,
        content_type: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> String {
        let body = body.into();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server binds");
        let addr = listener.local_addr().expect("local addr available");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await.expect("read request");
            let extra_headers = headers
                .iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });
        format!("http://{addr}/v1")
    }

    #[test]
    fn chat_completions_url_accepts_base_or_endpoint() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8080/v1/"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://127.0.0.1:8080/v1/chat/completions"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn auth_header_is_optional() {
        let unauthenticated = OpenAIBaseProvider::new(None, "http://localhost/v1".to_string());
        assert_eq!(unauthenticated.auth_header(), None);

        let authenticated = OpenAIBaseProvider::new(
            Some(" token ".to_string()),
            "http://localhost/v1".to_string(),
        );
        assert_eq!(authenticated.auth_header().as_deref(), Some("Bearer token"));
    }

    #[test]
    fn openai_base_wrapper_uses_chat_completions_profile_constructor() {
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            Some(" token ".to_string()),
            "https://api.mistral.ai/v1".to_string(),
            crate::llm::support::http::create_http_client(),
            OpenAICompatibleProfile::mistral(),
        );

        assert_eq!(
            provider.client.profile(),
            OpenAICompatibleProfile::mistral()
        );
        assert_eq!(provider.client.profile().label, "mistral");
        assert_eq!(
            provider.client.endpoint(),
            "https://api.mistral.ai/v1/chat/completions"
        );
        assert_eq!(provider.auth_header().as_deref(), Some("Bearer token"));
    }

    #[test]
    fn builds_tool_chat_body_with_tools_and_without_parallel_tool_calls() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "You are helpful.",
            &[],
            &[sample_tool()],
            "local-model",
            4096,
            None,
            true,
            chat_request_options(&generic_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["model"], json!("local-model"));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert!(body.get("tools").is_some());
        assert!(body.get("parallel_tool_calls").is_none());
        // json_mode=true with tools now sets response_format (P0.5 probes confirm support).
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn adds_json_mode_only_without_tools() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "local-model",
            1024,
            None,
            true,
            chat_request_options(&generic_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn encodes_native_image_parts_in_chat_messages() {
        let user = Message::user("What is this?").with_user_content_parts(vec![
            MessageContentPart::image("image/png", b"png".to_vec()),
        ]);
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[user],
            &[],
            "vision-model",
            1024,
            None,
            false,
            chat_request_options(&generic_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        let content = &body["messages"][1]["content"];
        assert_eq!(content[0]["type"], json!("text"));
        assert_eq!(content[1]["type"], json!("image_url"));
        assert_eq!(
            content[1]["image_url"]["url"],
            json!("data:image/png;base64,cG5n")
        );
    }

    #[test]
    fn builds_image_analysis_body_with_data_url() {
        let body = build_image_analysis_body(b"jpg", "Describe", "System", "vision-model");

        assert_eq!(body["messages"][1]["content"][0]["text"], json!("Describe"));
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["url"],
            json!("data:image/jpeg;base64,anBn")
        );
    }

    #[test]
    fn infers_common_image_mime_types() {
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
        let jpeg = [0xFF, 0xD8, 0xFF];
        let gif = *b"GIF89a";
        let webp = [b'R', b'I', b'F', b'F', 0, 0, 0, 0, b'W', b'E', b'B', b'P'];

        assert_eq!(infer_image_mime_type(&png), "image/png");
        assert_eq!(infer_image_mime_type(&jpeg), "image/jpeg");
        assert_eq!(infer_image_mime_type(&gif), "image/gif");
        assert_eq!(infer_image_mime_type(&webp), "image/webp");
    }

    #[test]
    fn decode_utf8_prefix_keeps_split_multibyte_tail() {
        let mut pending = b"hello ".to_vec();
        pending.extend_from_slice(&[0xF0, 0x9F]);

        assert_eq!(
            decode_utf8_prefix(&mut pending)
                .expect("valid utf8 prefix")
                .as_deref(),
            Some("hello ")
        );
        assert_eq!(pending, vec![0xF0, 0x9F]);

        pending.extend_from_slice(&[0x99, 0x82]);
        assert_eq!(
            decode_utf8_prefix(&mut pending)
                .expect("completed utf8")
                .as_deref(),
            Some("🙂")
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn normalize_newlines_keeps_sse_boundaries_predictable() {
        let mut buffer = "data: one\r\n\r\ndata: two\n\n".to_string();

        normalize_newlines_in_place(&mut buffer);

        assert_eq!(buffer, "data: one\n\ndata: two\n\n");
    }

    #[test]
    fn normalizes_tool_arguments() {
        assert_eq!(normalize_tool_arguments_str(""), "{}");
        assert_eq!(
            normalize_tool_arguments_str(r#"{"city":"Paris"}"#),
            r#"{"city":"Paris"}"#
        );
        assert_eq!(
            normalize_tool_arguments_str(r#""{\"city\":\"Paris\"}""#),
            r#"{"city":"Paris"}"#
        );
    }

    #[test]
    fn parses_tool_calls_and_usage() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": {"city": "Paris"}}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15,
                "prompt_tokens_details": {"cached_tokens": 7}
            }
        });

        let parsed = parse_chat_response(response, &generic_profile(), &ToolCallIdMapper::new())
            .expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call_1");
        assert_eq!(parsed.usage.expect("usage").cached_tokens, Some(7));
    }

    // -----------------------------------------------------------------------
    // Mistral message layout policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_prepare_structured_messages_formats_tool_message() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![Message::tool(
            "call_abc123",
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages =
            prepare_structured_messages_mistral("You are helpful.", &history, &mut mapper);

        let tool_msg = &messages[1];
        assert_eq!(tool_msg["role"], json!("tool"));
        assert_eq!(tool_msg["content"], json!("{\"temperature\": 20}"));
        // "call_abc123" -> filter -> "callabc123" -> last 9 -> "allabc123"
        assert_eq!(tool_msg["tool_call_id"], json!("allabc123"));
        assert_eq!(tool_msg["name"], json!("get_weather"));
    }

    #[test]
    fn mistral_prepare_structured_messages_preserves_assistant_tool_calls() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![Message::assistant_with_tools(
            "I'll get the weather.",
            vec![ToolCall::new(
                "call_xyz".to_string(),
                ToolCallFunction {
                    name: "get_weather".to_string(),
                    arguments: "{\"city\":\"Paris\"}".to_string(),
                },
                false,
            )],
        )];
        let messages =
            prepare_structured_messages_mistral("You are helpful.", &history, &mut mapper);

        let assistant_msg = &messages[1];
        assert_eq!(assistant_msg["role"], json!("assistant"));
        assert_eq!(assistant_msg["content"], json!("I'll get the weather."));
        let tool_calls = assistant_msg["tool_calls"]
            .as_array()
            .expect("tool_calls should be present");
        assert_eq!(tool_calls.len(), 1);
        // "call_xyz" -> filter -> "callxyz" -> last 9 -> "callxyz" (7 chars)
        assert_eq!(tool_calls[0]["id"], json!("callxyz"));
        assert_eq!(tool_calls[0]["function"]["name"], json!("get_weather"));
    }

    #[test]
    fn mistral_system_messages_collected_before_main_system_prompt() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let history = vec![
            Message {
                role: "system".to_string(),
                content: "History system instruction".to_string(),
                ..Message::user("")
            },
            Message::user("Hello"),
        ];
        let messages =
            prepare_structured_messages_mistral("Main system prompt", &history, &mut mapper);

        // Order: history system, main system, user
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[0]["content"], json!("History system instruction"));
        assert_eq!(messages[1]["role"], json!("system"));
        assert_eq!(messages[1]["content"], json!("Main system prompt"));
        assert_eq!(messages[2]["role"], json!("user"));
    }

    #[test]
    fn generic_messages_put_main_system_prompt_first() {
        use super::prepare_structured_messages;
        let history = vec![
            Message {
                role: "system".to_string(),
                content: "History system instruction".to_string(),
                ..Message::user("")
            },
            Message::user("Hello"),
        ];
        let messages = prepare_structured_messages("Main system prompt", &history);

        // Order: main system, history system, user
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[0]["content"], json!("Main system prompt"));
        assert_eq!(messages[1]["role"], json!("system"));
        assert_eq!(messages[1]["content"], json!("History system instruction"));
        assert_eq!(messages[2]["role"], json!("user"));
    }

    #[test]
    fn mistral_bidirectional_id_mapping_roundtrip() {
        use super::prepare_structured_messages_mistral;
        let mut mapper = ToolCallIdMapper::new();
        let original_id = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";

        // Step 1: Register the original ID
        let mistral_id = mapper.register(original_id.to_string());
        assert_eq!(mistral_id, "43acb9e14");

        // Step 2: Prepare tool result message -- should use mapped ID
        let history = vec![Message::tool(
            original_id,
            "get_weather",
            "{\"temperature\": 20}",
        )];
        let messages = prepare_structured_messages_mistral("sys", &history, &mut mapper);
        let tool_msg = &messages[1];
        assert_eq!(tool_msg["tool_call_id"], json!(mistral_id));
    }

    // -----------------------------------------------------------------------
    // Mistral response content policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_parse_content_array_with_reasoning_chunks() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": [
                        {"type": "thinking", "content": "Let me think about this"},
                        {"type": "text", "text": "Hello world"}
                    ],
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello world"));
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("Let me think about this")
        );
    }

    #[test]
    fn mistral_parse_top_level_reasoning_content_fallback() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Hello",
                    "reasoning_content": "Internal reasoning",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Hello"));
        // reasoning_content from string content is None; top-level fallback kicks in
        assert_eq!(
            parsed.reasoning_content.as_deref(),
            Some("Internal reasoning")
        );
    }

    #[test]
    fn mistral_parse_tool_calls_with_known_mapping() {
        let mut mapper = ToolCallIdMapper::new();
        let original_id = "call_44456aeb-f16d-4c5e-8f38-f1243acb9e14";
        let mistral_id = mapper.register(original_id.to_string());
        assert_eq!(mistral_id, "43acb9e14");

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": mistral_id,
                        "type": "function",
                        "function": {"name": "get_weather", "arguments": "{\"city\": \"Paris\"}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Known mapping: invocation_id restores original ID, wire ID is the mistral ID
        assert_eq!(parsed.tool_calls[0].invocation_id().as_str(), original_id);
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), mistral_id);
    }

    #[test]
    fn mistral_parse_unknown_tool_call_id_becomes_provider_correlated() {
        let mapper = ToolCallIdMapper::new(); // empty mapper = no known mappings

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "D681PevKs",
                        "type": "function",
                        "function": {"name": "search", "arguments": "{}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Unknown mapping: provider-correlated (uses provider ID as-is)
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "D681PevKs");
    }

    #[test]
    fn mistral_parse_empty_tool_call_id_becomes_uncorrelated() {
        let mapper = ToolCallIdMapper::new();

        let response = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "  ",
                        "type": "function",
                        "function": {"name": "run_code", "arguments": "{}"}
                    }],
                    "role": "assistant"
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        });
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.tool_calls.len(), 1);
        // Empty ID: uncorrelated -- has no provider_tool_call_id (uses generated UUID)
        assert!(
            parsed.tool_calls[0]
                .tool_call_correlation
                .as_ref()
                .is_none_or(|c| c.provider_tool_call_id.is_none())
        );
        // wire_tool_call_id falls back to invocation_id (generated UUID)
        assert!(
            parsed.tool_calls[0]
                .wire_tool_call_id()
                .starts_with("call_")
        );
    }

    #[test]
    fn mistral_parse_cached_tokens_in_usage() {
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Hello",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 10,
                "total_tokens": 110,
                "prompt_tokens_details": {"cached_tokens": 42}
            }
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &mistral_profile(), &mapper).expect("response parses");
        let usage = parsed.usage.expect("usage present");
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 10);
        assert_eq!(usage.cached_tokens, Some(42));
    }

    #[test]
    fn generic_parse_preserves_string_only_behavior() {
        // Generic profile (StringOnly) does NOT handle content arrays
        let response = json!({
            "choices": [{
                "message": {
                    "content": "Simple text",
                    "role": "assistant"
                },
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}
        });
        let mapper = ToolCallIdMapper::new();
        let parsed =
            parse_chat_response(response, &generic_profile(), &mapper).expect("response parses");
        assert_eq!(parsed.content.as_deref(), Some("Simple text"));
        assert!(parsed.reasoning_content.is_none());
    }

    // -----------------------------------------------------------------------
    // Checkpoint 5: Request tweaks -- temperatures, parallel_tool_calls,
    // reasoning_effort, JSON mode
    // -----------------------------------------------------------------------

    #[test]
    fn mistral_tool_body_includes_parallel_tool_calls_and_tool_choice() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-large-latest",
            4096,
            None,
            false,
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert!(body.get("tools").is_some());
    }

    #[test]
    fn mistral_reasoning_model_tool_body_includes_reasoning_effort() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-small-2603",
            4096,
            None,
            false,
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["reasoning_effort"], json!("high"));
        // Reasoning model uses reasoning_temperature (0.7), not tool_temperature
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.7).abs() < 1e-6);
    }

    #[test]
    fn mistral_regular_model_tool_body_uses_tool_temperature() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-large-latest",
            4096,
            None,
            false,
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        // Regular model: no reasoning_effort
        assert!(body.get("reasoning_effort").is_none());
        // tool_temperature = 0.7
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.7).abs() < 1e-6);
    }

    #[test]
    fn mistral_tool_body_explicit_temperature_overrides_default() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "mistral-large-latest",
            4096,
            Some(0.23),
            false,
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.23).abs() < 1e-6);
    }

    #[test]
    fn mistral_reasoning_model_tool_body_explicit_effort_overrides_default() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "mistral-small-2603",
            4096,
            None,
            false,
            chat_request_options(&mistral_profile()).with_reasoning_effort(Some("none")),
            &mut mapper,
        );

        assert_eq!(body["reasoning_effort"], json!("none"));
    }

    #[test]
    fn generic_tool_body_no_parallel_or_reasoning() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "some-model",
            4096,
            None,
            false,
            chat_request_options(&generic_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert!(body.get("parallel_tool_calls").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn zai_tool_body_sets_stream_and_enabled_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "glm-4.7",
            4096,
            None,
            false,
            chat_request_options(&zai_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert!(body.get("response_format").is_none());
        let temp = body["temperature"].as_f64().expect("temperature present");
        assert!((temp - 0.95).abs() < 1e-6);
    }

    #[test]
    fn zai_plain_body_without_json_streams_with_enabled_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "glm-4.7",
            1024,
            None,
            false,
            chat_request_options(&zai_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["thinking"], json!({"type": "enabled"}));
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn zai_native_json_body_disables_thinking_and_streaming() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "glm-4.7",
            1024,
            None,
            true,
            chat_request_options(&zai_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn zai_json_with_tools_uses_native_json_mode() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "glm-4.7",
            1024,
            None,
            true,
            chat_request_options(&zai_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        // P0.5 probes confirm json_object + tools is accepted by ZAI.
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
        // ZaiUnlessNativeJsonMode: stream=false when native_json_mode=true
        assert_eq!(body["stream"], json!(false));
        // ZaiEnabledUnlessJsonMode: thinking disabled when native_json_mode=true
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[test]
    fn generic_tool_body_does_not_send_zai_thinking() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "some-model",
            4096,
            None,
            false,
            chat_request_options(&generic_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert!(body.get("thinking").is_none());
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn zai_sse_aggregates_content_reasoning_finish_and_usage() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"content":"hel","reasoning_content":"think "}}]}"#,
            &mut state,
        )
        .expect("first event parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"content":"lo","reasoning_content":"again"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":3}}}"#,
            &mut state,
        )
        .expect("second event parses");
        process_chat_sse_event("data: [DONE]", &mut state).expect("done event ignored");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.content.as_deref(), Some("hello"));
        assert_eq!(response.reasoning_content.as_deref(), Some("think again"));
        assert_eq!(response.finish_reason, "stop");
        let usage = response.usage.expect("usage captured");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 4);
        assert_eq!(usage.total_tokens, 14);
        assert_eq!(usage.cached_tokens, Some(3));
    }

    #[test]
    fn zai_sse_aggregates_fragmented_tool_arguments_and_preserves_id() {
        let mut state = StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        };

        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-zai-1","type":"function","function":{"name":"search","arguments":"{\"q"}}]}}]}"#,
            &mut state,
        )
        .expect("first tool delta parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":\"oxi"}}]}}]}"#,
            &mut state,
        )
        .expect("second tool delta parses");
        process_chat_sse_event(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"de\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            &mut state,
        )
        .expect("final tool delta parses");

        let response = finish_streaming_chat_response(state).expect("stream finalizes");
        assert_eq!(response.finish_reason, "tool_calls");
        assert_eq!(response.tool_calls.len(), 1);
        assert_ne!(
            response.tool_calls[0].invocation_id().as_str(),
            "call-zai-1"
        );
        assert_eq!(response.tool_calls[0].wire_tool_call_id(), "call-zai-1");
        assert_eq!(response.tool_calls[0].function.name, "search");
        assert_eq!(
            response.tool_calls[0].function.arguments,
            r#"{"q":"oxide"}"#
        );
    }

    #[test]
    fn zai_sse_empty_response_errors_cleanly() {
        let err = finish_streaming_chat_response(StreamingChatAccumulator {
            finish_reason: "unknown".to_string(),
            ..StreamingChatAccumulator::default()
        })
        .expect_err("empty stream should fail");

        assert!(err.to_string().contains("Empty response"));
    }

    #[test]
    fn streaming_tool_calls_handle_empty_id_as_uncorrelated() {
        let tool_calls = finalize_streaming_tool_calls(vec![super::PendingStreamingToolCall {
            id: Some("".to_string()),
            name: Some("search".to_string()),
            arguments: "{}".to_string(),
        }]);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(
            tool_calls[0].wire_tool_call_id(),
            tool_calls[0].invocation_id().as_str()
        );
    }

    #[test]
    fn parse_zai_flush_time_unix_timestamp() {
        let future_ts = (chrono::Utc::now().timestamp() + 300).to_string();
        let message = format!("Usage limit reached. Your limit will reset at {future_ts}");

        let wait_secs = parse_zai_flush_time(&message).expect("unix timestamp parses");
        assert!((wait_secs as i64 - 300).abs() < 5, "~300 seconds");
    }

    #[test]
    fn parse_zai_flush_time_milliseconds() {
        let future_ms = (chrono::Utc::now().timestamp_millis() + 300_000).to_string();
        let message = format!("Usage limit reached. Your limit will reset at {future_ms}");

        let wait_secs = parse_zai_flush_time(&message).expect("millisecond timestamp parses");
        assert!((wait_secs as i64 - 300).abs() < 5, "~300 seconds");
    }

    #[test]
    fn parse_zai_flush_time_iso_datetime() {
        let future_dt = chrono::Utc::now() + chrono::Duration::minutes(5);
        let message = format!(
            "Usage limit reached. Your limit will reset at {}",
            future_dt.format("%Y-%m-%dT%H:%M:%SZ")
        );

        let wait_secs = parse_zai_flush_time(&message).expect("ISO datetime parses");
        assert!(wait_secs >= 200, "~5 minutes");
    }

    #[test]
    fn parse_zai_flush_time_no_timestamp() {
        let wait_secs = parse_zai_flush_time("Rate limit exceeded. Please try again later.");
        assert_eq!(wait_secs, None);
    }

    #[tokio::test]
    async fn zai_chat_with_tools_uses_sse_transport() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\",\"reasoning_content\":\"reason\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let api_base = run_single_response_server(body, "text/event-stream").await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );
        let tools = vec![sample_tool()];

        let response = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &tools,
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: false,
                reasoning_effort: None,
            })
            .await
            .expect("SSE response parses");

        assert_eq!(response.content.as_deref(), Some("hello"));
        assert_eq!(response.reasoning_content.as_deref(), Some("reason"));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.expect("usage").total_tokens, 5);
    }

    #[tokio::test]
    async fn zai_native_json_chat_uses_non_streaming_transport() {
        let body = r#"{"choices":[{"message":{"content":"{\"ok\":true}","role":"assistant"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#;
        let api_base = run_single_response_server(body, "application/json").await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );

        let response = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &[],
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect("JSON response parses");

        assert_eq!(response.content.as_deref(), Some(r#"{"ok":true}"#));
        assert_eq!(response.finish_reason, "stop");
        assert_eq!(response.usage.expect("usage").total_tokens, 3);
    }

    #[tokio::test]
    async fn zai_streaming_429_uses_next_flush_time() {
        let future_ts = chrono::Utc::now().timestamp() + 240;
        let body = format!(
            r#"{{"error":{{"message":"Usage limit reached. Your limit will reset at {future_ts}"}}}}"#
        );
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            body,
            "application/json",
            &[],
        )
        .await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );
        let tools = vec![sample_tool()];

        let err = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &tools,
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: false,
                reasoning_effort: None,
            })
            .await
            .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, message } => {
                let wait_secs = wait_secs.expect("next_flush_time should be parsed");
                assert!((wait_secs as i64 - 240).abs() < 5, "~240 seconds");
                assert!(message.contains("Usage limit reached"));
            }
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn zai_native_json_429_uses_next_flush_time() {
        let future_ts = chrono::Utc::now().timestamp() + 180;
        let body = format!(
            r#"{{"error":{{"message":"Usage limit reached. Your limit will reset at {future_ts}"}}}}"#
        );
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            body,
            "application/json",
            &[],
        )
        .await;
        let provider = OpenAIBaseProvider::new_with_client_and_profile(
            None,
            api_base,
            reqwest::Client::new(),
            zai_profile(),
        );

        let err = provider
            .chat_with_tools(ChatWithToolsRequest {
                system_prompt: "system",
                messages: &[],
                tools: &[],
                model_id: "glm-4.7",
                max_tokens: 128,
                temperature: None,
                json_mode: true,
                reasoning_effort: None,
            })
            .await
            .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, message } => {
                let wait_secs = wait_secs.expect("next_flush_time should be parsed");
                assert!((wait_secs as i64 - 180).abs() < 5, "~180 seconds");
                assert!(message.contains("Usage limit reached"));
            }
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn generic_streaming_429_uses_retry_after_header() {
        let api_base = run_single_status_response_server(
            "429 Too Many Requests",
            r#"{"error":"rate limit"}"#,
            "application/json",
            &[("Retry-After", "17")],
        )
        .await;

        let err = send_streaming_chat_request(
            &reqwest::Client::new(),
            &format!("{api_base}/chat/completions"),
            &json!({"stream": true}),
            None,
            &generic_profile(),
        )
        .await
        .expect_err("429 should map to rate limit");

        match err {
            LlmError::RateLimit { wait_secs, .. } => assert_eq!(wait_secs, Some(17)),
            other => panic!("expected rate limit, got {other:?}"),
        }
    }

    #[test]
    fn json_mode_added_with_tools_for_mistral_profile() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[sample_tool()],
            "local-model",
            1024,
            None,
            true, // json_mode = true
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        // P0.5 probes confirm json_object + tools is accepted by Mistral.
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }

    #[test]
    fn json_mode_added_without_tools_for_mistral_profile() {
        let mut mapper = ToolCallIdMapper::new();
        let body = build_tool_chat_body(
            "system",
            &[],
            &[],
            "local-model",
            1024,
            None,
            true, // json_mode = true
            chat_request_options(&mistral_profile()).with_reasoning_effort(None),
            &mut mapper,
        );

        assert_eq!(body["response_format"], json!({"type": "json_object"}));
    }
}
