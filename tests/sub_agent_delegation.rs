use oxide_agent::agent::providers::DelegationProvider;
use oxide_agent::agent::ToolProvider;
use oxide_agent::config::Settings;
use oxide_agent::llm::LlmClient;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
#[ignore = "Requires real LLM provider credentials and network access"]
async fn sub_agent_delegation_smoke_test() -> anyhow::Result<()> {
    let settings = Settings::new()?;
    let llm = Arc::new(LlmClient::new(&settings));
    let provider = DelegationProvider::new(llm, 1);

    let args = json!({
        "task": "Сделай короткое резюме о том, что такое Rust и где его применяют.",
        "tools": ["write_todos"],
        "context": "Ответ должен быть кратким, 3-5 предложений."
    });

    let result = provider
        .execute("delegate_to_sub_agent", &args.to_string(), None)
        .await?;

    assert!(!result.trim().is_empty());
    Ok(())
}
