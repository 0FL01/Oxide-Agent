use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::agent::{
    AgentContext, CompactionRequest, CompactionService, CompactionSummarizer,
    CompactionSummarizerConfig, CompactionTrigger, EphemeralSession, TodoItem, TodoStatus,
};
use oxide_agent_core::llm::LlmClient;
use std::sync::Arc;

#[tokio::test]
async fn public_compaction_service_preserves_live_context_and_todos() {
    let mut session = EphemeralSession::new(256);
    session.memory_mut().todos.update(vec![
        TodoItem {
            description: "Keep pinned context intact".to_string(),
            status: TodoStatus::InProgress,
        },
        TodoItem {
            description: "Validate public compaction lifecycle".to_string(),
            status: TodoStatus::Pending,
        },
    ]);
    session
        .memory_mut()
        .add_message(AgentMessage::topic_agents_md(
            "# Topic AGENTS\nPreserve context identity.",
        ));
    session
        .memory_mut()
        .add_message(AgentMessage::user_task("Ship stage 12"));
    session.memory_mut().add_message(AgentMessage::user(
        "Older request about compaction hardening.",
    ));
    session
        .memory_mut()
        .add_message(AgentMessage::assistant("Older response with findings."));
    session
        .memory_mut()
        .add_message(AgentMessage::user("Recent request 1."));
    session
        .memory_mut()
        .add_message(AgentMessage::assistant("Recent response 1."));
    session
        .memory_mut()
        .add_message(AgentMessage::user("Recent request 2."));
    session
        .memory_mut()
        .add_message(AgentMessage::assistant("Recent response 2."));

    let llm_client = Arc::new(LlmClient::new(
        &oxide_agent_core::config::AgentSettings::default(),
    ));
    let service = CompactionService::default().with_summarizer(CompactionSummarizer::new(
        llm_client,
        CompactionSummarizerConfig {
            model_name: String::new(),
            provider_name: String::new(),
            timeout_secs: 1,
        },
    ));
    let request = CompactionRequest::new(
        CompactionTrigger::Manual,
        "Ship stage 12",
        "system prompt",
        &[],
        "demo-model",
        256,
        false,
    );

    let outcome = service
        .prepare_for_run(&request, &mut session)
        .await
        .expect("public compaction service succeeds");

    assert!(outcome.applied);
    assert_eq!(session.memory().todos.items.len(), 2);
    assert_eq!(
        session
            .memory()
            .todos
            .current_task()
            .map(|item| item.description.as_str()),
        Some("Keep pinned context intact")
    );

    let messages = session.memory().get_messages();
    assert_eq!(
        messages[0].content,
        "[TOPIC_AGENTS_MD]\n# Topic AGENTS\nPreserve context identity."
    );
    assert!(messages
        .iter()
        .any(|message| message.summary_payload().is_some()));
    assert_eq!(messages[messages.len() - 4].content, "Recent request 1.");
    assert_eq!(messages[messages.len() - 1].content, "Recent response 2.");
    assert!(messages.iter().all(|message| !message
        .content
        .contains("Older request about compaction hardening.")));
}
