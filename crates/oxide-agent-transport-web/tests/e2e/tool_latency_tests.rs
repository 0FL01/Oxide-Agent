//! Tool execution latency E2E tests.

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::scripted_llm::{
    ScriptedLlmProvider, ScriptedResponse, ScriptedToolCall,
};
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::AppState;
use std::sync::Arc;

use crate::setup::execute_task;

/// Test: measure latency with multiple sequential tool calls.
/// This test verifies that 3 tool calls execute and measures total time.
/// Currently tools execute sequentially; after optimization should be parallel.
#[tokio::test]
async fn e2e_parallel_tool_execution_latency() {
    let test_start = std::time::Instant::now();

    let scripted = Arc::new(ScriptedLlmProvider::new(vec![
        ScriptedResponse::ToolCalls {
            tool_calls: vec![
                ScriptedToolCall {
                    id: "call_1".to_string(),
                    name: "todos_write".to_string(),
                    arguments:
                        r#"{"todos":[{"id":"1","description":"Task 1","status":"pending"}]}"#
                            .to_string(),
                },
                ScriptedToolCall {
                    id: "call_2".to_string(),
                    name: "todos_write".to_string(),
                    arguments:
                        r#"{"todos":[{"id":"2","description":"Task 2","status":"pending"}]}"#
                            .to_string(),
                },
                ScriptedToolCall {
                    id: "call_3".to_string(),
                    name: "todos_write".to_string(),
                    arguments:
                        r#"{"todos":[{"id":"3","description":"Task 3","status":"pending"}]}"#
                            .to_string(),
                },
            ],
            final_text: None,
        },
        ScriptedResponse::Text("All tasks created".to_string()),
    ]));

    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("test-model".to_string()),
        agent_model_provider: Some("scripted".to_string()),
        agent_timeout_secs: Some(10),
        ..Default::default()
    });

    let llm = LlmClient::new(&agent_settings);
    let llm = {
        let mut llm = llm;
        llm.register_provider("scripted".to_string(), scripted);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    let app_state = AppState::new(Arc::new(session_manager));
    let session_manager = app_state.session_manager();

    let t0 = std::time::Instant::now();
    let session_id = session_manager.create_session(1, None, None).await;
    eprintln!(
        "[TIMING-parallel] Create session: {}ms",
        t0.elapsed().as_millis()
    );

    let t1 = std::time::Instant::now();
    let task_id = session_manager
        .register_task(&session_id, "Create multiple tasks".to_string())
        .await
        .expect("session not found")
        .task_id;
    eprintln!(
        "[TIMING-parallel] Register task: {}ms",
        t1.elapsed().as_millis()
    );

    let t2 = std::time::Instant::now();
    execute_task(
        &session_manager,
        &session_id,
        &task_id,
        "Create multiple tasks",
    )
    .await;
    let execution_time = t2.elapsed().as_millis();
    eprintln!(
        "[TIMING-parallel] Execute 3 tool calls: {}ms",
        execution_time
    );

    let task = session_manager.get_task(&task_id).await;
    assert!(task.is_some(), "task should exist after execution");

    eprintln!(
        "[TIMING-parallel] Total test time: {}ms",
        test_start.elapsed().as_millis()
    );
    eprintln!(
        "[RESULT] Tool execution latency baseline: {}ms for 3 sequential calls",
        execution_time
    );

    assert!(
        execution_time < 1000,
        "3 tool calls should complete in under 1000ms (current sequential baseline), took {}ms. After parallel optimization, this should drop to ~300ms!",
        execution_time
    );
}
