use super::*;

#[derive(Default)]
struct InMemoryWikiBackend {
    objects: Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait::async_trait]
impl crate::agent::wiki_memory::WikiObjectBackend for InMemoryWikiBackend {
    async fn get_text(&self, key: &str) -> Result<Option<String>, crate::storage::StorageError> {
        Ok(self.objects.lock().await.get(key).cloned())
    }

    async fn put_text(&self, key: &str, content: &str) -> Result<(), crate::storage::StorageError> {
        self.objects
            .lock()
            .await
            .insert(key.to_string(), content.to_string());
        Ok(())
    }
}

#[test]
fn policy_controlled_hook_skips_disabled_manageable_hook() {
    let policy = Arc::new(std::sync::RwLock::new(HookAccessPolicy::new(
        None,
        std::collections::HashSet::from(["workload_distributor".to_string()]),
    )));
    let hook =
        PolicyControlledHook::new("workload_distributor", Box::new(BlockingTestHook), policy);
    let todos = TodoList::new();
    let memory = crate::agent::memory::AgentMemory::new(1024);

    let result = hook.handle(
        &HookEvent::BeforeAgent {
            prompt: "test".to_string(),
        },
        &HookContext::new(&todos, &memory, 0, 0, 4),
    );

    assert!(matches!(result, HookResult::Continue));
}

#[tokio::test]
async fn executor_injects_configured_wiki_memory_context() {
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("mock-model".to_string()),
        agent_model_provider: Some("mock".to_string()),
        ..crate::config::AgentSettings::default()
    });
    let context_id = crate::agent::wiki_memory::wiki_context_id(9, "session:9");
    let backend = Arc::new(InMemoryWikiBackend::default());
    backend.objects.lock().await.insert(
        "prod/wiki/v1/global/index.md".to_string(),
        "# Wiki Index\n".to_string(),
    );
    backend.objects.lock().await.insert(
        format!("prod/wiki/v1/contexts/{context_id}/index.md"),
        "# Wiki Index\n\n## Core pages\n\n- [overview](overview.md) - project facts\n".to_string(),
    );
    backend.objects.lock().await.insert(
        format!("prod/wiki/v1/contexts/{context_id}/overview.md"),
        "# Overview\n\nDurable project fact from wiki.".to_string(),
    );

    let mut provider = crate::llm::MockLlmProvider::new();
    provider.expect_chat_with_tools().return_once(|request| {
        assert!(request.system_prompt.contains("## Durable Wiki Memory"));
        assert!(request
            .system_prompt
            .contains("Durable project fact from wiki."));
        Ok(crate::llm::ChatResponse {
            content: Some(
                r#"{"thought":"done","tool_call":null,"final_answer":"ok","awaiting_user_input":null}"#
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: None,
        })
    });
    provider
        .expect_chat_completion()
        .returning(|_, _, _, _, _| {
            Err(crate::llm::LlmError::Unknown("Not implemented".to_string()))
        });
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("mock".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(backend, "prod");
    let mut executor =
        AgentExecutor::new(Arc::new(llm), session, settings).with_wiki_memory_store(wiki_store);

    let result = executor.execute("use project facts", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "ok"
    ));
}

#[tokio::test]
async fn executor_flushes_explicit_remember_to_wiki_after_completed_run() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"remembered","awaiting_user_input":null}"#,
    )
    .with_wiki_memory_store(wiki_store);

    let result = executor
        .execute(
            "Remember this: staging deploys must run smoke tests first.",
            None,
        )
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "remembered"
    ));
    let objects = backend.objects.lock().await;
    let page_entry = objects
        .iter()
        .find(|(key, _)| key.contains("/wiki/v1/contexts/") && key.contains("/pages/"))
        .map(|(_, value)| value)
        .expect("wiki page should be flushed");
    assert!(page_entry.contains("type: note"));
    assert!(page_entry.contains("staging deploys must run smoke tests first"));
    let index = objects
        .iter()
        .find(|(key, _)| key.ends_with("/index.md") && key.contains("/wiki/v1/contexts/"))
        .map(|(_, value)| value)
        .expect("wiki index should be reconciled");
    assert!(index.contains("pages/"));
    let log = objects
        .iter()
        .find(|(key, _)| key.ends_with("/log.md") && key.contains("/wiki/v1/contexts/"))
        .map(|(_, value)| value)
        .expect("wiki log should be reconciled");
    assert!(log.contains("post-run wiki memory candidate capture"));
}

#[tokio::test]
async fn executor_flushes_russian_save_intent_to_wiki_after_completed_run() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"сохранил","awaiting_user_input":null}"#,
    )
    .with_wiki_memory_store(wiki_store);

    let result = executor
        .execute(
            "Сохрани это в память: перед деплоем запускать smoke tests.",
            None,
        )
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "сохранил"
    ));
    let objects = backend.objects.lock().await;
    let page_entry = objects
        .iter()
        .find(|(key, _)| key.contains("/wiki/v1/contexts/") && key.contains("/pages/"))
        .map(|(_, value)| value)
        .expect("wiki page should be flushed for Russian save intent");
    assert!(page_entry.contains("type: note"));
    assert!(page_entry.contains("перед деплоем запускать smoke tests"));
}

#[test]
fn hard_timeout_uses_configured_duration_and_message() {
    let executor = build_executor_with_timeout(36_000);

    assert_eq!(
        executor.agent_timeout_duration(),
        std::time::Duration::from_secs(36_000)
    );
    assert_eq!(
        executor.agent_timeout_error_message(),
        "Task exceeded timeout limit (600 minutes)"
    );
}

#[test]
fn executor_timeout_check_uses_configured_value_and_ignores_idle_sessions() {
    let mut executor = build_executor_with_timeout(0);

    executor.session_mut().start_task();
    assert!(executor.is_timed_out());

    executor.reset();
    assert!(!executor.is_timed_out());
}

#[tokio::test]
async fn execute_new_task_remembers_task_and_appends_single_user_task() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"ok","awaiting_user_input":null}"#,
    );

    let result = executor.execute("ship it", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "ok"
    ));
    assert_eq!(executor.last_task(), Some("ship it"));

    let user_task_count = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .count();
    assert_eq!(user_task_count, 1);
}
