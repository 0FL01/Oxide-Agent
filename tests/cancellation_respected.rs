use another_chat_rs::agent::{AgentExecutor, AgentSession, AgentStatus};
use another_chat_rs::config::Settings;
use another_chat_rs::llm::LlmClient;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn settings_without_llm_providers() -> Settings {
    Settings {
        telegram_token: "dummy".to_string(),
        allowed_users_str: None,
        agent_allowed_users_str: None,
        groq_api_key: None,
        mistral_api_key: None,
        zai_api_key: None,
        gemini_api_key: None,
        openrouter_api_key: None,
        tavily_api_key: None,
        r2_access_key_id: None,
        r2_secret_access_key: None,
        r2_endpoint_url: None,
        r2_bucket_name: None,
        openrouter_site_url: String::new(),
        openrouter_site_name: "Another Chat TG Bot".to_string(),
        system_message: None,
    }
}

#[tokio::test]
async fn cancellation_token_is_not_overwritten_by_task_start() {
    let llm = Arc::new(LlmClient::new(&settings_without_llm_providers()));
    let mut session = AgentSession::new(1, 1);

    let token = CancellationToken::new();
    token.cancel();
    session.cancellation_token = token;

    let mut executor = AgentExecutor::new(llm, session);
    let result = executor.execute("test", None).await;

    let Err(err) = result else {
        panic!("expected cancellation error");
    };
    assert!(
        err.to_string().contains("отменена"),
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
}
