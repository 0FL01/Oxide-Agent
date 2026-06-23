#![cfg_attr(
    not(any(oxide_module_llm_provider_opencode_go, oxide_module_tool_wiki_memory)),
    allow(dead_code)
)]

use super::*;
use crate::agent::{AgentExecutionEffort, AgentExecutionOptions};

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
            let _permit = gate.acquire().await.map_err(|_| {
                crate::storage::StorageError::InvalidInput("test put gate closed".into())
            })?;
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

#[tokio::test]
async fn prepare_execution_uses_executor_model_routes_override() {
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_routes: Some(vec![crate::config::ModelInfo {
            id: "global-primary".to_string(),
            provider: "global-provider".to_string(),
            max_output_tokens: 1_000,
            context_window_tokens: 8_000,
            weight: 1,
        }]),
        ..crate::config::AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(llm, session, settings);
    let override_routes = vec![
        crate::config::ModelInfo {
            id: "override-primary".to_string(),
            provider: "override-provider".to_string(),
            max_output_tokens: 2_000,
            context_window_tokens: 16_000,
            weight: 1,
        },
        crate::config::ModelInfo {
            id: "override-fallback".to_string(),
            provider: "override-provider".to_string(),
            max_output_tokens: 3_000,
            context_window_tokens: 32_000,
            weight: 1,
        },
    ];
    executor.set_model_routes_override(override_routes.clone());

    let prepared = executor
        .prepare_execution("use selected model", None, AgentExecutionOptions::default())
        .await;

    assert_eq!(prepared.runner_config.model_name, "override-primary");
    assert_eq!(
        prepared.runner_config.model_provider.as_deref(),
        Some("override-provider")
    );
    assert_eq!(prepared.runner_config.model_max_output_tokens, 2_000);
    assert_eq!(prepared.runner_config.model_routes, override_routes);
}

#[tokio::test]
async fn prepare_execution_heavy_effort_raises_runner_budgets() {
    let mut executor = build_executor();

    let prepared = executor
        .prepare_execution(
            "research deeply",
            None,
            AgentExecutionOptions::with_effort(AgentExecutionEffort::Heavy),
        )
        .await;

    assert!(prepared.runner_config.max_iterations >= 512);
    assert!(prepared.runner_config.continuation_limit >= 150);
    assert!(prepared.runner_config.timeout_secs >= 180 * 60);
    assert!(
        prepared
            .system_prompt
            .contains("2-4 independent research branches")
    );
    assert!(prepared.system_prompt.contains("wait_sub_agents"));
    assert!(prepared.system_prompt.contains("Before final answer"));
}

#[test]
fn execution_options_preserve_effort_derived_reasoning_when_unset() {
    assert_eq!(AgentExecutionOptions::default().reasoning_effort(), None);
    assert_eq!(
        AgentExecutionOptions::with_effort(AgentExecutionEffort::Standard).reasoning_effort(),
        None
    );
    assert_eq!(
        AgentExecutionOptions::with_effort(AgentExecutionEffort::Extended).reasoning_effort(),
        Some("high")
    );
    assert_eq!(
        AgentExecutionOptions::with_effort(AgentExecutionEffort::Heavy).reasoning_effort(),
        Some("high")
    );
}

#[test]
fn execution_options_reasoning_override_wins_over_runtime_effort() {
    let options = AgentExecutionOptions::with_effort(AgentExecutionEffort::Heavy)
        .with_reasoning_effort("medium");

    assert_eq!(options.reasoning_effort(), Some("medium"));
    assert_eq!(options.effort, AgentExecutionEffort::Heavy);
}

#[cfg(oxide_module_tool_wiki_memory)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
#[tokio::test]
async fn new_task_inserts_soft_temporal_boundary_after_long_pause() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"answer ready","tool_call":null,"final_answer":"new topic answer","awaiting_user_input":null}"#,
    );
    executor.session_mut().memory.add_message(
        crate::agent::memory::AgentMessage::user_task("old topic").with_created_at_unix(Some(1)),
    );

    let result = executor.execute("new topic", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "new topic answer"
    ));
    let messages = executor.session().memory.get_messages();
    let boundary_index = messages
        .iter()
        .position(|message| message.content.starts_with("[TEMPORAL_CONTEXT]"))
        .expect("temporal boundary should be inserted");
    let new_task_index = messages
        .iter()
        .position(|message| message.content == "new topic")
        .expect("new task should be inserted");

    assert!(boundary_index < new_task_index);
    assert!(messages[boundary_index].content.contains("long pause"));
    assert!(!messages[boundary_index].content.contains("1779802440"));
}

#[cfg(oxide_module_llm_provider_opencode_go)]
#[tokio::test]
async fn executor_injects_configured_wiki_memory_context() {
    crate::agent::wiki_memory::cache::invalidate_shared_caches_for_tests().await;
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..crate::config::AgentSettings::default()
    });
    let context_id = crate::agent::wiki_memory::wiki_context_id(9, "session:9");
    let backend = Arc::new(InMemoryWikiBackend::default());
    backend.objects.lock().await.insert(
        "test-exec-wiki/wiki/v1/global/index.md".to_string(),
        "# Wiki Index\n".to_string(),
    );
    backend.objects.lock().await.insert(
        format!("test-exec-wiki/wiki/v1/contexts/{context_id}/index.md"),
        "# Wiki Index\n\n## Core pages\n\n- [overview](overview.md) - project facts\n".to_string(),
    );
    backend.objects.lock().await.insert(
        format!("test-exec-wiki/wiki/v1/contexts/{context_id}/overview.md"),
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
        .expect_complete_internal_text()
        .returning(|_, _, _, _, _| {
            Err(crate::llm::LlmError::unknown("Not implemented".to_string()))
        });
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("opencode-go".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(backend, "test-exec-wiki");
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
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        agent_model_context_window_tokens: Some(100),
        ..crate::config::AgentSettings::default()
    });
    let mut provider = crate::llm::MockLlmProvider::new();
    provider.expect_complete_internal_text().times(1).returning(
        |_, _, user_message, model_id, _| {
            assert_eq!(model_id, "deepseek-v4-flash");
            assert!(user_message.contains("## Source History"));
            Ok("Current compact handoff summary.".to_string())
        },
    );
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("opencode-go".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(Arc::new(llm), session, settings);
    executor.session_mut().last_task = Some("Ship compaction".to_string());
    executor.session_mut().memory.set_max_tokens(100);
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_task(
            "Ship compaction",
        ));
    // Add enough old messages to create a compressible range.
    // Using large content to exceed the tail target budget.
    for i in 0..5 {
        executor
            .session_mut()
            .memory
            .add_message(crate::agent::memory::AgentMessage::user_turn(format!(
                "old {i}: {}",
                "x".repeat(200)
            )));
    }
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 1"));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 2"));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 3"));

    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
    executor
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

    // New system: block created in CompactionState, raw memory preserved.
    assert!(
        executor
            .session()
            .memory
            .compaction_state()
            .has_active_blocks(),
        "compaction should have created an active block"
    );
    // Raw messages are preserved (not replaced).
    assert!(
        executor
            .session()
            .memory
            .get_messages()
            .iter()
            .any(|m| m.content.contains("old 0:")),
        "raw memory should be preserved"
    );
    // Rendered context should be smaller (block summary replaces old messages).
    let rendered = executor.session().memory.rendered_messages();
    assert!(
        rendered
            .iter()
            .any(|m| m.content.contains("Compressed conversation section")),
        "rendered context should contain block summary"
    );
    assert_eq!(event_names, vec!["runtime_started", "runtime_completed"]);
}

#[tokio::test]
async fn manual_compaction_runtime_generations_increment_across_repeated_compactions() {
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        agent_model_context_window_tokens: Some(100),
        ..crate::config::AgentSettings::default()
    });
    let mut provider = crate::llm::MockLlmProvider::new();
    provider.expect_complete_internal_text().times(3).returning(
        |_, _, user_message, model_id, _| {
            assert_eq!(model_id, "deepseek-v4-flash");
            assert!(user_message.contains("## Source History"));
            Ok("Current compact handoff summary.".to_string())
        },
    );
    provider
        .expect_transcribe_audio()
        .returning(|_, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("opencode-go".to_string(), Arc::new(provider));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(Arc::new(llm), session, settings);
    executor.session_mut().last_task = Some("Ship compaction".to_string());
    executor.session_mut().memory.set_max_tokens(100);
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_task(
            "Ship compaction",
        ));
    // Add enough old messages for first compaction.
    for i in 0..5 {
        executor
            .session_mut()
            .memory
            .add_message(crate::agent::memory::AgentMessage::user_turn(format!(
                "old {i}: {}",
                "x".repeat(200)
            )));
    }
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 1"));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 2"));
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user("Continue 3"));

    let mut event_generations = Vec::new();
    for turn in [
        "after first compact",
        "after second compact",
        "after third compact",
    ] {
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
        executor
            .compact_current_context(Some(progress_tx))
            .await
            .expect("manual compaction succeeds");

        while let Some(event) = progress_rx.recv().await {
            if let crate::agent::progress::AgentEvent::RuntimeCompactionCompleted {
                generation,
                ..
            } = event
            {
                event_generations.push(generation);
            }
        }

        // Add more large messages for the next compaction.
        for i in 0..3 {
            executor.session_mut().memory.add_message(
                crate::agent::memory::AgentMessage::user_turn(format!(
                    "{turn} extra {i}: {}",
                    "y".repeat(200)
                )),
            );
        }
    }

    // Block refs are monotonic (b1, b2, b3).
    assert_eq!(event_generations, vec![1, 2, 3]);
    assert!(
        executor
            .session()
            .memory
            .compaction_state()
            .has_active_blocks(),
        "should have active blocks after repeated compaction"
    );
}

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
#[tokio::test]
async fn background_writer_extracts_previous_message_for_empty_remember_payload() {
    let backend = Arc::new(InMemoryWikiBackend::default());
    let store_backend: Arc<dyn crate::agent::wiki_memory::WikiObjectBackend> = backend.clone();
    let wiki_store = crate::agent::wiki_memory::WikiStore::new(store_backend, "prod");
    let settings = Arc::new(crate::config::AgentSettings {
        agent_model_id: Some("mock-agent".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        wiki_memory_writer_enabled: Some(true),
        wiki_memory_writer_model_id: Some("mock-writer".to_string()),
        wiki_memory_writer_model_provider: Some("opencode-go".to_string()),
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
        .expect_complete_internal_text()
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
        .returning(|_, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));
    provider
        .expect_analyze_image()
        .returning(|_, _, _, _| Err(crate::llm::LlmError::unknown("Not implemented".to_string())));

    let mut llm = LlmClient::new(settings.as_ref());
    llm.register_provider("opencode-go".to_string(), Arc::new(provider));
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
        executor.agent_timeout_duration(AgentExecutionOptions::default()),
        std::time::Duration::from_secs(36_000)
    );
    assert_eq!(
        executor.agent_timeout_error_message(AgentExecutionOptions::default()),
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

#[cfg(oxide_module_llm_provider_opencode_go)]
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

#[cfg(oxide_module_llm_provider_opencode_go)]
#[tokio::test]
async fn new_task_admission_inline_for_normal_input() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"ok","awaiting_user_input":null}"#,
    );
    executor.session_mut().memory.set_max_tokens(5000);

    let result = executor.execute("Ship the feature", None).await;
    assert!(result.is_ok());

    let user_task_msg = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .find(|m| m.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .expect("UserTask message should exist");

    // Inline: content is the raw text, no externalized payload.
    assert_eq!(user_task_msg.content, "Ship the feature");
    assert!(user_task_msg.externalized_payload.is_none());
}

#[cfg(oxide_module_llm_provider_opencode_go)]
#[tokio::test]
async fn new_task_admission_manifest_for_oversized_input() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"ok","awaiting_user_input":null}"#,
    );
    // inline_threshold = max(2000, 5000/4) = 2000 tokens.
    // A ~12000-char varied-text string is ~3000 tokens → Manifest.
    executor.session_mut().memory.set_max_tokens(5000);

    let huge_task = "The quick brown fox jumps over the lazy dog. ".repeat(300);

    let result = executor.execute(&huge_task, None).await;
    assert!(result.is_ok());

    let user_task_msg = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .find(|m| m.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .expect("UserTask message should exist");

    // Manifest: content is bounded with manifest header, not the full raw text.
    assert!(user_task_msg.content.contains("[Externalized content"));
    // Manifest is ~1500 chars (head+tail preview + metadata); raw is ~13200 chars.
    assert!(user_task_msg.content.len() < huge_task.len() / 2);

    // Lossless raw content preserved in externalized_payload.
    assert!(user_task_msg.externalized_payload.is_some());
    let payload = user_task_msg.externalized_payload.as_ref().unwrap();
    let raw = payload
        .inline_fallback
        .as_ref()
        .expect("inline_fallback should be set");
    assert!(raw.contains(&huge_task));
}
