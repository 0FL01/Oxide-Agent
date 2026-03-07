use oxide_agent_core::agent::memory::{AgentMessage, MessageRole};
use oxide_agent_core::agent::providers::{TodoItem, TodoStatus};
use oxide_agent_core::agent::{AgentExecutor, AgentSession, AgentStatus, SessionId};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn settings_without_llm_providers() -> AgentSettings {
    AgentSettings {
        openrouter_site_name: "Oxide Agent Bot".to_string(),
        ..AgentSettings::default()
    }
}

#[tokio::test]
async fn resume_execution_seeds_original_task_when_memory_is_empty() {
    let settings = Arc::new(settings_without_llm_providers());
    let llm = Arc::new(LlmClient::new(&settings));
    let mut session = AgentSession::new(SessionId::from(2));

    let token = CancellationToken::new();
    token.cancel();
    session.cancellation_token = token;

    let mut executor = AgentExecutor::new(llm, session, settings);
    let _ = executor
        .execute("original task", Some("resume answer"), None)
        .await;

    let user_messages = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.content.clone())
        .collect::<Vec<_>>();
    assert_eq!(user_messages, vec!["original task", "resume answer"]);
}

#[tokio::test]
async fn resume_execution_does_not_duplicate_original_task_when_memory_exists() {
    let settings = Arc::new(settings_without_llm_providers());
    let llm = Arc::new(LlmClient::new(&settings));
    let mut session = AgentSession::new(SessionId::from(3));
    session
        .memory
        .add_message(AgentMessage::user("original task"));

    let token = CancellationToken::new();
    token.cancel();
    session.cancellation_token = token;

    let mut executor = AgentExecutor::new(llm, session, settings);
    let _ = executor
        .execute("original task", Some("resume answer"), None)
        .await;

    let user_messages = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| message.content.clone())
        .collect::<Vec<_>>();
    assert_eq!(user_messages, vec!["original task", "resume answer"]);
}

#[tokio::test]
async fn cancellation_token_is_not_overwritten_by_task_start() {
    let settings = Arc::new(settings_without_llm_providers());
    let llm = Arc::new(LlmClient::new(&settings));
    let mut session = AgentSession::new(SessionId::from(1));

    session.memory.todos.items = vec![
        TodoItem {
            description: "Task 1".to_string(),
            status: TodoStatus::Pending,
        },
        TodoItem {
            description: "Task 2".to_string(),
            status: TodoStatus::InProgress,
        },
    ];

    let token = CancellationToken::new();
    token.cancel();
    session.cancellation_token = token;

    let mut executor = AgentExecutor::new(llm, session, settings);
    let result = executor.execute("test", None, None).await;

    let Err(err) = result else {
        panic!("expected cancellation error");
    };
    assert!(
        err.to_string().contains("cancelled"),
        "unexpected error: {err}"
    );
    assert!(
        !executor.session().is_processing(),
        "executor session stuck in processing after cancellation"
    );
    assert!(
        matches!(executor.session().status, AgentStatus::Error(_)),
        "unexpected status: {:?}",
        executor.session().status
    );
    assert!(
        executor.session().memory.todos.items.is_empty(),
        "todos were not cleared on cancellation"
    );
}
