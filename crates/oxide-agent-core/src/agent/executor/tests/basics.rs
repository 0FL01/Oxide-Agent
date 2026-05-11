use super::*;

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
