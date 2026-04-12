//! Scripted LLM provider for deterministic E2E tests.
//!
//! Accepts a sequence of scripted responses and returns them in order.
//! If the sequence is exhausted, returns a default response.
//!
//! This is used in E2E tests to measure application-level latency
//! without depending on real LLM API responses.

use async_trait::async_trait;
use oxide_agent_core::llm::{
    ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage, ToolCall,
    ToolCallCorrelation, ToolCallFunction,
};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A single scripted response.
#[derive(Debug, Clone)]
pub enum ScriptedResponse {
    /// Return a plain text response (no tool calls).
    Text(String),
    /// Return one or more tool calls followed by a final text.
    ToolCalls {
        tool_calls: Vec<ScriptedToolCall>,
        final_text: Option<String>,
    },
}

/// A scripted tool call.
#[derive(Debug, Clone)]
pub struct ScriptedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ScriptedToolCall {
    fn into_tool_call(self) -> ToolCall {
        let correlation = ToolCallCorrelation::new(self.id.clone())
            .with_provider_tool_call_id(format!("scripted-{}", self.id));

        ToolCall::new(
            self.id,
            ToolCallFunction {
                name: self.name,
                arguments: self.arguments,
            },
            false,
        )
        .with_correlation(correlation)
    }
}

impl ScriptedResponse {
    fn into_chat_response(self) -> ChatResponse {
        match self {
            ScriptedResponse::Text(content) => ChatResponse {
                content: Some(content),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            },
            ScriptedResponse::ToolCalls {
                tool_calls,
                final_text,
            } => ChatResponse {
                content: final_text,
                tool_calls: tool_calls
                    .into_iter()
                    .map(ScriptedToolCall::into_tool_call)
                    .collect(),
                finish_reason: "tool_calls".to_string(),
                reasoning_content: None,
                usage: None,
            },
        }
    }
}

/// Scripted LLM provider for deterministic E2E tests.
///
/// # Example
///
/// ```rust,ignore
/// let provider = ScriptedLlmProvider::new(vec![
///     ScriptedResponse::ToolCalls {
///         tool_calls: vec![ScriptedToolCall {
///             id: "call_1".to_string(),
///             name: "write_todos".to_string(),
///             arguments: r#"{"todos":[{"description":"Test","status":"in_progress"}]}"#.to_string(),
///         }],
///         final_text: None,
///     },
///     ScriptedResponse::Text("Done!".to_string()),
/// ]);
/// ```
pub struct ScriptedLlmProvider {
    responses: Arc<RwLock<VecDeque<ScriptedResponse>>>,
}

impl ScriptedLlmProvider {
    /// Create a new scripted provider with the given response sequence.
    #[must_use]
    pub fn new(responses: Vec<ScriptedResponse>) -> Self {
        Self {
            responses: Arc::new(RwLock::new(responses.into())),
        }
    }

    /// Push an additional response to the end of the queue.
    pub async fn push(&self, response: ScriptedResponse) {
        self.responses.write().await.push_back(response);
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlmProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        let response = self.responses.write().await.pop_front();
        match response {
            Some(ScriptedResponse::Text(text)) => Ok(text),
            Some(ScriptedResponse::ToolCalls { final_text, .. }) => {
                Ok(final_text.unwrap_or_default())
            }
            None => Ok("No scripted response available.".to_string()),
        }
    }

    async fn chat_with_tools<'a>(
        &self,
        _: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        let response = self.responses.write().await.pop_front();
        match response {
            Some(ScriptedResponse::Text(text)) => {
                // Return valid structured JSON to avoid structured-output parsing failures.
                let json = serde_json::json!({
                    "thought": "Responding to user",
                    "final_answer": text
                });
                Ok(ChatResponse {
                    content: Some(json.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: Some(TokenUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    }),
                })
            }
            Some(ScriptedResponse::ToolCalls {
                tool_calls,
                final_text,
            }) => Ok(ScriptedResponse::ToolCalls {
                tool_calls,
                final_text,
            }
            .into_chat_response()),
            None => {
                let json = serde_json::json!({
                    "thought": "No scripted response available",
                    "final_answer": "No scripted response available."
                });
                Ok(ChatResponse {
                    content: Some(json.to_string()),
                    tool_calls: Vec::new(),
                    finish_reason: "stop".to_string(),
                    reasoning_content: None,
                    usage: Some(TokenUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                    }),
                })
            }
        }
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown("transcribe not implemented".to_string()))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "analyze_image not implemented".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{ScriptedResponse, ScriptedToolCall};

    #[test]
    fn scripted_tool_calls_include_correlation_metadata() {
        let response = ScriptedResponse::ToolCalls {
            tool_calls: vec![ScriptedToolCall {
                id: "call_1".to_string(),
                name: "write_todos".to_string(),
                arguments: r#"{"todos":[]}"#.to_string(),
            }],
            final_text: None,
        }
        .into_chat_response();

        let tool_call = response.tool_calls.first().expect("tool call present");
        let correlation = tool_call.correlation();

        assert_eq!(tool_call.invocation_id().as_str(), "call_1");
        assert_eq!(correlation.legacy_tool_call_id(), "call_1");
        assert_eq!(correlation.wire_tool_call_id(), "scripted-call_1");
    }
}
