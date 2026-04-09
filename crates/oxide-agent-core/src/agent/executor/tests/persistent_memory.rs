use super::*;

#[tokio::test]
async fn execute_persists_episode_after_final_response() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let store_for_coordinator: Arc<dyn crate::agent::persistent_memory::PersistentMemoryStore> =
        store.clone();
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"persisted ok","awaiting_user_input":null}"#,
    )
    .with_persistent_memory_store(store.clone());
    executor.memory_classifier = Some(Arc::new(StubMemoryTaskClassifier::success(
        super::super::types::retrieval_fallback_classification(),
    )));
    executor.persistent_memory = Some(
        PersistentMemoryCoordinator::new(store_for_coordinator)
            .with_memory_writer(Arc::new(StubPostRunMemoryWriter)),
    );

    let result = executor.execute("persist final response", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "persisted ok"
    ));

    let task_id = executor
        .session()
        .current_task_id
        .clone()
        .expect("task id should be recorded");
    let episode = store
        .get_episode(&task_id)
        .await
        .expect("episode lookup should succeed")
        .expect("episode should exist");
    assert_eq!(episode.goal, "persist final response");

    let session_id = executor.session().session_id.to_string();
    let state = store
        .get_session_state(&session_id)
        .await
        .expect("session state lookup should succeed")
        .expect("session state should exist");
    assert_eq!(state.cleanup_status, CleanupStatus::Finalized);
    assert_eq!(state.pending_episode_id, None);
}

#[tokio::test]
async fn execute_waiting_for_user_input_updates_session_state_without_episode() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"blocked","tool_call":null,"final_answer":null,"awaiting_user_input":{"kind":"text","prompt":"Send more details"}}"#,
    )
    .with_persistent_memory_store(store.clone());

    let result = executor.execute("need more details", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::WaitingForUserInput(ref request))
            if request.kind == UserInputKind::Text
                && request.prompt == "Send more details"
    ));

    let task_id = executor
        .session()
        .current_task_id
        .clone()
        .expect("task id should be recorded");
    assert!(store
        .get_episode(&task_id)
        .await
        .expect("episode lookup should succeed")
        .is_none());

    let session_id = executor.session().session_id.to_string();
    let state = store
        .get_session_state(&session_id)
        .await
        .expect("session state lookup should succeed")
        .expect("session state should exist");
    assert_eq!(state.cleanup_status, CleanupStatus::Idle);
    assert_eq!(state.pending_episode_id.as_deref(), Some(task_id.as_str()));
}

#[tokio::test]
async fn execute_post_run_cleanup_leaves_small_hot_context() {
    let store = Arc::new(InMemoryMemoryRepository::new());
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"cleanup complete","awaiting_user_input":null}"#,
    )
    .with_persistent_memory_store(store);

    for idx in 0..36 {
        executor
            .session_mut()
            .memory
            .add_message(crate::agent::memory::AgentMessage::user(verbose_turn(
                "user-context",
                idx,
                220,
            )));
        executor
            .session_mut()
            .memory
            .add_message(crate::agent::memory::AgentMessage::assistant(verbose_turn(
                "assistant-context",
                idx,
                220,
            )));
    }

    let tokens_before = executor.session().memory.token_count();
    assert!(
        tokens_before > 16 * 1024,
        "fixture should exceed the Stage 19 residual budget target"
    );

    let result = executor.execute("verify post-run cleanup", None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "cleanup complete"
    ));

    let tokens_after = executor.session().memory.token_count();
    assert!(
        tokens_after <= 16 * 1024,
        "post-run cleanup should leave a small hot context (before={tokens_before}, after={tokens_after})"
    );
    assert!(
        executor
            .session()
            .memory
            .get_messages()
            .iter()
            .any(|message| message.kind == crate::agent::compaction::AgentMessageKind::Summary),
        "post-run cleanup should retain a structured summary"
    );
}

#[tokio::test]
async fn prepare_execution_injects_durable_memory_context() {
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_episodes_lexical()
        .times(1)
        .return_once(|_, _| {
            Ok(vec![oxide_agent_memory::EpisodeSearchHit {
                record: oxide_agent_memory::EpisodeRecord {
                    episode_id: "episode-1".to_string(),
                    thread_id: "thread-1".to_string(),
                    context_key: "session:9".to_string(),
                    goal: "Fix deploy regression".to_string(),
                    summary: "Earlier deploy broke staging until config was corrected.".to_string(),
                    outcome: oxide_agent_memory::EpisodeOutcome::Success,
                    tools_used: vec!["memory_search".to_string()],
                    artifacts: Vec::new(),
                    failures: Vec::new(),
                    importance: 0.82,
                    created_at: chrono::Utc::now(),
                },
                score: 0.7,
                snippet: "episode hit".to_string(),
            }])
        });
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .return_once(|_, _| {
            Ok(vec![oxide_agent_memory::MemorySearchHit {
                record: oxide_agent_memory::MemoryRecord {
                    memory_id: "memory-1".to_string(),
                    context_key: "session:9".to_string(),
                    source_episode_id: Some("episode-9".to_string()),
                    memory_type: oxide_agent_memory::MemoryType::Procedure,
                    title: "Deploy fix procedure".to_string(),
                    content: "Rebuild config, then rerun the deploy with the staging profile."
                        .to_string(),
                    short_description: "staging recovery steps".to_string(),
                    importance: 0.95,
                    confidence: 0.91,
                    source: Some("test".to_string()),
                    content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                        oxide_agent_memory::MemoryType::Procedure,
                        "Rebuild config, then rerun the deploy with the staging profile.",
                    )),
                    reason: Some("fixture".to_string()),
                    tags: vec!["deploy".to_string()],
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    deleted_at: None,
                },
                score: 0.9,
                snippet: "memory hit".to_string(),
            }])
        });

    let mut executor = build_executor().with_storage_memory_repository(Arc::new(storage));
    executor.memory_classifier = Some(Arc::new(StubMemoryTaskClassifier::success(
        super::super::types::retrieval_fallback_classification(),
    )));
    let prepared = executor
        .prepare_execution("how was the deploy fixed before?", None)
        .await;

    let injected = prepared
        .messages
        .iter()
        .find(|message| {
            message.role == "system" && message.content.contains("Scoped durable memory context")
        })
        .expect("durable memory context should be injected");
    assert!(injected.content.contains("memory memory-1"));
    assert!(injected.content.contains("episode episode-1"));
}

#[tokio::test]
async fn prepare_execution_uses_retrieval_fallback_when_classifier_fails() {
    let mut storage = MockStorageProvider::new();
    storage
        .expect_search_memory_episodes_lexical()
        .times(1)
        .return_once(|_, _| {
            Ok(vec![oxide_agent_memory::EpisodeSearchHit {
                record: oxide_agent_memory::EpisodeRecord {
                    episode_id: "episode-1".to_string(),
                    thread_id: "thread-1".to_string(),
                    context_key: "session:9".to_string(),
                    goal: "Fix deploy regression".to_string(),
                    summary: "Earlier deploy broke staging until config was corrected.".to_string(),
                    outcome: oxide_agent_memory::EpisodeOutcome::Success,
                    tools_used: vec!["memory_search".to_string()],
                    artifacts: Vec::new(),
                    failures: Vec::new(),
                    importance: 0.82,
                    created_at: chrono::Utc::now(),
                },
                score: 0.7,
                snippet: "episode hit".to_string(),
            }])
        });
    storage
        .expect_search_memory_records_lexical()
        .times(1)
        .return_once(|_, _| {
            Ok(vec![oxide_agent_memory::MemorySearchHit {
                record: oxide_agent_memory::MemoryRecord {
                    memory_id: "memory-1".to_string(),
                    context_key: "session:9".to_string(),
                    source_episode_id: Some("episode-9".to_string()),
                    memory_type: oxide_agent_memory::MemoryType::Procedure,
                    title: "Deploy fix procedure".to_string(),
                    content: "Rebuild config, then rerun the deploy with the staging profile."
                        .to_string(),
                    short_description: "staging recovery steps".to_string(),
                    importance: 0.95,
                    confidence: 0.91,
                    source: Some("test".to_string()),
                    content_hash: Some(oxide_agent_memory::stable_memory_content_hash(
                        oxide_agent_memory::MemoryType::Procedure,
                        "Rebuild config, then rerun the deploy with the staging profile.",
                    )),
                    reason: Some("fixture".to_string()),
                    tags: vec!["deploy".to_string()],
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    deleted_at: None,
                },
                score: 0.9,
                snippet: "memory hit".to_string(),
            }])
        });

    let mut executor = build_executor().with_storage_memory_repository(Arc::new(storage));
    executor.memory_classifier = Some(Arc::new(StubMemoryTaskClassifier::failure(
        anyhow::anyhow!("classifier route misconfigured"),
    )));

    let prepared = executor
        .prepare_execution("how was the deploy fixed before?", None)
        .await;

    let injected = prepared
        .messages
        .iter()
        .find(|message| {
            message.role == "system" && message.content.contains("Scoped durable memory context")
        })
        .expect("retrieval fallback should still inject durable memory context");
    assert!(injected.content.contains("memory memory-1"));
    assert!(injected.content.contains("episode episode-1"));

    let stored_classification = prepared
        .memory_classification
        .expect("prepared execution should keep write-safe fallback classification");
    assert!(!stored_classification.read_policy.inject_prompt_memory);
    assert!(!stored_classification.write_policy.allow_llm_durable_writes);
}
