#![cfg_attr(not(oxide_module_llm_provider_opencode_go), allow(dead_code))]

use super::*;
use crate::agent::{AgentExecutionEffort, AgentExecutionOptions};

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
    let payload = user_task_msg
        .externalized_payload
        .as_ref()
        .expect("manifest payload should be attached");
    let raw = payload
        .inline_fallback
        .as_ref()
        .expect("inline_fallback should be set");
    assert!(raw.contains(&huge_task));
}
