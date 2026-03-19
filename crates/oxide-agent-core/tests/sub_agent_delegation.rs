use oxide_agent_core::agent::hooks::DelegationGuardHook;
use oxide_agent_core::agent::providers::DelegationProvider;
use oxide_agent_core::agent::ToolProvider;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{
    ChatResponse, ChatWithToolsRequest, LlmClient, LlmError, LlmProvider, Message,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedSubAgentRequest {
    model_id: String,
    max_tokens: u32,
    message_count: usize,
}

struct BudgetProbeProvider {
    requests: Arc<Mutex<Vec<RecordedSubAgentRequest>>>,
}

#[async_trait::async_trait]
impl LlmProvider for BudgetProbeProvider {
    async fn chat_completion(
        &self,
        _system_prompt: &str,
        _history: &[Message],
        _user_message: &str,
        _model_id: &str,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        unreachable!("delegation smoke test uses chat_with_tools")
    }

    async fn transcribe_audio(
        &self,
        _audio_bytes: Vec<u8>,
        _mime_type: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation smoke test does not transcribe audio")
    }

    async fn analyze_image(
        &self,
        _image_bytes: Vec<u8>,
        _text_prompt: &str,
        _system_prompt: &str,
        _model_id: &str,
    ) -> Result<String, LlmError> {
        unreachable!("delegation smoke test does not analyze images")
    }

    async fn chat_with_tools<'a>(
        &self,
        request: ChatWithToolsRequest<'a>,
    ) -> Result<ChatResponse, LlmError> {
        self.requests
            .lock()
            .expect("probe lock")
            .push(RecordedSubAgentRequest {
                model_id: request.model_id.to_string(),
                max_tokens: request.max_tokens,
                message_count: request.messages.len(),
            });

        Ok(ChatResponse {
            content: Some(
                r#"{"thought":"done","tool_call":null,"final_answer":"budget-path-ok"}"#
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    }
}

#[tokio::test]
async fn sub_agent_delegation_budget_path_smoke_test() -> anyhow::Result<()> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let settings = Arc::new(AgentSettings {
        sub_agent_model_id: Some("sub-model".to_string()),
        sub_agent_model_provider: Some("mock-provider".to_string()),
        sub_agent_max_output_tokens: Some(1_234),
        sub_agent_context_window_tokens: Some(48_000),
        ..AgentSettings::default()
    });
    let mut llm = LlmClient::new(&settings);
    llm.register_provider(
        "mock-provider".to_string(),
        Arc::new(BudgetProbeProvider {
            requests: Arc::clone(&requests),
        }),
    );
    let provider = DelegationProvider::new(Arc::new(llm), 1, settings.clone());

    let args = json!({
        "task": "Return a short confirmation.",
        "tools": ["write_todos"],
        "context": "Keep the answer short."
    });

    let result = provider
        .execute("delegate_to_sub_agent", &args.to_string(), None, None)
        .await?;

    assert_eq!(result.trim(), "budget-path-ok");

    let captured = requests.lock().expect("probe lock").clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].model_id, "sub-model");
    assert_eq!(captured[0].max_tokens, 1_234);
    assert_eq!(captured[0].message_count, 1);

    Ok(())
}

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
