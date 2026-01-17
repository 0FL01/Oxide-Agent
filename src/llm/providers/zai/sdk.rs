mod messages;
mod stream;

use super::ZaiProvider;
use crate::llm::{ChatResponse, LlmError, Message, ToolDefinition};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::Serialize;
use zai_rs::model::chat::ChatCompletion;
use zai_rs::model::chat_base_response::ChatCompletionResponse;
use zai_rs::model::chat_message_types::{TextMessage, VisionMessage, VisionRichContent};
use zai_rs::model::chat_models::{GLM4_5_air, GLM4_5v, GLM4_7};
use zai_rs::model::tools::ThinkingType;
use zai_rs::model::traits::{Chat, ModelName, ThinkEnable};
use zai_rs::ZaiError;

use messages::{
    convert_to_text_messages, convert_to_vision_messages, convert_tools, extract_text_content,
};
use stream::stream_text_response;

const ZAI_TEMPERATURE: f32 = 0.95;
const ZAI_IMAGE_MAX_TOKENS: u32 = 4000;

enum ZaiModel {
    Main(GLM4_7),
    Sub(GLM4_5_air),
    Vision(GLM4_5v),
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
            ZaiModel::Vision(model) => {
                let messages =
                    convert_to_vision_messages(system_prompt, history, Some(user_message));
                let client = build_vision_request(
                    model,
                    messages,
                    &self.api_key,
                    self.api_base.as_deref(),
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
            self.api_base.as_deref(),
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
                let mut client = build_text_request(
                    model,
                    messages,
                    &self.api_key,
                    self.api_base.as_deref(),
                    max_tokens,
                )?;
                if !converted_tools.is_empty() {
                    client = client.add_tools(converted_tools);
                }
                let client = client.enable_stream().with_tool_stream(true);
                stream_text_response(client).await
            }
            ZaiModel::Sub(model) => {
                let mut client = build_text_request(
                    model,
                    messages,
                    &self.api_key,
                    self.api_base.as_deref(),
                    max_tokens,
                )?;
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
        let client = build_text_request(
            model,
            messages,
            &self.api_key,
            self.api_base.as_deref(),
            max_tokens,
        )?;
        client.send().await.map_err(map_zai_error)
    }
}

fn select_model(model_id: &str) -> Result<ZaiModel, LlmError> {
    let normalized = model_id.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "glm-4.7" | "glm-4" | "mainagent" => Ok(ZaiModel::Main(GLM4_7 {})),
        "glm-4.5-air" | "glm-4-air" | "subagent" => Ok(ZaiModel::Sub(GLM4_5_air {})),
        "glm-4.5v" | "glm-4v" => Ok(ZaiModel::Vision(GLM4_5v {})),
        _ => Err(LlmError::Unknown(format!(
            "Unsupported ZAI model id: {model_id}"
        ))),
    }
}

fn build_text_request<N>(
    model: N,
    messages: Vec<TextMessage>,
    api_key: &str,
    api_base: Option<&str>,
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
        .with_temperature(ZAI_TEMPERATURE)
        .with_max_tokens(max_tokens)
        .with_thinking(ThinkingType::Enabled);

    if let Some(base) = api_base {
        client = client.with_url(base);
    }

    for message in iter {
        client = client.add_messages(message);
    }

    Ok(client)
}

fn build_vision_request(
    model: GLM4_5v,
    messages: Vec<VisionMessage>,
    api_key: &str,
    api_base: Option<&str>,
    max_tokens: u32,
) -> Result<ChatCompletion<GLM4_5v, VisionMessage>, LlmError> {
    let mut iter = messages.into_iter();
    let first = iter
        .next()
        .ok_or_else(|| LlmError::ApiError("ZAI request has no messages".to_string()))?;

    let mut client = ChatCompletion::new(model, first, api_key.to_string())
        .with_temperature(ZAI_TEMPERATURE)
        .with_max_tokens(max_tokens);

    if let Some(base) = api_base {
        client = client.with_url(base);
    }

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
        ZaiError::RateLimitError { message, .. } => LlmError::RateLimit {
            wait_secs: None,
            message,
        },
        ZaiError::HttpError {
            status: 429,
            message,
        } => LlmError::RateLimit {
            wait_secs: None,
            message,
        },
        ZaiError::NetworkError(err) => LlmError::NetworkError(err.to_string()),
        ZaiError::JsonError(err) => LlmError::JsonError(err.to_string()),
        other => LlmError::ApiError(other.to_string()),
    }
}
