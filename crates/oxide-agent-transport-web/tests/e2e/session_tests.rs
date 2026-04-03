//! Session-level E2E tests.

use super::setup::{execute_task, setup_test};

/// Test: a simple text response completes successfully.
#[tokio::test]
async fn e2e_simple_text_response() {
    let test_start = std::time::Instant::now();
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();
    eprintln!(
        "[TIMING-simple] Setup: {}ms",
        test_start.elapsed().as_millis()
    );

    let t0 = std::time::Instant::now();
    let session_id = session_manager.create_session(1, None, None).await;
    eprintln!(
        "[TIMING-simple] Create session: {}ms",
        t0.elapsed().as_millis()
    );

    let t1 = std::time::Instant::now();
    let task_id = session_manager
        .register_task(&session_id, "Hello".to_string())
        .await
        .expect("session not found")
        .task_id;
    eprintln!(
        "[TIMING-simple] Register task: {}ms",
        t1.elapsed().as_millis()
    );

    let t2 = std::time::Instant::now();
    execute_task(&session_manager, &session_id, &task_id, "Hello").await;
    eprintln!(
        "[TIMING-simple] Execute task: {}ms",
        t2.elapsed().as_millis()
    );

    let task = session_manager.get_task(&task_id).await;
    assert!(task.is_some(), "task should exist after execution");

    eprintln!(
        "[TIMING-simple] Total: {}ms",
        test_start.elapsed().as_millis()
    );
}

/// Test: session can be created, retrieved, and deleted.
#[tokio::test]
async fn e2e_session_lifecycle() {
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();

    let session_id = session_manager.create_session(42, None, None).await;
    assert!(!session_id.is_empty());

    let meta = session_manager.get_session(&session_id).await;
    let meta = meta.expect("session should exist after creation");
    assert_eq!(meta.user_id, 42);

    let deleted = session_manager.delete_session(&session_id).await;
    assert!(deleted, "delete should succeed");

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

    assert!(
        session_ready_ms < 500,
        "session_ready should be < 500ms, was {}ms",
        session_ready_ms
    );

    eprintln!("Session ready latency: {}ms", session_ready_ms);
}
