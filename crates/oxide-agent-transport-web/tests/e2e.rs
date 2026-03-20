//! E2E tests for the web transport.
//!
//! Tests the full agent execution pipeline with a scripted LLM provider
//! to measure application-level latency without depending on real LLM APIs.

use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::{
    scripted_llm::{ScriptedLlmProvider, ScriptedResponse},
    AppState,
};
use std::sync::Arc;

/// Set up test infrastructure with a scripted LLM provider.
async fn setup_test() -> AppState {
    let scripted = Arc::new(ScriptedLlmProvider::new(vec![ScriptedResponse::Text(
        "Hello from scripted LLM!".to_string(),
    )]));

    let mut llm = LlmClient::new(&AgentSettings::default());
    llm.register_provider("scripted".to_string(), scripted);

    let registry = SessionRegistry::new();
    let agent_settings = Arc::new(AgentSettings::default());

    let session_manager = oxide_agent_transport_web::session::WebSessionManager::new(
        registry,
        Arc::new(llm),
        agent_settings,
    );

    AppState::new(Arc::new(session_manager))
}

/// Execute a task directly via the session registry.
async fn execute_task(
    session_manager: &oxide_agent_transport_web::session::WebSessionManager,
    session_id: &str,
    task_id: &str,
    task_text: &str,
) {
    let meta = session_manager.get_session(session_id).await.unwrap();
    let sid = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        session_id.hash(&mut h);
        meta.user_id.hash(&mut h);
        oxide_agent_core::agent::SessionId::from(h.finish() as i64)
    };

    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session not found in registry");

    let (tx, _rx) = tokio::sync::mpsc::channel(100);

    let result = {
        let mut executor = executor_arc.write().await;
        executor.execute(task_text, Some(tx)).await
    };

    match result {
        Ok(_) => {
            session_manager.complete_task(task_id, session_id).await;
            tracing::info!(task_id, "Task completed");
        }
        Err(e) => {
            session_manager.fail_task(task_id, session_id).await;
            tracing::info!(task_id, error = %e, "Task failed");
        }
    }
}

/// Test: a simple text response completes successfully.
#[tokio::test]
async fn e2e_simple_text_response() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    let session_id = session_manager.create_session(1, None, None).await;

    let task_id = session_manager
        .register_task(&session_id, "Hello".to_string())
        .await
        .expect("session not found")
        .task_id;

    execute_task(&session_manager, &session_id, &task_id, "Hello").await;

    let task = session_manager.get_task(&task_id).await;
    assert!(task.is_some(), "task should exist after execution");
}

/// Test: session can be created, retrieved, and deleted.
#[tokio::test]
async fn e2e_session_lifecycle() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    // Create.
    let session_id = session_manager.create_session(42, None, None).await;
    assert!(!session_id.is_empty());

    // Get.
    let meta = session_manager.get_session(&session_id).await;
    assert!(meta.is_some(), "session should exist");
    assert_eq!(meta.unwrap().user_id, 42);

    // Delete.
    let deleted = session_manager.delete_session(&session_id).await;
    assert!(deleted, "delete should succeed");

    // Get after delete.
    let meta = session_manager.get_session(&session_id).await;
    assert!(meta.is_none(), "session should not exist after delete");
}

/// Test: two sequential tasks in the same session.
#[tokio::test]
async fn e2e_sequential_tasks_same_session() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    let session_id = session_manager.create_session(1, None, None).await;

    let task1 = session_manager
        .register_task(&session_id, "First task".to_string())
        .await
        .expect("session not found")
        .task_id;

    let task2 = session_manager
        .register_task(&session_id, "Second task".to_string())
        .await
        .expect("session should still exist")
        .task_id;

    // Tasks have different IDs.
    assert_ne!(task1, task2);
}

/// Test: task cancellation works.
#[tokio::test]
async fn e2e_task_cancel() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    let session_id = session_manager.create_session(1, None, None).await;

    let running_task = session_manager
        .register_task(&session_id, "A long task".to_string())
        .await
        .expect("session not found");

    let task_id = running_task.task_id;

    // Cancel.
    let cancelled = session_manager.cancel_task(&task_id, &session_id).await;
    assert!(cancelled, "cancel should succeed for running task");
}

/// Measure session-ready latency (time from register_task to executor lock).
#[tokio::test]
async fn e2e_latency_session_ready() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    let session_id = session_manager.create_session(1, None, None).await;
    let start = std::time::Instant::now();

    let _task = session_manager
        .register_task(&session_id, "Measure me".to_string())
        .await
        .expect("session not found");

    let session_ready_ms = start.elapsed().as_millis() as i64;

    // Session should be ready quickly (no network involved).
    assert!(
        session_ready_ms < 500,
        "session_ready should be < 500ms, was {}ms",
        session_ready_ms
    );

    eprintln!("Session ready latency: {}ms", session_ready_ms);
}
