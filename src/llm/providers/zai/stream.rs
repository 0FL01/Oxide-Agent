use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall, ToolCallFunction};
use futures_util::StreamExt;
use std::collections::HashMap;
use tracing::debug;

// Streaming structures for ZAI tool calling with reasoning
#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamChunk {
    pub(super) choices: Vec<ZaiStreamChoice>,
    pub(super) usage: Option<ZaiStreamUsage>,
}

#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamUsage {
    #[serde(rename = "prompt_tokens")]
    pub(super) prompt: u32,
    #[serde(rename = "completion_tokens")]
    pub(super) completion: u32,
    #[serde(rename = "total_tokens")]
    pub(super) total: u32,
}

#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamChoice {
    pub(super) delta: ZaiStreamDelta,
    pub(super) finish_reason: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamDelta {
    pub(super) content: Option<String>,
    pub(super) reasoning_content: Option<String>,
    pub(super) tool_calls: Option<Vec<ZaiStreamToolCall>>,
}

#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamToolCall {
    pub(super) index: usize,
    pub(super) id: Option<String>,
    #[serde(rename = "type")]
    pub(super) _type: Option<String>,
    pub(super) function: Option<ZaiStreamFunction>,
}

#[derive(serde::Deserialize, Debug)]
pub(super) struct ZaiStreamFunction {
    pub(super) name: Option<String>,
    pub(super) arguments: Option<String>,
}

pub(super) async fn process_zai_stream(
    mut stream: impl futures_util::Stream<
            Item = Result<
                eventsource_stream::Event,
                eventsource_stream::EventStreamError<reqwest::Error>,
            >,
        > + Unpin,
) -> Result<ChatResponse, LlmError> {
    let mut reasoning_content = String::new();
    let mut content = String::new();
    let mut final_tool_calls: HashMap<usize, ToolCall> = HashMap::new();
    let mut finish_reason = String::from("unknown");
    let mut usage: Option<TokenUsage> = None;

    while let Some(event_result) = stream.next().await {
        match event_result {
            Ok(event) => {
                // Check for [DONE] marker
                if event.data.trim() == "[DONE]" {
                    break;
                }

                // Parse JSON from event data
                let parsed: ZaiStreamChunk = serde_json::from_str(&event.data)
                    .map_err(|e| LlmError::JsonError(format!("Failed to parse event data: {e}")))?;

                if let Some(choice) = parsed.choices.first() {
                    let delta = &choice.delta;
                    process_stream_delta(
                        delta,
                        &mut reasoning_content,
                        &mut content,
                        &mut final_tool_calls,
                    );

                    // Update finish reason
                    if let Some(ref reason) = choice.finish_reason {
                        finish_reason.clone_from(reason);
                    }
                }

                // Capture usage statistics (usually in last chunk)
                if let Some(ref u) = parsed.usage {
                    usage = Some(TokenUsage {
                        prompt_tokens: u.prompt,
                        completion_tokens: u.completion,
                        total_tokens: u.total,
                    });
                }
            }
            Err(e) => {
                return Err(LlmError::NetworkError(format!("SSE stream error: {e}")));
            }
        }
    }

    debug!(
        "ZAI: Tool call completed (tool_calls: {}, reasoning_len: {}, content_len: {})",
        final_tool_calls.len(),
        reasoning_content.len(),
        content.len()
    );

    // Convert HashMap to Vec, sorted by index
    let mut tool_calls_vec: Vec<_> = final_tool_calls.into_iter().collect();
    tool_calls_vec.sort_by_key(|(index, _)| *index);
    let tool_calls = tool_calls_vec.into_iter().map(|(_, tc)| tc).collect();

    Ok(ChatResponse {
        content: if content.is_empty() {
            None
        } else {
            Some(content)
        },
        tool_calls,
        finish_reason,
        reasoning_content: if reasoning_content.is_empty() {
            None
        } else {
            Some(reasoning_content)
        },
        usage,
    })
}

fn process_stream_delta(
    delta: &ZaiStreamDelta,
    reasoning_content: &mut String,
    content: &mut String,
    final_tool_calls: &mut HashMap<usize, ToolCall>,
) {
    // Collect reasoning/thinking
    if let Some(ref reasoning) = delta.reasoning_content {
        reasoning_content.push_str(reasoning);
    }

    // Collect content
    if let Some(ref text) = delta.content {
        content.push_str(text);
    }

    // Collect tool calls
    if let Some(ref tool_calls) = delta.tool_calls {
        for tc in tool_calls {
            let index = tc.index;
            if let Some(existing) = final_tool_calls.get_mut(&index) {
                // Append to existing tool call
                if let Some(ref func) = tc.function {
                    if let Some(ref args) = func.arguments {
                        existing.function.arguments.push_str(args);
                    }
                }
            } else if let (Some(id), Some(func)) = (&tc.id, &tc.function) {
                // New tool call
                if let Some(ref name) = func.name {
                    final_tool_calls.insert(
                        index,
                        ToolCall {
                            id: id.clone(),
                            function: ToolCallFunction {
                                name: name.clone(),
                                arguments: func.arguments.clone().unwrap_or_default(),
                            },
                            is_recovered: false,
                        },
                    );
                }
            }
        }
    }
}
