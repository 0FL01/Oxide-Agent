use reqwest::Client as HttpClient;
use serde_json::{Map, Value, json};

use crate::llm::providers::protocol_profiles::RESPONSES_LIKE_TOOL_PROFILE;
use crate::llm::support::http::send_json_request;
use crate::llm::support::media::{image_data_url, image_data_url_with_mime};
use crate::llm::{ChatResponse, LlmError, Message, MessageContentPart, TokenUsage, ToolDefinition};

#[derive(Debug, Clone)]
pub(super) struct ResponsesClient {
    http_client: HttpClient,
    endpoint: String,
    api_key: String,
}

impl ResponsesClient {
    pub(super) fn new(
        http_client: HttpClient,
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            http_client,
            endpoint: endpoint.into(),
            api_key: api_key.into(),
        }
    }

    pub(super) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(super) async fn post_json(&self, body: &Value) -> Result<Value, LlmError> {
        let auth =
            (!self.api_key.trim().is_empty()).then(|| format!("Bearer {}", self.api_key.trim()));
        send_json_request(
            &self.http_client,
            &self.endpoint,
            body,
            auth.as_deref(),
            &[],
        )
        .await
    }
}

pub(super) fn build_text_body(
    system_prompt: &str,
    history: &[Message],
    user_message: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let (instructions, mut input) = prepare_input(system_prompt, history);
    input.push(user_input_item(user_message, &[]));
    build_body(instructions, input, &[], model_id, max_tokens, None)
}

pub(super) fn build_image_body(
    image_bytes: &[u8],
    text_prompt: &str,
    system_prompt: &str,
    model_id: &str,
    max_tokens: u32,
) -> Value {
    let content = vec![
        json!({ "type": "input_text", "text": text_prompt }),
        json!({ "type": "input_image", "image_url": image_data_url(image_bytes) }),
    ];
    build_body(
        system_prompt.trim().to_string(),
        vec![json!({ "role": "user", "content": content })],
        &[],
        model_id,
        max_tokens,
        None,
    )
}

pub(super) fn build_tool_body(
    system_prompt: &str,
    history: &[Message],
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    reasoning_effort: Option<&str>,
) -> Value {
    let (instructions, input) = prepare_input(system_prompt, history);
    build_body(
        instructions,
        input,
        tools,
        model_id,
        max_tokens,
        reasoning_effort,
    )
}

fn build_body(
    instructions: String,
    input: Vec<Value>,
    tools: &[ToolDefinition],
    model_id: &str,
    max_tokens: u32,
    reasoning_effort: Option<&str>,
) -> Value {
    let mut body = json!({
        "model": model_id,
        "instructions": instructions,
        "input": input,
        "max_output_tokens": max_tokens,
        "stream": false,
        "store": false,
    });

    if !tools.is_empty() {
        body["tools"] = json!(prepare_tools(tools));
        body["tool_choice"] = json!("auto");
    }
    if let Some(effort) = reasoning_effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .filter(|effort| !matches!(*effort, "none" | "off" | "disabled"))
    {
        body["reasoning"] = json!({ "effort": effort });
    }
    body
}

fn prepare_input(system_prompt: &str, history: &[Message]) -> (String, Vec<Value>) {
    let mut instructions = system_prompt.trim().to_string();
    let mut input = Vec::new();

    for message in history {
        match message.role.as_str() {
            "system" => append_instructions(&mut instructions, &message.content),
            "assistant" => {
                if !message.content.trim().is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": message.content }],
                    }));
                }
                if let Some(tool_calls) = &message.tool_calls {
                    input.extend(tool_calls.iter().filter_map(|tool_call| {
                        RESPONSES_LIKE_TOOL_PROFILE
                            .encode_tool_call(tool_call)
                            .and_then(|call| call.into_responses_like())
                            .map(|call| {
                                let mut item = Map::from_iter([
                                    ("type".to_string(), json!("function_call")),
                                    ("call_id".to_string(), json!(call.call_id)),
                                    ("name".to_string(), json!(call.name)),
                                    ("arguments".to_string(), json!(call.arguments)),
                                ]);
                                if let Some(item_id) = call.item_id {
                                    item.insert("id".to_string(), json!(item_id));
                                }
                                Value::Object(item)
                            })
                    }));
                }
            }
            "tool" => {
                if let Some(result) = RESPONSES_LIKE_TOOL_PROFILE
                    .encode_tool_result(message)
                    .and_then(|result| result.into_responses_like())
                {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": result.call_id,
                        "output": result.output,
                    }));
                }
            }
            _ => input.push(user_input_item(&message.content, &message.content_parts)),
        }
    }

    (instructions, input)
}

fn append_instructions(instructions: &mut String, additional: &str) {
    let additional = additional.trim();
    if additional.is_empty() {
        return;
    }
    if !instructions.is_empty() {
        instructions.push_str("\n\n");
    }
    instructions.push_str(additional);
}

fn user_input_item(text: &str, parts: &[MessageContentPart]) -> Value {
    let mut content = Vec::new();
    if !text.is_empty() || parts.is_empty() {
        content.push(json!({ "type": "input_text", "text": text }));
    }
    content.extend(parts.iter().map(|part| match part {
        MessageContentPart::Image { mime_type, bytes } => json!({
            "type": "input_image",
            "image_url": image_data_url_with_mime(bytes, mime_type),
        }),
    }));
    json!({ "role": "user", "content": content })
}

fn prepare_tools(tools: &[ToolDefinition]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect()
}

pub(super) fn parse_response(response: Value) -> Result<ChatResponse, LlmError> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown Responses API error");
        return Err(LlmError::api_error(format!(
            "OpenCode Responses API error: {message}"
        )));
    }

    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| LlmError::api_error("OpenCode Responses response is missing output"))?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();

    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if part.get("type").and_then(Value::as_str) == Some("output_text")
                            && let Some(part_text) = part.get("text").and_then(Value::as_str)
                        {
                            text.push_str(part_text);
                        }
                    }
                }
            }
            Some("function_call") => {
                let name = item.get("name").and_then(Value::as_str).ok_or_else(|| {
                    LlmError::api_error("Responses function call is missing name")
                })?;
                let arguments = item
                    .get("arguments")
                    .and_then(|value| {
                        value
                            .as_str()
                            .map(ToString::to_string)
                            .or_else(|| serde_json::to_string(value).ok())
                    })
                    .unwrap_or_else(|| "{}".to_string());
                let item_id = item.get("id").and_then(Value::as_str);
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .filter(|call_id| !call_id.trim().is_empty());
                let tool_call = match call_id {
                    Some(call_id) => RESPONSES_LIKE_TOOL_PROFILE
                        .inbound_provider_tool_call(call_id, item_id, name, arguments),
                    None => {
                        RESPONSES_LIKE_TOOL_PROFILE.inbound_uncorrelated_tool_call(name, arguments)
                    }
                };
                tool_calls.push(tool_call);
            }
            _ => {}
        }
    }

    let content = (!text.is_empty()).then_some(text);
    if content.is_none() && tool_calls.is_empty() {
        return Err(LlmError::EmptyResponse(format!(
            "OpenCode Responses returned no text or tool calls (status={})",
            response
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        )));
    }

    let finish_reason = response
        .get("incomplete_details")
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if tool_calls.is_empty() {
                "stop".to_string()
            } else {
                "tool_calls".to_string()
            }
        });

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason,
        reasoning_content: None,
        usage: response.get("usage").and_then(parse_usage),
    })
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        prompt_tokens: value.get("input_tokens")?.as_u64()? as u32,
        completion_tokens: value.get("output_tokens")?.as_u64()? as u32,
        total_tokens: value.get("total_tokens")?.as_u64()? as u32,
        cached_tokens: value
            .get("input_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(Value::as_u64)
            .map(|tokens| tokens as u32),
        cache_creation_tokens: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{build_image_body, build_tool_body, parse_response};
    use crate::llm::{
        Message, MessageContentPart, ToolCall, ToolCallCorrelation, ToolCallFunction,
        ToolDefinition,
    };
    use serde_json::json;

    fn read_file_tool() -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: json!({ "type": "object" }),
        }
    }

    #[test]
    fn tool_body_preserves_responses_correlation_images_and_reasoning() {
        let correlation = ToolCallCorrelation::new("invoke-1")
            .with_provider_tool_call_id("call-1")
            .with_provider_item_id("item-1");
        let history = vec![
            Message::user("inspect").with_user_content_parts(vec![MessageContentPart::image(
                "image/png",
                b"png".to_vec(),
            )]),
            Message::assistant_with_tools(
                "",
                vec![
                    ToolCall::new(
                        "invoke-1",
                        ToolCallFunction {
                            name: "read_file".to_string(),
                            arguments: "{}".to_string(),
                        },
                        false,
                    )
                    .with_correlation(correlation.clone()),
                ],
            ),
            Message::tool_with_correlation("invoke-1", correlation, "read_file", "done"),
        ];

        let body = build_tool_body(
            "system",
            &history,
            &[read_file_tool()],
            "muse-spark-1.3-contributor",
            128,
            Some("high"),
        );

        assert_eq!(body["stream"], json!(false));
        assert_eq!(body["reasoning"]["effort"], json!("high"));
        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_image"));
        assert_eq!(body["input"][1]["id"], json!("item-1"));
        assert_eq!(body["input"][1]["call_id"], json!("call-1"));
        assert_eq!(body["input"][2]["call_id"], json!("call-1"));
        assert_eq!(body["tools"][0]["name"], json!("read_file"));
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn image_body_uses_responses_input_image() {
        let body = build_image_body(
            b"png",
            "describe",
            "system",
            "muse-spark-1.2-contributor",
            64,
        );

        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_image"));
        assert_eq!(
            body["input"][0]["content"][1]["image_url"],
            json!("data:image/jpeg;base64,cG5n")
        );
    }

    #[test]
    fn parses_text_tool_calls_usage_and_wire_ids() {
        let parsed = parse_response(json!({
            "status": "completed",
            "output": [
                {
                    "type": "message",
                    "content": [{ "type": "output_text", "text": "answer" }]
                },
                {
                    "type": "function_call",
                    "id": "item-2",
                    "call_id": "call-2",
                    "name": "read_file",
                    "arguments": "{}"
                }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15,
                "input_tokens_details": { "cached_tokens": 4 }
            }
        }))
        .expect("response parses");

        assert_eq!(parsed.content.as_deref(), Some("answer"));
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.tool_calls[0].wire_tool_call_id(), "call-2");
        assert_eq!(
            parsed.tool_calls[0]
                .correlation()
                .provider_item_id
                .expect("provider item id")
                .as_str(),
            "item-2"
        );
        assert_eq!(parsed.usage.expect("usage").cached_tokens, Some(4));
    }
}
