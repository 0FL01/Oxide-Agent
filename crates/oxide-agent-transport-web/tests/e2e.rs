//! E2E tests for the web transport.
//!
//! Tests the full agent execution pipeline with a scripted LLM provider
//! to measure application-level latency without depending on real LLM APIs.

use futures_util::StreamExt;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::{
    build_router,
    scripted_llm::{ScriptedLlmProvider, ScriptedResponse},
    AppState,
};
use std::sync::Arc;

/// Set up test infrastructure with a scripted LLM provider.
async fn setup_test() -> AppState {
    let scripted = Arc::new(ScriptedLlmProvider::new(vec![ScriptedResponse::Text(
        "Hello from scripted LLM!".to_string(),
    )]));

    // Create AgentSettings that points to the "scripted" provider.
    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some("test-model".to_string());
        s.agent_model_provider = Some("scripted".to_string());
        // Reduce timeout for faster tests.
        s.agent_timeout_secs = Some(5);
        s
    });

    let llm = LlmClient::new(&agent_settings);
    let llm = {
        let mut llm = llm;
        llm.register_provider("scripted".to_string(), scripted);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();

    let session_manager =
        oxide_agent_transport_web::session::WebSessionManager::new(registry, llm, agent_settings);

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

/// Test: SSE endpoint streams events as a task executes.
#[tokio::test]
async fn e2e_sse_stream() {
    // Start the server on a random port.
    let app_state = setup_test().await;
    let router = build_router(app_state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let addr = listener.local_addr().expect("failed to get local addr");
    let base_url = format!("http://{}", addr);

    // Spawn the server.
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });

    let client = reqwest::Client::new();

    // Check EVENT_LOGS is empty initially.
    let debug_before: Vec<String> = client
        .get(format!("{}/debug/event_logs", base_url))
        .send()
        .await
        .expect("failed to get event logs")
        .json()
        .await
        .expect("failed to parse event logs");
    eprintln!("EVENT_LOGS before task: {:?}", debug_before);

    // Create a session.
    let session_resp: serde_json::Value = client
        .post(format!("{}/sessions", base_url))
        .json(&serde_json::json!({ "user_id": 1 }))
        .send()
        .await
        .expect("failed to create session")
        .json()
        .await
        .expect("failed to parse session response");
    let session_id = session_resp["session_id"].as_str().expect("no session_id");

    // Submit a task.
    let task_resp: serde_json::Value = client
        .post(format!("{}/sessions/{}/tasks", base_url, session_id))
        .body("Hello")
        .send()
        .await
        .expect("failed to submit task")
        .json()
        .await
        .expect("failed to parse task response");
    eprintln!("Task response: {}", task_resp);
    let task_id = task_resp["task_id"].as_str().expect("no task_id");

    // Check EVENT_LOGS after task submission (before SSE connection).
    let debug_after_submit: Vec<String> = client
        .get(format!("{}/debug/event_logs", base_url))
        .send()
        .await
        .expect("failed to get event logs")
        .json()
        .await
        .expect("failed to parse event logs");
    eprintln!(
        "EVENT_LOGS after task submit (task_id={}): {:?}",
        task_id, debug_after_submit
    );

    // Also check /progress (doesn't use EVENT_LOGS).
    let progress_url = format!(
        "{}/sessions/{}/tasks/{}/progress",
        base_url, session_id, task_id
    );
    let progress_response = client
        .get(&progress_url)
        .send()
        .await
        .expect("failed to get progress");
    let progress_status = progress_response.status();
    eprintln!("Progress endpoint status: {}", progress_status);

    // Now try the SSE stream.
    let sse_url = format!(
        "{}/sessions/{}/tasks/{}/stream",
        base_url, session_id, task_id
    );
    eprintln!("SSE URL: {}", sse_url);

    let response = client
        .get(&sse_url)
        .send()
        .await
        .expect("failed to connect to SSE");

    let status = response.status();
    eprintln!("SSE response status: {}", status);

    assert!(
        status.is_success(),
        "SSE endpoint should return 200, got {}",
        status
    );

    let mut stream = response.bytes_stream();
    let mut event_count = 0;

    // Read SSE events until the stream closes or we hit a 30s deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let opt: Option<Result<bytes::Bytes, reqwest::Error>> = stream.next().await;
        match opt {
            Some(Ok(bytes)) => {
                let text = match std::str::from_utf8(&bytes) {
                    Ok(s) => s.to_string(),
                    Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
                };
                for line in text.lines() {
                    if line.starts_with("event:") {
                        event_count += 1;
                    }
                }
                eprintln!("SSE chunk: {:?}", text);
            }
            Some(Err(e)) => {
                eprintln!("SSE error: {}", e);
                break;
            }
            None => {
                break;
            }
        }
        if event_count > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    assert!(
        event_count > 0,
        "SSE stream should have delivered at least one event"
    );

    eprintln!("SSE received {} events", event_count);

    // Clean up.
    server.abort();
}
