use super::*;

#[derive(Default)]
struct InMemoryWikiBackend {
    objects: Mutex<std::collections::HashMap<String, String>>,
    put_gate: StdMutex<Option<Arc<tokio::sync::Semaphore>>>,
}

#[async_trait::async_trait]
impl crate::agent::wiki_memory::WikiObjectBackend for InMemoryWikiBackend {
    async fn get_text(&self, key: &str) -> Result<Option<String>, crate::storage::StorageError> {
        Ok(self.objects.lock().await.get(key).cloned())
    }

    async fn put_text(&self, key: &str, content: &str) -> Result<(), crate::storage::StorageError> {
        let gate = self
            .put_gate
            .lock()
            .ok()
            .and_then(|gate| gate.as_ref().cloned());
        if let Some(gate) = gate {
            let _permit = gate
                .acquire()
                .await
                .map_err(|_| crate::storage::StorageError::S3Put("test put gate closed".into()))?;
        }

        self.objects
            .lock()
            .await
            .insert(key.to_string(), content.to_string());
        Ok(())
    }

    async fn delete_text(&self, key: &str) -> Result<(), crate::storage::StorageError> {
        self.objects.lock().await.remove(key);
        Ok(())
    }
}

async fn wait_for_wiki_entry(
    backend: &Arc<InMemoryWikiBackend>,
    predicate: impl Fn(&str, &str) -> bool,
) -> (String, String) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Some(entry) = {
            let objects = backend.objects.lock().await;
            objects
                .iter()
                .find(|(key, value)| predicate(key, value))
                .map(|(key, value)| (key.clone(), value.clone()))
        } {
            return entry;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for wiki background writer"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[test]
fn policy_controlled_hook_skips_disabled_manageable_hook() {
    let policy = Arc::new(std::sync::RwLock::new(HookAccessPolicy::new(
        None,
        std::collections::HashSet::from(["search_budget".to_string()]),
    )));
    let hook = PolicyControlledHook::new("search_budget", Box::new(BlockingTestHook), policy);
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

#[test]
fn executor_registers_episodic_extract_hook_for_wiki_drafts() {
    let executor = build_executor();

    assert!(executor.runner.has_registered_hook("episodic_extract"));
}

#[cfg(feature = "tool-wiki-memory")]
#[test]
fn executor_exposes_wiki_memory_tools_when_store_configured() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend;
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let executor = build_executor().with_wiki_memory_store(wiki_store);
    let tools = executor.current_tool_definitions();

    assert!(tools.iter().any(|tool| tool.name == "wiki_memory_list"));
    assert!(tools.iter().any(|tool| tool.name == "wiki_memory_read"));
    assert!(tools.iter().any(|tool| tool.name == "wiki_memory_search"));
    assert!(tools.iter().any(|tool| tool.name == "wiki_memory_delete"));
}

#[tokio::test]
async fn new_task_clears_stale_todos_before_completion_check() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"answer ready","tool_call":null,"final_answer":"quick answer","awaiting_user_input":null}"#,
    );
    executor
        .session_mut()
        .memory
        .todos
        .items
        .push(crate::agent::providers::TodoItem::new(
            "stale unfinished work",
        ));

    let result = executor.execute("answer a simple question", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "quick answer"
    ));
    assert!(executor.session().memory.todos.items.is_empty());
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
async fn manual_compaction_uses_current_compaction_controller() {
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("mock-model".to_string()),
        agent_model_provider: Some("mock".to_string()),
        ..crate::config::AgentSettings::default()
    });
    let mut provider = crate::llm::MockLlmProvider::new();
    provider
        .expect_chat_completion()
        .times(1)
        .returning(|_, _, user_message, model_id, _| {
            assert_eq!(model_id, "mock-model");
            assert!(user_message.contains("## Source History"));
            Ok("Current compact handoff summary.".to_string())
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
    let mut executor = AgentExecutor::new(Arc::new(llm), session, settings);
    executor.session_mut().last_task = Some("Ship compaction".to_string());
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_task(
            "Ship compaction",
        ));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::summary(
            "[COMPACTION_SUMMARY]\nOld summary",
        ));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue"));

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
    let outcome = executor
        .compact_current_context(Some(progress_tx))
        .await
        .expect("manual compaction succeeds");
    let mut event_names = Vec::new();
    while let Some(event) = progress_rx.recv().await {
        event_names.push(match event {
            crate::agent::progress::AgentEvent::RuntimeCompactionStarted { .. } => {
                "runtime_started"
            }
            crate::agent::progress::AgentEvent::RuntimeCompactionCompleted { .. } => {
                "runtime_completed"
            }
            _ => "other",
        });
    }

    assert_eq!(outcome.metadata.generation, 1);
    assert_eq!(outcome.metadata.provider, "mock");
    assert_eq!(outcome.metadata.route, "mock-model");
    assert!(outcome.replacement.history_items_after <= outcome.replacement.history_items_before);
    let messages = executor.session().memory.get_messages();
    assert_eq!(
        messages
            .iter()
            .filter(|message| message
                .content
                .starts_with(crate::agent::compaction::OXIDE_COMPACTED_SUMMARY_PREFIX))
            .count(),
        1
    );
    assert!(messages
        .iter()
        .all(|message| !message.content.contains("[COMPACTION_SUMMARY]")));
    assert_eq!(event_names, vec!["runtime_started", "runtime_completed"]);
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
    let (_, page_entry) = wait_for_wiki_entry(&backend, |key, _| {
        key.contains("/wiki/v1/contexts/") && key.contains("/pages/")
    })
    .await;
    assert!(page_entry.contains("type: note"));
    assert!(page_entry.contains("staging deploys must run smoke tests first"));
    let (_, index) = wait_for_wiki_entry(&backend, |key, _| {
        key.ends_with("/index.md") && key.contains("/wiki/v1/contexts/")
    })
    .await;
    assert!(index.contains("pages/"));
    let (_, log) = wait_for_wiki_entry(&backend, |key, _| {
        key.ends_with("/log.md") && key.contains("/wiki/v1/contexts/")
    })
    .await;
    assert!(log.contains("post-run wiki memory candidate capture"));
}

#[tokio::test]
async fn executor_prefers_contentful_final_answer_for_explicit_remember() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"Lucky number = 42.","awaiting_user_input":null}"#,
    )
    .with_wiki_memory_store(wiki_store);

    let result = executor
        .execute("Remember this: my lucky number", None)
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "Lucky number = 42."
    ));
    let (_, page_entry) = wait_for_wiki_entry(&backend, |key, _| {
        key.contains("/wiki/v1/contexts/") && key.contains("/pages/")
    })
    .await;
    assert!(page_entry.contains("Lucky number = 42."));
    assert!(!page_entry.contains("# explicit-remember\n\nRemember this: my lucky number"));
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
    let (_, page_entry) = wait_for_wiki_entry(&backend, |key, _| {
        key.contains("/wiki/v1/contexts/") && key.contains("/pages/")
    })
    .await;
    assert!(page_entry.contains("type: note"));
    assert!(page_entry.contains("перед деплоем запускать smoke tests"));
}

#[tokio::test]
async fn executor_spawns_wiki_memory_flush_without_blocking_completed_result() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *backend.put_gate.lock().expect("put gate lock poisoned") = Some(Arc::clone(&gate));

    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"remembered","awaiting_user_input":null}"#,
    )
    .with_wiki_memory_store(wiki_store);

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        executor.execute(
            "Remember this: background wiki flush must not block completion.",
            None,
        ),
    )
    .await;

    gate.add_permits(1);

    assert!(matches!(
        result,
        Ok(Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer))) if answer == "remembered"
    ));

    let (_, page_entry) = wait_for_wiki_entry(&backend, |key, _| {
        key.contains("/wiki/v1/contexts/") && key.contains("/pages/")
    })
    .await;
    assert!(page_entry.contains("background wiki flush must not block completion"));
}

#[tokio::test]
async fn background_writer_extracts_previous_message_for_empty_remember_payload() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("mock-agent".to_string()),
        agent_model_provider: Some("mock".to_string()),
        wiki_memory_writer_enabled: Some(true),
        wiki_memory_writer_model_id: Some("mock-writer".to_string()),
        wiki_memory_writer_model_provider: Some("mock".to_string()),
        wiki_memory_writer_max_output_tokens: Some(4096),
        wiki_memory_writer_timeout_secs: Some(5),
        ..crate::config::AgentSettings::default()
    });

    let mut provider = crate::llm::MockLlmProvider::new();
    provider.expect_chat_with_tools().return_once(|_| {
        Ok(crate::llm::ChatResponse {
            content: Some(
                r#"{"thought":"done","tool_call":null,"final_answer":"Запомнил.","awaiting_user_input":null}"#
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
        .return_once(|system_prompt, history, user_message, model_id, max_tokens| {
            assert!(system_prompt.contains("background memory curator"));
            assert!(history.is_empty());
            assert_eq!(model_id, "mock-writer");
            assert_eq!(max_tokens, 4096);
            assert!(user_message.contains("Локация юзера"));
            assert!(user_message.contains("сохрани в память"));
            Ok(r#"{"candidates":[{"kind":"fact","title":"User location","content":"Локация пользователя: Россия, Кировская область, Котельнич.","confidence":0.92,"importance":0.8,"tags":["explicit-remember","profile","location"],"evidence":["Previous user message supplied the location and the latest user message asked to save it."],"reason":"User explicitly asked to save the prior location context."}]}"#.to_string())
        });
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("mock".to_string(), Arc::new(provider));
    let mut session = AgentSession::new(9_i64.into());
    session
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_turn(
            "Добавь что Локация юзера: Россия, Кировская обл, Котельнич",
        ));
    session
        .memory
        .add_message(crate::agent::memory::AgentMessage::assistant(
            "Запомнил: локация юзера — Россия, Кировская обл., Котельнич.",
        ));
    let mut executor =
        AgentExecutor::new(Arc::new(llm), session, settings).with_wiki_memory_store(wiki_store);

    let result = executor.execute("сохрани в память", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "Запомнил."
    ));
    let (_, page_entry) = wait_for_wiki_entry(&backend, |key, value| {
        key.contains("/wiki/v1/contexts/") && key.contains("/pages/") && value.contains("Котельнич")
    })
    .await;
    assert!(page_entry.contains("Локация пользователя"));
    assert!(!page_entry.contains("# сохрани в память"));
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
