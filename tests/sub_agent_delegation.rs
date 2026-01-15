use oxide_agent::agent::providers::DelegationProvider;
use oxide_agent::agent::ToolProvider;
use oxide_agent::config::Settings;
use oxide_agent::llm::LlmClient;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
#[ignore = "Requires real LLM provider credentials and network access"]
async fn sub_agent_delegation_smoke_test() -> anyhow::Result<()> {
    let settings = Arc::new(Settings::new()?);
    let llm = Arc::new(LlmClient::new(&settings));
    let provider = DelegationProvider::new(llm, 1, settings.clone());

    let args = json!({
        "task": "Make a short summary of what Rust is and where it is used.",
        "tools": ["write_todos"],
        "context": "The answer should be brief, 3-5 sentences."
    });

    let result = provider
        .execute("delegate_to_sub_agent", &args.to_string(), None, None)
        .await?;

    assert!(!result.trim().is_empty());
    Ok(())
}
