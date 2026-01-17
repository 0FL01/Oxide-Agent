use super::map_zai_error;
use crate::llm::{ChatResponse, LlmError, TokenUsage, ToolCall, ToolCallFunction};
use futures_util::StreamExt;
use serde::Serialize;
use std::collections::BTreeMap;
use zai_rs::model::chat::ChatCompletion;
use zai_rs::model::chat_base_response::{ToolCallMessage, Usage};
use zai_rs::model::chat_message_types::TextMessage;
use zai_rs::model::traits::{Chat, ModelName};
use zai_rs::model::StreamChatLikeExt;

struct PendingToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

pub(super) async fn stream_text_response<N>(
    mut client: ChatCompletion<N, TextMessage, zai_rs::model::traits::StreamOn>,
) -> Result<ChatResponse, LlmError>
where
    N: ModelName + Chat + Serialize,
    (N, TextMessage): zai_rs::model::traits::Bounded,
{
    let mut stream = client.to_stream().await.map_err(map_zai_error)?;
    let mut reasoning_content = String::new();
    let mut content = String::new();
    let mut finish_reason = String::from("unknown");
    let mut usage: Option<TokenUsage> = None;
    let mut pending_tool_calls: BTreeMap<usize, PendingToolCall> = BTreeMap::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(map_zai_error)?;
        if let Some(choice) = chunk.choices.first() {
            if let Some(delta) = &choice.delta {
                if let Some(reasoning) = &delta.reasoning_content {
                    reasoning_content.push_str(reasoning);
                }
                if let Some(text) = &delta.content {
                    content.push_str(text);
                }
                if let Some(tool_calls) = &delta.tool_calls {
                    apply_tool_call_delta(tool_calls, &mut pending_tool_calls);
                }
            }
            if let Some(reason) = &choice.finish_reason {
                finish_reason.clone_from(reason);
            }
        }
        if let Some(chunk_usage) = chunk.usage {
            usage = Some(map_usage(chunk_usage));
        }
    }

    let tool_calls = finalize_tool_calls(pending_tool_calls);

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

fn map_usage(usage: Usage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage.prompt_tokens.unwrap_or(0),
        completion_tokens: usage.completion_tokens.unwrap_or(0),
        total_tokens: usage.total_tokens.unwrap_or(0),
    }
}

fn apply_tool_call_delta(
    tool_calls: &[ToolCallMessage],
    pending: &mut BTreeMap<usize, PendingToolCall>,
) {
    for (idx, call) in tool_calls.iter().enumerate() {
        if let Some(call_type) = call.type_.as_deref() {
            if call_type != "function" {
                continue;
            }
        }

        let entry = pending.entry(idx).or_insert_with(|| PendingToolCall {
            id: call.id.clone(),
            name: None,
            arguments: String::new(),
        });

        if entry.id.is_none() {
            entry.id = call.id.clone();
        }

        if let Some(function) = &call.function {
            if entry.name.is_none() {
                entry.name = function.name.clone();
            }
            if let Some(arguments) = &function.arguments {
                entry.arguments.push_str(arguments);
            }
        }
    }
}

fn finalize_tool_calls(pending: BTreeMap<usize, PendingToolCall>) -> Vec<ToolCall> {
    pending
        .into_iter()
        .filter_map(|(idx, call)| {
            let name = call.name?;
            let id = call.id.unwrap_or_else(|| format!("zai-tool-{idx}"));
            let arguments = if call.arguments.trim().is_empty() {
                "{}".to_string()
            } else {
                call.arguments
            };
            Some(ToolCall {
                id,
                function: ToolCallFunction { name, arguments },
                is_recovered: false,
            })
        })
        .collect()
}
