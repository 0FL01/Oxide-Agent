use anyhow::Result;
use async_trait::async_trait;
use oxide_agent_core::agent::{
    AgentExecutionOutcome, AgentExecutor, AgentSession, PendingInputKind, SessionId,
};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{
    ChatResponse, LlmClient, LlmError, LlmProvider, Message, ToolCall, ToolCallFunction,
    ToolDefinition,
};
use std::sync::Arc;

struct WaitingInputProvider;

#[async_trait]
impl LlmProvider for WaitingInputProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "chat_completion is not used in this test".to_string(),
        ))
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "transcribe_audio is not used in this test".to_string(),
        ))
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        Err(LlmError::Unknown(
            "analyze_image is not used in this test".to_string(),
        ))
    }

    async fn chat_with_tools(
        &self,
        _system_prompt: &str,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model_id: &str,
        _max_tokens: u32,
        _json_mode: bool,
    ) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            content: Some("tool_call".to_string()),
            tool_calls: vec![ToolCall {
                id: "call_waiting_input_1".to_string(),
                function: ToolCallFunction {
                    name: "request_user_input".to_string(),
                    arguments: r#"{"prompt":"Approve deployment?","kind":"choice","choice":{"options":["yes","no"],"allow_multiple":false,"min_choices":1,"max_choices":1}}"#.to_string(),
                },
                is_recovered: false,
            }],
            finish_reason: "tool_calls".to_string(),
            reasoning_content: None,
            usage: None,
        })
    }
}

fn waiting_input_settings() -> AgentSettings {
    AgentSettings {
        openrouter_site_name: "Oxide Agent Bot".to_string(),
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("openrouter".to_string()),
        agent_model_max_tokens: Some(8_192),
        ..AgentSettings::default()
    }
}

#[tokio::test]
async fn executor_returns_waiting_input_for_real_request_tool() {
    let settings = Arc::new(waiting_input_settings());
    let mut llm_client = LlmClient::new(settings.as_ref());
    llm_client.register_provider("openrouter".to_string(), Arc::new(WaitingInputProvider));
    let llm = Arc::new(llm_client);

    let mut executor = AgentExecutor::new(llm, AgentSession::new(SessionId::from(7001)), settings);

    let result = executor
        .execute_with_outcome("Decide deployment strategy", None, None)
        .await;

    let outcome = match result {
        Ok(value) => value,
        Err(error) => panic!("expected waiting-input outcome, got error: {error}"),
    };

    let pending = match outcome {
        AgentExecutionOutcome::WaitingInput(pending_input) => pending_input,
        AgentExecutionOutcome::Completed(answer) => {
            panic!("expected waiting-input outcome, got completion: {answer}")
        }
    };

    assert_eq!(pending.prompt, "Approve deployment?");
    match pending.kind {
        PendingInputKind::Choice(choice) => {
            assert_eq!(choice.options, vec!["yes".to_string(), "no".to_string()]);
            assert!(!choice.allow_multiple);
            assert_eq!(choice.min_choices, 1);
            assert_eq!(choice.max_choices, 1);
        }
        PendingInputKind::Text(_) => panic!("expected choice pending input"),
    }
}
