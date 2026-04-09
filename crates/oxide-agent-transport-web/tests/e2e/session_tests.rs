//! Session-level E2E tests.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use oxide_agent_core::agent::SessionId;

use super::helpers::{
    create_session_http_with_user, create_task_http_with_body, fetch_task_events,
    structured_awaiting_user_input_response, structured_final_answer_response,
    unstructured_text_response, wait_for_task_status, wait_for_zai_calls,
};
use super::providers::{ControlledNarratorProvider, RecordedToolRequest, SequencedZaiProvider};
use super::setup::{
    execute_task, setup_test, setup_web_test_with_custom_providers,
    setup_web_test_with_structured_main_provider,
};

fn derive_session_id(session_id: &str, user_id: i64) -> SessionId {
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    user_id.hash(&mut h);
    SessionId::from(h.finish() as i64)
}

fn request_contains(request: &RecordedToolRequest, needle: &str) -> bool {
    request.system_prompt.contains(needle)
        || request
            .messages
            .iter()
            .any(|message| message.content.contains(needle))
}

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

#[tokio::test]
async fn e2e_runtime_context_appended_on_next_iteration() {
    let zai_provider = Arc::new(
        SequencedZaiProvider::new(vec![
            super::helpers::tool_call_response(
                "write_todos",
                serde_json::json!({
                    "todos": [{
                        "description": "Investigate GPT-5.4-mini limits",
                        "status": "in_progress"
                    }]
                }),
            ),
            unstructured_text_response("updated answer with clarified GPT-5.4-mini scope"),
            unstructured_text_response("updated answer with clarified GPT-5.4-mini scope"),
        ])
        .with_blocked_calls([1]),
    );
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_custom_providers(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260409;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Investigate weekly Codex limits",
    )
    .await;

    wait_for_zai_calls(&zai_provider, 1, Duration::from_secs(2)).await;

    let sid = derive_session_id(&session_id, user_id);
    assert!(
        session_manager
            .session_registry()
            .enqueue_runtime_context(
                &sid,
                "Clarification: речь именно об GPT-5.4 mini".to_string(),
            )
            .await,
        "runtime context should be queued for the active session"
    );

    zai_provider.release_call(1);

    wait_for_zai_calls(&zai_provider, 2, Duration::from_secs(2)).await;
    let requests = zai_provider.request_log().await;
    assert!(
        requests.len() >= 2,
        "expected a follow-up LLM call after append"
    );
    assert!(
        request_contains(&requests[1], "Clarification: речь именно об GPT-5.4 mini"),
        "second request should include appended runtime context"
    );

    let _ = session_manager.cancel_task(&task_id, &session_id).await;

    server.abort();
}

#[tokio::test]
async fn e2e_web_followup_while_running_becomes_separate_task() {
    let zai_provider = Arc::new(
        SequencedZaiProvider::new(vec![
            unstructured_text_response("first answer"),
            unstructured_text_response("second answer"),
        ])
        .with_blocked_calls([1]),
    );
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_custom_providers(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260410;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
    let task1_id =
        create_task_http_with_body(&client, &base_url, &session_id, "Initial prompt").await;

    wait_for_zai_calls(&zai_provider, 1, Duration::from_secs(2)).await;

    let task2_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Follow-up clarification while the first task is still running",
    )
    .await;
    assert_ne!(
        task1_id, task2_id,
        "web transport currently creates a separate task"
    );

    zai_provider.release_call(1);

    wait_for_task_status(
        session_manager.as_ref(),
        &task1_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_task_status(
        session_manager.as_ref(),
        &task2_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 2, Duration::from_secs(2)).await;

    let requests = zai_provider.request_log().await;
    assert_eq!(
        requests.len(),
        2,
        "expected two separate top-level requests"
    );
    assert!(request_contains(&requests[0], "Initial prompt"));
    assert!(request_contains(
        &requests[1],
        "Follow-up clarification while the first task is still running"
    ));

    let task1_events = fetch_task_events(&client, &base_url, &session_id, &task1_id).await;
    let task1_event_names: Vec<&str> = task1_events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect();
    assert!(
        !task1_event_names.iter().any(|event| event.contains("continuation")),
        "web transport should expose the current gap: follow-up becomes a separate task, not a continuation"
    );

    server.abort();
}

#[tokio::test]
async fn e2e_resume_after_user_input_reuses_saved_task() {
    let provider = Arc::new(SequencedZaiProvider::new(vec![
        structured_awaiting_user_input_response("text", "Send the exact GPT-5.4-mini scope."),
        structured_final_answer_response("resumed with clarified GPT-5.4-mini scope"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_structured_main_provider(provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260411;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
    let task_id =
        create_task_http_with_body(&client, &base_url, &session_id, "Investigate Codex limits")
            .await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_zai_calls(&provider, 1, Duration::from_secs(2)).await;

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");

    {
        let executor = executor_arc.read().await;
        assert!(
            executor.session().pending_user_input().is_some(),
            "session should be waiting for user input after the first run"
        );
    }

    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let outcome = {
        let mut executor = executor_arc.write().await;
        executor
            .resume_after_user_input(
                "The clarification is specifically about GPT-5.4 mini".to_string(),
                Some(tx),
            )
            .await
            .expect("resume should succeed")
    };

    assert!(matches!(
        outcome,
        oxide_agent_core::agent::AgentExecutionOutcome::Completed(_)
    ));
    wait_for_zai_calls(&provider, 2, Duration::from_secs(2)).await;

    {
        let executor = executor_arc.read().await;
        assert!(
            executor.session().pending_user_input().is_none(),
            "pending user input should be cleared after resume"
        );
    }

    let requests = provider.request_log().await;
    assert_eq!(requests.len(), 2);
    assert!(request_contains(&requests[0], "Investigate Codex limits"));
    assert!(request_contains(
        &requests[1],
        "The clarification is specifically about GPT-5.4 mini"
    ));

    server.abort();
}
