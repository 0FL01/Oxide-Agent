use oxide_agent::agent::{AgentExecutor, AgentSession, AgentStatus, TodoItem, TodoStatus};
use oxide_agent::config::Settings;
use oxide_agent::llm::LlmClient;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn settings_without_llm_providers() -> Settings {
    Settings {
        telegram_token: "dummy".to_string(),
        openrouter_site_name: "Oxide Agent TG Bot".to_string(),
        ..Settings::default()
    }
}

#[tokio::test]
async fn cancellation_token_is_not_overwritten_by_task_start() {
    let settings = Arc::new(settings_without_llm_providers());
    let llm = Arc::new(LlmClient::new(&settings));
    let mut session = AgentSession::new(1, 1);

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
    let result = executor.execute("test", None).await;

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
