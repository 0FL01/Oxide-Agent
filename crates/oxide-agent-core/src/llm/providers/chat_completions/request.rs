//! Request-body construction for the shared Chat Completions wire path.

use super::profile::{
    ChatCompletionsProfile, ChatMessageLayoutPolicy, ChatReasoningPolicy, ChatStreamingPolicy,
    ChatThinkingPolicy, ChatToolChoicePolicy, JsonModePolicy, ModelMatchPolicy,
};
use crate::llm::providers::protocol_profiles::CHAT_LIKE_TOOL_PROFILE;
use crate::llm::support::media;
use crate::llm::{Message, MessageContentPart, ToolDefinition};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ChatCompletionsRequestPlan {
    pub(crate) profile: ChatCompletionsProfile,
}

impl ChatCompletionsRequestPlan {
    #[must_use]
    pub(crate) const fn new(profile: ChatCompletionsProfile) -> Self {
        Self { profile }
    }
}

pub(crate) trait ChatToolCallIdMapper {
    fn map_tool_call_id(&mut self, id: &str) -> String;
}

#[cfg(feature = "llm-openai-base")]
impl ChatToolCallIdMapper for crate::llm::providers::openai_base::ToolCallIdMapper {
    fn map_tool_call_id(&mut self, id: &str) -> String {
        self.mistral_id_for(id)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChatRequestOptions<'a> {
    pub(crate) profile: ChatCompletionsProfile,
    pub(crate) allow_native_image_parts: bool,
    pub(crate) allow_native_image_parts_for_tool_results: bool,
    pub(crate) require_non_empty_tool_result_content: bool,
    pub(crate) model_supports_reasoning: Option<bool>,
    pub(crate) reasoning_disabled: bool,
    pub(crate) reasoning_effort: Option<&'a str>,
}

impl<'a> ChatRequestOptions<'a> {
    #[must_use]
    pub(crate) const fn new(profile: ChatCompletionsProfile) -> Self {
        Self {
            profile,
            allow_native_image_parts: true,
            allow_native_image_parts_for_tool_results: true,
            require_non_empty_tool_result_content: false,
            model_supports_reasoning: None,
            reasoning_disabled: false,
            reasoning_effort: None,
        }
    }

    #[must_use]
    pub(crate) const fn with_native_image_parts(mut self, allow: bool) -> Self {
        self.allow_native_image_parts = allow;
        self.allow_native_image_parts_for_tool_results = allow;
        self
    }

    #[must_use]
    pub(crate) const fn with_native_image_parts_for_tool_results(mut self, allow: bool) -> Self {
        self.allow_native_image_parts_for_tool_results = allow;
        self
    }

    #[must_use]
    pub(crate) const fn with_non_empty_tool_result_content(mut self, require: bool) -> Self {
        self.require_non_empty_tool_result_content = require;
        self
    }

    #[must_use]
    pub(crate) const fn with_model_supports_reasoning(mut self, supports: bool) -> Self {
        self.model_supports_reasoning = Some(supports);
        self
    }

    #[must_use]
    pub(crate) const fn with_reasoning_disabled(mut self, disabled: bool) -> Self {
        self.reasoning_disabled = disabled;
        self
    }

    #[must_use]
    pub(crate) const fn with_reasoning_effort(mut self, effort: Option<&'a str>) -> Self {
        self.reasoning_effort = effort;
        self
    }
}

#[must_use]
pub(crate) fn build_text_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
    options: ChatRequestOptions<'_>,
    tool_id_mapper: Option<&mut dyn ChatToolCallIdMapper>,
) -> Value {
    let mut messages = prepare_messages(system_prompt, history, options, tool_id_mapper);
    messages.push(json!({
        "role": "user",
        "content": user_message,
    }));

    let supports_reasoning = model_supports_reasoning(options, model_id);
    let mut body = json!({
        "model": model_id,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": if supports_reasoning {
            options.profile.temperatures.reasoning
        } else {
            options.profile.temperatures.chat
        },
    });
    apply_streaming_policy(&mut body, options.profile, false);
    apply_reasoning_policy(&mut body, model_id, options, supports_reasoning);
    body
}

#[must_use]
pub(crate) fn build_tool_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    json_mode: bool,
    options: ChatRequestOptions<'_>,
    tool_id_mapper: Option<&mut dyn ChatToolCallIdMapper>,
) -> Value {
    let openai_tools = prepare_tools_json(tools);
    let has_tools = !openai_tools.is_empty();
    let native_json_mode = should_use_native_json_mode(options.profile, json_mode, has_tools);
    let supports_reasoning = model_supports_reasoning(options, model_id);
    let effective_temperature = if supports_reasoning {
        temperature.unwrap_or(options.profile.temperatures.reasoning)
    } else {
        temperature.unwrap_or(options.profile.temperatures.tools)
    };

    let mut body = json!({
        "model": model_id,
        "messages": prepare_messages(system_prompt, history, options, tool_id_mapper),
        "max_tokens": max_tokens,
        "temperature": effective_temperature,
    });

    apply_streaming_policy(&mut body, options.profile, native_json_mode);

    if has_tools {
        body["tools"] = json!(openai_tools);
        if matches!(
            options.profile.tool_choice,
            ChatToolChoicePolicy::AutoWhenToolsExist
        ) {
            body["tool_choice"] = json!("auto");
        }
        if options.profile.require_parameters_with_tools {
            body["provider"] = json!({ "require_parameters": true });
        }
    }

    if let Some(parallel) = options.profile.parallel_tool_calls
        && (!options.profile.parallel_tool_calls_only_with_tools || has_tools)
    {
        body["parallel_tool_calls"] = json!(parallel);
    }

    if native_json_mode {
        body["response_format"] = json!({ "type": "json_object" });
    }

    apply_thinking_policy(&mut body, options.profile, native_json_mode);
    apply_reasoning_policy(&mut body, model_id, options, supports_reasoning);

    body
}

#[must_use]
pub(crate) fn build_image_body(
    image_bytes: &[u8],
    mime_type: Option<&str>,
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
    options: ChatRequestOptions<'_>,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "messages": [
            {"role": "system", "content": system_prompt},
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text_prompt},
                    {
                        "type": "image_url",
                        "image_url": {"url": image_data_url_with_optional_mime(image_bytes, mime_type)}
                    }
                ]
            }
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    });
    apply_streaming_policy(&mut body, options.profile, false);
    apply_reasoning_policy(
        &mut body,
        model_id,
        options,
        model_supports_reasoning(options, model_id),
    );
    body
}

#[must_use]
pub(crate) fn build_audio_body(
    audio_bytes: &[u8],
    mime_type: &str,
    text_prompt: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
) -> Value {
    json!({
        "model": model_id,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": text_prompt},
                    {
                        "type": "input_audio",
                        "input_audio": {
                            "data": media::base64_data(audio_bytes),
                            "format": media::audio_input_format(mime_type)
                        }
                    }
                ]
            }
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    })
}

#[must_use]
pub(crate) fn build_video_body(
    video_bytes: &[u8],
    mime_type: &str,
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
    max_tokens: u32,
    temperature: f32,
) -> Value {
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
                        "video_url": {"url": media::data_url(mime_type, video_bytes)}
                    }
                ]
            }
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    })
}

#[must_use]
pub(crate) fn prepare_messages(
    system_prompt: &str,
    history: &[Message],
    options: ChatRequestOptions<'_>,
    tool_id_mapper: Option<&mut dyn ChatToolCallIdMapper>,
) -> Vec<Value> {
    match options.profile.message_layout {
        ChatMessageLayoutPolicy::GenericOpenAI => prepare_generic_messages(
            system_prompt,
            history,
            options.allow_native_image_parts,
            options.allow_native_image_parts_for_tool_results,
            options.profile.include_empty_system_message,
            options.profile.require_reasoning_content_on_tool_calls,
            options.require_non_empty_tool_result_content,
        ),
        ChatMessageLayoutPolicy::MistralStrict => prepare_mistral_messages(
            system_prompt,
            history,
            tool_id_mapper,
            options.allow_native_image_parts_for_tool_results,
            options.profile.require_reasoning_content_on_tool_calls,
            options.require_non_empty_tool_result_content,
        ),
    }
}

#[must_use]
pub(crate) fn prepare_tools_json(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect()
}

#[must_use]
pub(crate) fn image_data_url(image_bytes: &[u8]) -> String {
    media::image_data_url(image_bytes)
}

#[must_use]
pub(crate) fn image_data_url_with_mime(image_bytes: &[u8], mime_type: &str) -> String {
    media::image_data_url_with_mime(image_bytes, mime_type)
}

#[must_use]
pub(crate) fn normalized_image_mime_type(mime_type: &str, image_bytes: &[u8]) -> String {
    media::normalized_image_mime_type(mime_type, image_bytes)
}

#[must_use]
pub(crate) fn infer_image_mime_type(image_bytes: &[u8]) -> &'static str {
    media::infer_image_mime_type(image_bytes)
}

#[must_use]
pub(crate) fn should_use_native_json_mode(
    profile: ChatCompletionsProfile,
    json_mode: bool,
    has_tools: bool,
) -> bool {
    matches!(profile.json_mode, JsonModePolicy::Standard) && json_mode && !has_tools
}

fn prepare_generic_messages(
    system_prompt: &str,
    history: &[Message],
    allow_native_image_parts: bool,
    allow_native_image_parts_for_tool_results: bool,
    include_empty_system_message: bool,
    require_reasoning_content: bool,
    require_non_empty_tool_result_content: bool,
) -> Vec<Value> {
    let mut messages = Vec::with_capacity(history.len() + 1);

    if include_empty_system_message || !system_prompt.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": system_prompt,
        }));
    }

    for msg in history {
        match msg.role.as_str() {
            "system" => {
                if include_empty_system_message || !msg.content.trim().is_empty() {
                    messages.push(json!({
                        "role": "system",
                        "content": msg.content,
                    }));
                }
            }
            "assistant" => {
                let mut mapper = None;
                messages.push(assistant_message(
                    msg,
                    &mut mapper,
                    require_reasoning_content,
                ));
            }
            "tool" => {
                let mut mapper = None;
                if let Some(tool_message) = tool_result_message(
                    msg,
                    &mut mapper,
                    false,
                    allow_native_image_parts_for_tool_results,
                    require_non_empty_tool_result_content,
                ) {
                    messages.push(tool_message);
                }
            }
            _ => messages.push(json!({
                "role": "user",
                "content": user_message_content(msg, allow_native_image_parts),
            })),
        }
    }

    messages
}

fn prepare_mistral_messages(
    system_prompt: &str,
    history: &[Message],
    mut mapper: Option<&mut dyn ChatToolCallIdMapper>,
    allow_native_image_parts_for_tool_results: bool,
    require_reasoning_content: bool,
    require_non_empty_tool_result_content: bool,
) -> Vec<Value> {
    let mut history_systems = Vec::new();
    let mut other_messages = Vec::new();

    for msg in history {
        match msg.role.as_str() {
            "system" => history_systems.push(json!({
                "role": "system",
                "content": msg.content,
            })),
            "assistant" => other_messages.push(assistant_message(
                msg,
                &mut mapper,
                require_reasoning_content,
            )),
            "tool" => {
                if let Some(tool_message) = tool_result_message(
                    msg,
                    &mut mapper,
                    true,
                    allow_native_image_parts_for_tool_results,
                    require_non_empty_tool_result_content,
                ) {
                    other_messages.push(tool_message);
                }
            }
            _ => other_messages.push(json!({
                "role": "user",
                "content": msg.content,
            })),
        }
    }

    let mut messages = Vec::with_capacity(history_systems.len() + 1 + other_messages.len());
    messages.extend(history_systems);
    messages.push(json!({
        "role": "system",
        "content": system_prompt,
    }));
    messages.extend(other_messages);
    messages
}

fn assistant_message(
    msg: &Message,
    mapper: &mut Option<&mut dyn ChatToolCallIdMapper>,
    require_reasoning_content: bool,
) -> Value {
    // Per the OpenAI Chat Completions spec, assistant messages that carry
    // tool_calls should have `content: null` when there is no text. Some
    // providers (e.g. Xiaomi MiMo) reject `"content": ""` with a 400
    // "text is not set" error in this case. When content is non-empty, it
    // is always sent as a string. When content is empty but there are no
    // tool_calls, we keep the empty string (existing behavior for pure
    // text assistant messages).
    let has_tool_calls = msg
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty());
    let content = if msg.content.is_empty() && has_tool_calls {
        Value::Null
    } else {
        json!(msg.content)
    };
    let mut message = json!({
        "role": "assistant",
        "content": content,
    });

    // Some reasoning-capable providers (e.g. Xiaomi MiMo, DeepSeek) require
    // a `reasoning_content` field on assistant messages that carry
    // tool_calls, even when empty. Omitting it causes a 400 "text is not
    // set" / "Param Incorrect" error on subsequent requests. When the
    // profile requires it and the message has tool_calls, we always
    // include the field — using the stored reasoning if non-empty, or an
    // empty string otherwise. Providers that ignore unknown fields are not
    // affected (the field is only emitted when the profile opts in).
    if has_tool_calls && require_reasoning_content {
        let reasoning = msg
            .reasoning_content
            .as_deref()
            .filter(|reasoning| !reasoning.trim().is_empty())
            .unwrap_or("");
        message["reasoning_content"] = json!(reasoning);
    } else if let Some(reasoning_content) = msg
        .reasoning_content
        .as_deref()
        .filter(|reasoning| !reasoning.trim().is_empty())
    {
        message["reasoning_content"] = json!(reasoning_content);
    }
    if let Some(tool_calls) = &msg.tool_calls {
        let api_tool_calls: Vec<Value> = tool_calls
            .iter()
            .filter_map(|tool_call| {
                CHAT_LIKE_TOOL_PROFILE
                    .encode_tool_call(tool_call)
                    .and_then(|call| call.into_chat_like())
                    .map(|call| {
                        let id = map_id(mapper, &call.id);
                        json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments,
                            }
                        })
                    })
            })
            .collect();

        if !api_tool_calls.is_empty() {
            message["tool_calls"] = json!(api_tool_calls);
        }
    }
    message
}

fn tool_result_message(
    msg: &Message,
    mapper: &mut Option<&mut dyn ChatToolCallIdMapper>,
    include_name: bool,
    allow_native_image_parts: bool,
    require_non_empty_content: bool,
) -> Option<Value> {
    let result = CHAT_LIKE_TOOL_PROFILE
        .encode_tool_result(msg)
        .and_then(|result| result.into_chat_like())?;
    let id = map_id(mapper, &result.tool_call_id);
    let text_content = tool_result_text_content(
        &result.content,
        &msg.content_parts,
        require_non_empty_content,
    );
    let content = if allow_native_image_parts && !msg.content_parts.is_empty() {
        let mut parts = Vec::new();
        if !text_content.is_empty() {
            parts.push(json!({
                "type": "text",
                "text": text_content,
            }));
        }
        for part in &msg.content_parts {
            match part {
                MessageContentPart::Image { mime_type, bytes } if !bytes.is_empty() => {
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": media::image_data_url_with_mime(bytes, mime_type),
                        },
                    }));
                }
                MessageContentPart::Image { .. } => {}
            }
        }
        if parts.is_empty() {
            json!(text_content)
        } else {
            json!(parts)
        }
    } else {
        json!(text_content)
    };
    let mut message = json!({
        "role": "tool",
        "tool_call_id": id,
        "content": content,
    });
    if include_name && let Some(name) = result.name {
        message["name"] = json!(name);
    }
    Some(message)
}

fn tool_result_text_content(
    content: &str,
    content_parts: &[MessageContentPart],
    require_non_empty: bool,
) -> String {
    if !content.is_empty() || !require_non_empty {
        return content.to_string();
    }

    if content_parts.iter().any(|part| match part {
        MessageContentPart::Image { bytes, .. } => !bytes.is_empty(),
    }) {
        "Tool returned image attachment(s).".to_string()
    } else {
        "Tool returned no textual content.".to_string()
    }
}

fn map_id(mapper: &mut Option<&mut dyn ChatToolCallIdMapper>, id: &str) -> String {
    match mapper.as_deref_mut() {
        Some(mapper) => mapper.map_tool_call_id(id),
        None => id.to_string(),
    }
}

fn user_message_content(message: &Message, allow_native_image_parts: bool) -> Value {
    if !allow_native_image_parts || message.content_parts.is_empty() {
        return json!(message.content);
    }

    let mut parts = Vec::new();
    if !message.content.is_empty() {
        parts.push(json!({
            "type": "text",
            "text": message.content,
        }));
    }

    for part in &message.content_parts {
        match part {
            MessageContentPart::Image { mime_type, bytes } if !bytes.is_empty() => {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": image_data_url_with_mime(bytes, mime_type),
                    },
                }));
            }
            MessageContentPart::Image { .. } => {}
        }
    }

    if parts.is_empty() {
        json!(message.content)
    } else {
        json!(parts)
    }
}

fn apply_streaming_policy(
    body: &mut Value,
    profile: ChatCompletionsProfile,
    native_json_mode: bool,
) {
    if !profile.include_stream_field {
        return;
    }
    match profile.streaming {
        ChatStreamingPolicy::NonStreaming => body["stream"] = json!(false),
        ChatStreamingPolicy::ZaiUnlessNativeJsonMode => body["stream"] = json!(!native_json_mode),
    }
}

fn apply_thinking_policy(
    body: &mut Value,
    profile: ChatCompletionsProfile,
    native_json_mode: bool,
) {
    match profile.thinking {
        ChatThinkingPolicy::None => {}
        ChatThinkingPolicy::ZaiEnabledUnlessJsonMode => {
            let thinking_type = if native_json_mode {
                "disabled"
            } else {
                "enabled"
            };
            body["thinking"] = json!({ "type": thinking_type });
        }
    }
}

fn apply_reasoning_policy(
    body: &mut Value,
    model_id: &str,
    options: ChatRequestOptions<'_>,
    supports_reasoning: bool,
) {
    if options.reasoning_disabled || !supports_reasoning {
        return;
    }

    match options.profile.reasoning {
        ChatReasoningPolicy::None => {}
        ChatReasoningPolicy::Mistral { default_effort, .. } => {
            body["reasoning_effort"] = json!(options.reasoning_effort.unwrap_or(default_effort));
        }
        ChatReasoningPolicy::OpenCodeGo { default_effort } => {
            let _ = model_id;
            body["reasoning_effort"] = json!(options.reasoning_effort.unwrap_or(default_effort));
        }
    }
}

fn model_supports_reasoning(options: ChatRequestOptions<'_>, model_id: &str) -> bool {
    if let Some(supports) = options.model_supports_reasoning {
        return supports;
    }
    match options.profile.reasoning {
        ChatReasoningPolicy::None => false,
        ChatReasoningPolicy::OpenCodeGo { .. } => false,
        ChatReasoningPolicy::Mistral { model_match, .. } => match model_match {
            ModelMatchPolicy::None => false,
            ModelMatchPolicy::CaseInsensitiveContains(needle) => model_id
                .to_ascii_lowercase()
                .contains(needle.to_ascii_lowercase().as_str()),
        },
    }
}

fn image_data_url_with_optional_mime(image_bytes: &[u8], mime_type: Option<&str>) -> String {
    match mime_type {
        Some(mime_type) => image_data_url_with_mime(image_bytes, mime_type),
        None => image_data_url(image_bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Message, MessageContentPart, ToolCall, ToolCallFunction, ToolDefinition};

    struct PrefixMapper;

    impl ChatToolCallIdMapper for PrefixMapper {
        fn map_tool_call_id(&mut self, id: &str) -> String {
            format!("m{id}")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(9)
                .collect()
        }
    }

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

    fn sample_tool_call(id: &str) -> ToolCall {
        ToolCall::new(
            id,
            ToolCallFunction {
                name: "get_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
            },
            false,
        )
    }

    #[test]
    fn chat_completions_generic_tool_request_matches_openai_base_legacy() {
        let history = vec![
            Message::system("History system"),
            Message::assistant_with_tools("", vec![sample_tool_call("call_123")]),
            Message::tool("call_123", "get_weather", "sunny"),
            Message::user("next").with_user_content_parts(vec![MessageContentPart::image(
                "image/png",
                vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'],
            )]),
        ];

        let body = build_tool_body(
            "System",
            &history,
            &[sample_tool()],
            "gpt-4o",
            1024,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::generic()),
            None,
        );

        assert_eq!(body["model"], json!("gpt-4o"));
        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["tool_choice"], json!("auto"));
        assert_eq!(body["tools"][0]["function"]["name"], json!("get_weather"));
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["id"],
            json!("call_123")
        );
        assert_eq!(body["messages"][3]["tool_call_id"], json!("call_123"));
        assert_eq!(
            body["messages"][4]["content"][1]["type"],
            json!("image_url")
        );
        assert!(body.get("parallel_tool_calls").is_none());
    }

    #[test]
    fn chat_completions_mistral_request_uses_strict_layout_and_mapped_ids() {
        let history = vec![
            Message::system("History system"),
            Message::assistant_with_tools("", vec![sample_tool_call("call_abcdefghi")]),
            Message::tool("call_abcdefghi", "get_weather", "sunny"),
        ];
        let mut mapper = PrefixMapper;

        let body = build_tool_body(
            "Main system",
            &history,
            &[sample_tool()],
            "mistral-small-2603",
            2048,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::mistral()),
            Some(&mut mapper),
        );

        assert_eq!(body["messages"][0]["content"], json!("History system"));
        assert_eq!(body["messages"][1]["content"], json!("Main system"));
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["id"],
            json!("mcallabcd")
        );
        assert_eq!(body["messages"][3]["tool_call_id"], json!("mcallabcd"));
        assert_eq!(body["messages"][3]["name"], json!("get_weather"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn chat_completions_generic_tool_request_includes_image_content_parts() {
        let mut tool_message = Message::tool("call_123", "browser_observe", "observed");
        tool_message.content_parts = vec![MessageContentPart::image(
            "image/png",
            vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'],
        )];
        let history = vec![
            Message::system("History system"),
            Message::assistant_with_tools("", vec![sample_tool_call("call_123")]),
            tool_message,
        ];

        let body = build_tool_body(
            "System",
            &history,
            &[sample_tool()],
            "gpt-4o",
            1024,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::generic()),
            None,
        );

        assert_eq!(body["messages"][3]["role"], json!("tool"));
        assert_eq!(body["messages"][3]["tool_call_id"], json!("call_123"));
        let content = body["messages"][3]["content"]
            .as_array()
            .expect("content is array");
        assert_eq!(content[0]["type"], json!("text"));
        assert_eq!(content[0]["text"], json!("observed"));
        assert_eq!(content[1]["type"], json!("image_url"));
        assert!(
            content[1]["image_url"]["url"]
                .as_str()
                .expect("image_url url is a string")
                .starts_with("data:image/png;base64,")
        );
    }

    #[test]
    fn chat_completions_can_keep_user_images_while_tool_results_are_text_only() {
        let mut tool_message = Message::tool("call_123", "browser_observe", "");
        tool_message.content_parts = vec![MessageContentPart::image(
            "image/png",
            vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'],
        )];
        let user =
            Message::user("inspect this").with_user_content_parts(vec![MessageContentPart::image(
                "image/png",
                b"png".to_vec(),
            )]);
        let history = vec![
            Message::assistant_with_tools("", vec![sample_tool_call("call_123")]),
            tool_message,
            user,
        ];

        let body = build_tool_body(
            "System",
            &history,
            &[sample_tool()],
            "strict-vision-model",
            1024,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::generic())
                .with_native_image_parts(true)
                .with_native_image_parts_for_tool_results(false)
                .with_non_empty_tool_result_content(true),
            None,
        );

        assert_eq!(
            body["messages"][2]["content"],
            json!("Tool returned image attachment(s).")
        );
        assert!(body["messages"][2]["content"].is_string());
        let user_content = body["messages"][3]["content"]
            .as_array()
            .expect("user image content remains native");
        assert_eq!(user_content[0]["type"], json!("text"));
        assert_eq!(user_content[1]["type"], json!("image_url"));
    }

    #[test]
    fn chat_completions_strict_tool_result_empty_text_gets_stable_projection() {
        let history = vec![
            Message::assistant_with_tools("", vec![sample_tool_call("call_123")]),
            Message::tool("call_123", "noop", ""),
        ];

        let body = build_tool_body(
            "System",
            &history,
            &[sample_tool()],
            "strict-text-model",
            1024,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::generic())
                .with_non_empty_tool_result_content(true),
            None,
        );

        assert_eq!(
            body["messages"][2]["content"],
            json!("Tool returned no textual content.")
        );
    }

    #[test]
    fn chat_completions_zai_native_json_disables_streaming_thinking_conflict() {
        let body = build_tool_body(
            "System",
            &[],
            &[],
            "glm-4.6",
            1024,
            None,
            true,
            ChatRequestOptions::new(ChatCompletionsProfile::zai()),
            None,
        );

        assert_eq!(body["response_format"], json!({"type": "json_object"}));
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["stream"], json!(false));
    }

    #[test]
    fn chat_completions_openrouter_tool_request_sets_require_parameters() {
        let body = build_tool_body(
            "System",
            &[],
            &[sample_tool()],
            "openai/gpt-4o",
            1024,
            Some(0.2),
            true,
            ChatRequestOptions::new(ChatCompletionsProfile::openrouter())
                .with_native_image_parts(false),
            None,
        );

        assert_eq!(body["provider"], json!({"require_parameters": true}));
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.get("stream").is_none());
    }

    #[test]
    fn chat_completions_opencode_openai_body_preserves_reasoning_effort() {
        let body = build_tool_body(
            "System",
            &[],
            &[sample_tool()],
            "deepseek-v4-flash",
            32_000,
            None,
            false,
            ChatRequestOptions::new(ChatCompletionsProfile::opencode_go())
                .with_model_supports_reasoning(true)
                .with_reasoning_effort(Some("medium")),
            None,
        );

        assert_eq!(body["model"], json!("deepseek-v4-flash"));
        assert_eq!(body["reasoning_effort"], json!("medium"));
        assert_eq!(body["parallel_tool_calls"], json!(true));
        assert_eq!(body["tool_choice"], json!("auto"));
    }

    #[test]
    fn assistant_message_with_empty_content_and_tool_calls_sends_null_content() {
        // MiMo and other strict OpenAI-compatible providers reject
        // `"content": ""` for tool-only assistant messages with a 400
        // "text is not set". The spec says content should be null.
        let msg = Message::assistant_with_tools("", vec![sample_tool_call("call_1")]);
        let message = assistant_message(&msg, &mut None, false);
        assert_eq!(message["role"], json!("assistant"));
        assert!(
            message["content"].is_null(),
            "content should be null for empty tool-only assistant message"
        );
        assert!(message["tool_calls"].is_array());
    }

    #[test]
    fn assistant_message_with_text_and_tool_calls_sends_string_content() {
        let msg = Message::assistant_with_tools("thinking...", vec![sample_tool_call("call_1")]);
        let message = assistant_message(&msg, &mut None, false);
        assert_eq!(message["content"], json!("thinking..."));
        assert!(message["tool_calls"].is_array());
    }

    #[test]
    fn assistant_message_with_empty_content_and_no_tool_calls_keeps_empty_string() {
        let msg = Message::assistant("");
        let message = assistant_message(&msg, &mut None, false);
        assert_eq!(message["content"], json!(""));
        assert!(message.get("tool_calls").is_none());
    }

    #[test]
    fn assistant_message_requires_reasoning_content_with_empty_reasoning() {
        // MiMo/DeepSeek require reasoning_content on tool-call assistant
        // messages even when empty. When require_reasoning_content is true
        // and the message has tool_calls, the field must be present.
        let msg = Message::assistant_with_tools("", vec![sample_tool_call("call_1")]);
        let message = assistant_message(&msg, &mut None, true);
        assert_eq!(message["reasoning_content"], json!(""));
        assert!(message["tool_calls"].is_array());
    }

    #[test]
    fn assistant_message_requires_reasoning_content_with_nonempty_reasoning() {
        let msg = Message::assistant_with_tools_and_reasoning(
            "",
            Some("step-by-step analysis".to_string()),
            vec![sample_tool_call("call_1")],
        );
        let message = assistant_message(&msg, &mut None, true);
        assert_eq!(message["reasoning_content"], json!("step-by-step analysis"));
    }

    #[test]
    fn assistant_message_without_requirement_omits_empty_reasoning_content() {
        let msg = Message::assistant_with_tools("", vec![sample_tool_call("call_1")]);
        let message = assistant_message(&msg, &mut None, false);
        assert!(
            message.get("reasoning_content").is_none(),
            "reasoning_content should be absent when not required and empty"
        );
    }

    #[test]
    fn assistant_message_without_tool_calls_never_includes_reasoning_content_when_empty() {
        let mut msg = Message::assistant("");
        msg.reasoning_content = Some(String::new());
        let message = assistant_message(&msg, &mut None, true);
        assert!(
            message.get("reasoning_content").is_none(),
            "reasoning_content should not be force-included without tool_calls"
        );
    }
}
