use oxide_agent_core::agent::hooks::DelegationGuardHook;
use oxide_agent_core::agent::providers::DelegationProvider;
use oxide_agent_core::agent::ToolProvider;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
#[ignore = "Requires real LLM provider credentials and network access"]
async fn sub_agent_delegation_smoke_test() -> anyhow::Result<()> {
    let settings = Arc::new(AgentSettings::new()?);
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

#[test]
fn delegation_guard_whitelist_test() {
    let hook = DelegationGuardHook::new();

    // Russian retrieval tasks (should NOT be blocked)
    let ru_tasks = [
        "Собери статьи о ATS",
        "Прочитай документацию",
        "Найди файлы с конфигами",
        "Излеки данные из отчета",
        "Получи список вакансий",
    ];

    for task in ru_tasks {
        assert!(
            hook.check_task(task).is_none(),
            "Russian retrieval task should pass whitelist: {}",
            task
        );
    }

    // English retrieval tasks (should NOT be blocked)
    let en_tasks = [
        "Collect articles about ATS",
        "Read the documentation",
        "Find config files",
        "Extract data from report",
        "Retrieve job listings",
        "Gather information about market",
        "Compile a list of sources",
    ];

    for task in en_tasks {
        assert!(
            hook.check_task(task).is_none(),
            "English retrieval task should pass whitelist: {}",
            task
        );
    }

    // Analytical tasks (SHOULD be blocked)
    let analytical_tasks = [
        "Проанализируй методы фильтрации",
        "Объясни как работают ATS",
        "Сравни DevOps и Backend позиции",
        "Evaluate the effectiveness of filters",
        "Why is this approach better?",
    ];

    for task in analytical_tasks {
        assert!(
            hook.check_task(task).is_some(),
            "Analytical task should be blocked: {}",
            task
        );
    }
}
