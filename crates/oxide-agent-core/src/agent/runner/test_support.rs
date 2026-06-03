//! Shared test fixtures for runner module tests.

use crate::config::AgentSettings;
use crate::llm::{ChatResponse, LlmClient, LlmError, MockLlmProvider};
use std::sync::Arc;

pub(super) fn build_llm_client(provider: MockLlmProvider) -> Arc<LlmClient> {
    build_llm_client_for_provider(provider, "opencode-go", "deepseek-v4-flash")
}

pub(super) fn build_llm_client_for_provider(
    provider: MockLlmProvider,
    provider_name: &str,
    model_name: &str,
) -> Arc<LlmClient> {
    let settings = AgentSettings {
        agent_model_id: Some(model_name.to_string()),
        agent_model_provider: Some(provider_name.to_string()),
        agent_model_max_output_tokens: Some(256),
        ..AgentSettings::default()
    };
    let mut llm_client = LlmClient::new(&settings);
    llm_client.register_provider(provider_name.to_string(), Arc::new(provider));
    Arc::new(llm_client)
}

pub(super) fn final_structured_response() -> ChatResponse {
    ChatResponse {
        content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
        tool_calls: Vec::new(),
        finish_reason: "stop".to_string(),
        reasoning_content: None,
        usage: None,
    }
}

pub(super) fn stub_non_chat_methods(provider: &mut MockLlmProvider) {
    provider
        .expect_complete_internal_text()
        .returning(|_, _, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
}

pub(super) fn single_final_response_provider() -> MockLlmProvider {
    let mut provider = MockLlmProvider::new();
    provider
        .expect_chat_with_tools()
        .return_once(|_| Ok(final_structured_response()));
    stub_non_chat_methods(&mut provider);
    provider
}

pub(super) fn accidental_structured_final_answer_provider() -> MockLlmProvider {
    let mut provider = MockLlmProvider::new();
    provider.expect_chat_with_tools().return_once(|_| {
        Ok(ChatResponse {
            content: Some(
                r#"{"thought":"Tool list ready","tool_call":null,"final_answer":"Tools: `read_file`, `write_file`","awaiting_user_input":null}"#
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    });
    stub_non_chat_methods(&mut provider);
    provider
}

pub(super) fn context_overflow_then_summary_then_final_provider() -> MockLlmProvider {
    let mut provider = MockLlmProvider::new();
    let mut sequence = mockall::Sequence::new();
    provider
        .expect_chat_with_tools()
        .times(1)
        .in_sequence(&mut sequence)
        .return_once(|_| {
            Err(LlmError::ApiError(
                "maximum context length exceeded".to_string(),
            ))
        });
    provider
        .expect_chat_with_tools()
        .times(1)
        .in_sequence(&mut sequence)
        .return_once(|_| Ok(final_structured_response()));
    provider.expect_complete_internal_text().times(1).returning(
        |_, _, user_message, model_id, _| {
            assert_eq!(model_id, "deepseek-v4-flash");
            assert!(user_message.contains("## Source History"));
            Ok("Runtime context-limit handoff summary.".to_string())
        },
    );
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
}

pub(super) fn pre_sampling_summary_then_final_provider() -> MockLlmProvider {
    let mut provider = MockLlmProvider::new();
    provider
        .expect_chat_with_tools()
        .times(1)
        .return_once(|_| Ok(final_structured_response()));
    provider.expect_complete_internal_text().times(1).returning(
        |_, _, user_message, model_id, _| {
            assert_eq!(model_id, "deepseek-v4-flash");
            assert!(user_message.contains("## Source History"));
            Ok("Pre-sampling handoff summary.".to_string())
        },
    );
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(LlmError::Unknown("Not implemented".to_string())));
    provider
}

pub(super) async fn collect_progress_events(
    progress_rx: &mut tokio::sync::mpsc::Receiver<crate::agent::progress::AgentEvent>,
) -> Vec<crate::agent::progress::AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = progress_rx.recv().await {
        events.push(event);
    }
    events
}
