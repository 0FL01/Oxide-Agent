mod messages;
mod stream;

use super::ZaiProvider;
use crate::llm::{ChatResponse, LlmError, Message, ToolDefinition};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use tracing::warn;
use zai_rs::model::chat::ChatCompletion;
use zai_rs::model::chat_base_response::ChatCompletionResponse;
use zai_rs::model::chat_message_types::{TextMessage, VisionMessage, VisionRichContent};
use zai_rs::model::chat_models::{GLM4_5_air, GLM4_5v, GLM5_turbo, GLM4_7, GLM5};
use zai_rs::model::tools::ThinkingType;
use zai_rs::model::traits::{Chat, ModelName, ThinkEnable};
use zai_rs::ZaiError;

use messages::{
    convert_to_text_messages, convert_to_vision_messages, convert_tools, extract_text_content,
};
use stream::stream_text_response;

const ZAI_TEMPERATURE: f32 = 0.95;
const ZAI_IMAGE_MAX_TOKENS: u32 = 4000;

#[derive(Debug)]
enum ZaiModel {
    Main(GLM4_7),
    Sub(GLM4_5_air),
    Vision(GLM4_5v),
    Flagship5(GLM5),
    Turbo5(GLM5_turbo),
}

impl ZaiProvider {
    pub(super) async fn chat_completion_sdk(
        &self,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        model_id: &str,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let response = match select_model(model_id)? {
            ZaiModel::Main(model) => {
                self.text_chat_completion(model, system_prompt, history, user_message, max_tokens)
                    .await?
            }
            ZaiModel::Sub(model) => {
                self.text_chat_completion(model, system_prompt, history, user_message, max_tokens)
                    .await?
            }
            ZaiModel::Flagship5(model) => {
                self.text_chat_completion(model, system_prompt, history, user_message, max_tokens)
                    .await?
            }
            ZaiModel::Turbo5(model) => {
                self.text_chat_completion(model, system_prompt, history, user_message, max_tokens)
                    .await?
            }
            ZaiModel::Vision(model) => {
                let messages =
                    convert_to_vision_messages(system_prompt, history, Some(user_message));
                let client = build_vision_request(
                    model,
                    messages,
                    &self.api_key,
                    &self.api_base,
                    max_tokens,
                )?;
                client.send().await.map_err(map_zai_error)?
            }
        };

        extract_text_from_response(response)
    }

    pub(super) async fn analyze_image_sdk(
        &self,
        image_bytes: Vec<u8>,
        text_prompt: &str,
        system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        let image_base64 = BASE64.encode(&image_bytes);
        let data_url = format!("data:image/jpeg;base64,{image_base64}");
        let user_message = VisionMessage::new_user()
            .add_user(VisionRichContent::image(data_url))
            .add_user(VisionRichContent::text(text_prompt.to_string()));

        let messages = vec![VisionMessage::system(system_prompt), user_message];
        let client = build_vision_request(
            GLM4_5v {},
            messages,
            &self.api_key,
            &self.api_base,
            ZAI_IMAGE_MAX_TOKENS,
        )?;

        let response = client.send().await.map_err(map_zai_error)?;
        extract_text_from_response(response)
    }

    pub(super) async fn chat_with_tools_sdk(
        &self,
        system_prompt: &str,
        history: &[Message],
        tools: &[ToolDefinition],
        model_id: &str,
        max_tokens: u32,
    ) -> Result<ChatResponse, LlmError> {
        let messages = convert_to_text_messages(system_prompt, history, None);
        let converted_tools = convert_tools(tools);

        match select_model(model_id)? {
            ZaiModel::Main(model) => {
                let mut client =
                    build_text_request(model, messages, &self.api_key, &self.api_base, max_tokens)?;
                if !converted_tools.is_empty() {
                    client = client.add_tools(converted_tools);
                }
                let client = client.enable_stream().with_tool_stream(true);
                stream_text_response(client).await
            }
            ZaiModel::Sub(model) => {
                let mut client =
                    build_text_request(model, messages, &self.api_key, &self.api_base, max_tokens)?;
                if !converted_tools.is_empty() {
                    client = client.add_tools(converted_tools);
                }
                let client = client.enable_stream();
                stream_text_response(client).await
            }
            ZaiModel::Flagship5(model) => {
                let mut client =
                    build_text_request(model, messages, &self.api_key, &self.api_base, max_tokens)?;
                if !converted_tools.is_empty() {
                    client = client.add_tools(converted_tools);
                }
                let client = client.enable_stream();
                stream_text_response(client).await
            }
            ZaiModel::Turbo5(model) => {
                let mut client =
                    build_text_request(model, messages, &self.api_key, &self.api_base, max_tokens)?;
                if !converted_tools.is_empty() {
                    client = client.add_tools(converted_tools);
                }
                let client = client.enable_stream();
                stream_text_response(client).await
            }
            ZaiModel::Vision(_) => Err(LlmError::Unknown(
                "ZAI vision model does not support tool calling".to_string(),
            )),
        }
    }

    async fn text_chat_completion<N>(
        &self,
        model: N,
        system_prompt: &str,
        history: &[Message],
        user_message: &str,
        max_tokens: u32,
    ) -> Result<ChatCompletionResponse, LlmError>
    where
        N: ModelName + Chat + ThinkEnable + Serialize,
        (N, TextMessage): zai_rs::model::traits::Bounded,
    {
        let messages = convert_to_text_messages(system_prompt, history, Some(user_message));
        let client =
            build_text_request(model, messages, &self.api_key, &self.api_base, max_tokens)?;
        client.send().await.map_err(map_zai_error)
    }
}

fn select_model(model_id: &str) -> Result<ZaiModel, LlmError> {
    let normalized = model_id.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "glm-4.7" | "glm-4" | "mainagent" => Ok(ZaiModel::Main(GLM4_7 {})),
        "glm-4.5-air" | "glm-4-air" | "subagent" => Ok(ZaiModel::Sub(GLM4_5_air {})),
        "glm-4.5v" | "glm-4v" => Ok(ZaiModel::Vision(GLM4_5v {})),
        "glm-5" | "flagship5" => Ok(ZaiModel::Flagship5(GLM5 {})),
        "glm-5-turbo" | "turbo5" => Ok(ZaiModel::Turbo5(GLM5_turbo {})),
        _ => Err(LlmError::Unknown(format!(
            "Unsupported ZAI model id: {model_id}"
        ))),
    }
}

fn build_text_request<N>(
    model: N,
    messages: Vec<TextMessage>,
    api_key: &str,
    api_base: &str,
    max_tokens: u32,
) -> Result<ChatCompletion<N, TextMessage>, LlmError>
where
    N: ModelName + Chat + ThinkEnable + Serialize,
    (N, TextMessage): zai_rs::model::traits::Bounded,
{
    let mut iter = messages.into_iter();
    let first = iter
        .next()
        .ok_or_else(|| LlmError::ApiError("ZAI request has no messages".to_string()))?;

    let mut client = ChatCompletion::new(model, first, api_key.to_string())
        .with_url(api_base)
        .with_temperature(ZAI_TEMPERATURE)
        .with_max_tokens(max_tokens)
        .with_thinking(ThinkingType::Enabled);

    for message in iter {
        client = client.add_messages(message);
    }

    Ok(client)
}

fn build_vision_request(
    model: GLM4_5v,
    messages: Vec<VisionMessage>,
    api_key: &str,
    api_base: &str,
    max_tokens: u32,
) -> Result<ChatCompletion<GLM4_5v, VisionMessage>, LlmError> {
    let mut iter = messages.into_iter();
    let first = iter
        .next()
        .ok_or_else(|| LlmError::ApiError("ZAI request has no messages".to_string()))?;

    let mut client = ChatCompletion::new(model, first, api_key.to_string())
        .with_url(api_base)
        .with_temperature(ZAI_TEMPERATURE)
        .with_max_tokens(max_tokens);

    for message in iter {
        client = client.add_messages(message);
    }

    Ok(client)
}

fn extract_text_from_response(response: ChatCompletionResponse) -> Result<String, LlmError> {
    let choice = response
        .choices
        .as_ref()
        .and_then(|choices| choices.first())
        .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))?;

    let content = extract_text_content(choice.message.content.clone())
        .ok_or_else(|| LlmError::ApiError("Empty response".to_string()))?;

    Ok(content)
}

fn map_zai_error(error: ZaiError) -> LlmError {
    match error {
        ZaiError::RateLimitError { message, .. } => {
            let wait_secs = parse_zai_flush_time(&message);
            LlmError::RateLimit { wait_secs, message }
        }
        ZaiError::HttpError {
            status: 429,
            message,
        } => {
            let wait_secs = parse_zai_flush_time(&message);
            LlmError::RateLimit { wait_secs, message }
        }
        ZaiError::Unknown { code: 0, message } if is_stream_transport_error(&message) => {
            warn!(
                error = %message,
                "Retryable ZAI SSE transport failure detected"
            );
            LlmError::NetworkError(message)
        }
        ZaiError::NetworkError(err) => LlmError::NetworkError(err.to_string()),
        ZaiError::JsonError(err) => LlmError::JsonError(err.to_string()),
        other => LlmError::ApiError(other.to_string()),
    }
}

/// Parse ZAI flush time from error message.
///
/// ZAI returns rate limit reset time in error messages like:
/// - "Usage limit reached. Your limit will reset at ${next_flush_time}"
/// - The next_flush_time can be a Unix timestamp or datetime string
///
/// Returns seconds to wait, or None if parsing fails.
pub fn parse_zai_flush_time(message: &str) -> Option<u64> {
    // Try to find timestamp pattern in message
    // ZAI may return: timestamp, ISO datetime, or placeholder ${next_flush_time}
    let message_lower = message.to_lowercase();

    // Pattern 1: Unix timestamp (digits only)
    if let Some(caps) = regex::Regex::new(r"\b(\d{10,13})\b")
        .ok()
        .and_then(|r| r.captures(&message_lower))
    {
        if let Some(ts_str) = caps.get(1) {
            let ts = ts_str.as_str();
            // Determine if seconds or milliseconds
            let ts_value: i64 = ts.parse().ok()?;
            let ts_seconds = if ts.len() > 10 {
                // Milliseconds
                ts_value / 1000
            } else {
                ts_value
            };
            let now = chrono::Utc::now().timestamp();
            let wait_secs = ts_seconds - now;
            if wait_secs > 0 {
                return Some(wait_secs as u64);
            }
        }
    }

    // Pattern 2: ISO datetime string (look for it in the message)
    if let Some(caps) =
        regex::Regex::new(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)")
            .ok()
            .and_then(|r| r.captures(message))
    {
        if let Some(dt_str) = caps.get(1) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(dt_str.as_str()) {
                let now = chrono::Utc::now();
                let duration = dt.signed_duration_since(now);
                if duration.num_seconds() > 0 {
                    return Some(duration.num_seconds() as u64);
                }
            }
        }
    }

    None
}

fn is_stream_transport_error(message: &str) -> bool {
    message
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("stream error:")
}

#[cfg(test)]
mod tests {
    use super::{is_stream_transport_error, map_zai_error, select_model};
    use crate::llm::LlmError;
    use zai_rs::ZaiError;

    // ── select_model tests ──────────────────────────────────────────────

    #[test]
    fn select_model_glm5_turbo_exact() {
        assert!(select_model("glm-5-turbo").is_ok());
    }

    #[test]
    fn select_model_glm5_turbo_alias_turbo5() {
        assert!(select_model("turbo5").is_ok());
    }

    #[test]
    fn select_model_glm5_turbo_case_insensitive() {
        assert!(select_model("GLM-5-Turbo").is_ok());
        assert!(select_model(" Glm-5-Turbo ").is_ok());
    }

    #[test]
    fn select_model_glm5_exact() {
        assert!(select_model("glm-5").is_ok());
    }

    #[test]
    fn select_model_glm5_alias_flagship5() {
        assert!(select_model("flagship5").is_ok());
    }

    #[test]
    fn select_model_glm5_case_insensitive() {
        assert!(select_model("GLM-5").is_ok());
        assert!(select_model(" glm-5 ").is_ok());
    }

    #[test]
    fn select_model_glm5_turbo_and_glm5_are_distinct() {
        // Both must resolve successfully — they map to different internal types
        assert!(select_model("glm-5-turbo").is_ok());
        assert!(select_model("glm-5").is_ok());
    }

    #[test]
    fn select_model_rejects_unknown() {
        let err = select_model("glm-3-fake").unwrap_err();
        assert!(matches!(err, LlmError::Unknown(msg) if msg.contains("glm-3-fake")));
    }

    #[test]
    fn select_model_existing_aliases_still_work() {
        assert!(select_model("glm-4.7").is_ok());
        assert!(select_model("glm-4").is_ok());
        assert!(select_model("mainagent").is_ok());
        assert!(select_model("glm-4.5-air").is_ok());
        assert!(select_model("glm-4-air").is_ok());
        assert!(select_model("subagent").is_ok());
        assert!(select_model("glm-4.5v").is_ok());
        assert!(select_model("glm-4v").is_ok());
    }

    // ── error mapping tests ─────────────────────────────────────────────

    #[test]
    fn maps_stream_decode_failure_to_network_error() {
        let error = ZaiError::Unknown {
            code: 0,
            message: "Stream error: error decoding response body".to_string(),
        };

        let mapped = map_zai_error(error);

        assert!(
            matches!(mapped, LlmError::NetworkError(message) if message == "Stream error: error decoding response body")
        );
    }

    #[test]
    fn keeps_non_stream_unknown_errors_as_api_errors() {
        let error = ZaiError::Unknown {
            code: 0,
            message: "unexpected upstream payload".to_string(),
        };

        let mapped = map_zai_error(error);

        assert!(
            matches!(mapped, LlmError::ApiError(message) if message == "Unknown error [0]: unexpected upstream payload")
        );
    }

    #[test]
    fn detects_stream_transport_errors_case_insensitively() {
        assert!(is_stream_transport_error(" Stream error: connection reset"));
        assert!(is_stream_transport_error(
            "stream error: error decoding response body"
        ));
        assert!(!is_stream_transport_error("unknown error"));
    }
}
