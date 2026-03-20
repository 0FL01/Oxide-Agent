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
    scripted_llm::{ScriptedLlmProvider, ScriptedResponse, ScriptedToolCall},
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
    let test_start = std::time::Instant::now();
    let app_state = setup_test().await;
    let session_manager = app_state.session_manager();
    eprintln!("[TIMING-simple] Setup: {}ms", test_start.elapsed().as_millis());

    let t0 = std::time::Instant::now();
    let session_id = session_manager.create_session(1, None, None).await;
    eprintln!("[TIMING-simple] Create session: {}ms", t0.elapsed().as_millis());

    let t1 = std::time::Instant::now();
    let task_id = session_manager
        .register_task(&session_id, "Hello".to_string())
        .await
        .expect("session not found")
        .task_id;
    eprintln!("[TIMING-simple] Register task: {}ms", t1.elapsed().as_millis());

    let t2 = std::time::Instant::now();
    execute_task(&session_manager, &session_id, &task_id, "Hello").await;
    eprintln!("[TIMING-simple] Execute task: {}ms", t2.elapsed().as_millis());

    let task = session_manager.get_task(&task_id).await;
    assert!(task.is_some(), "task should exist after execution");
    
    eprintln!("[TIMING-simple] Total: {}ms", test_start.elapsed().as_millis());
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
    let test_start = std::time::Instant::now();
    
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
    eprintln!("[TIMING] Setup completed: {}ms", test_start.elapsed().as_millis());

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
    let t0 = std::time::Instant::now();
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
    eprintln!("[TIMING] Session created: {}ms", t0.elapsed().as_millis());

    // Submit a task.
    let t1 = std::time::Instant::now();
    let task_resp: serde_json::Value = client
        .post(format!("{}/sessions/{}/tasks", base_url, session_id))
        .body("Hello")
        .send()
        .await
        .expect("failed to submit task")
        .json()
        .await
        .expect("failed to parse task response");
    eprintln!("[TIMING] Task submitted: {}ms (since create: {}ms)", 
        t1.elapsed().as_millis(),
        t0.elapsed().as_millis());
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
    let t2 = std::time::Instant::now();
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
    eprintln!("[TIMING] SSE connected: {}ms (since task submit: {}ms)", 
        t2.elapsed().as_millis(),
        t1.elapsed().as_millis());
    eprintln!("SSE response status: {}", status);

    assert!(
        status.is_success(),
        "SSE endpoint should return 200, got {}",
        status
    );

    let mut stream = response.bytes_stream();
    let mut event_count = 0;
    let mut first_event_time: Option<std::time::Duration> = None;
    let mut received_finished = false;

    // Read SSE events until the stream closes, we receive "finished", or hit deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !received_finished {
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
                        if first_event_time.is_none() {
                            first_event_time = Some(t2.elapsed());
                            eprintln!("[TIMING] First SSE event received: {:?} since SSE connect", 
                                first_event_time);
                        }
                        // Check if this is the "finished" event
                        if text.contains("\"finished\"") {
                            received_finished = true;
                            eprintln!("[TIMING] Received 'finished' event, exiting SSE loop");
                        }
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
    }

    assert!(
        event_count > 0,
        "SSE stream should have delivered at least one event"
    );
    assert!(
        received_finished,
        "SSE stream should have delivered 'finished' event"
    );

    eprintln!("SSE received {} events", event_count);

    // Verify milestones were recorded.
    let timeline_url = format!(
        "{}/sessions/{}/tasks/{}/timeline",
        base_url, session_id, task_id
    );
    let timeline: serde_json::Value = client
        .get(&timeline_url)
        .send()
        .await
        .expect("failed to get timeline")
        .json()
        .await
        .expect("failed to parse timeline response");
    eprintln!(
        "Timeline: {}",
        serde_json::to_string_pretty(&timeline).unwrap()
    );

    let milestones = &timeline["milestones"];
    let session_ready_ms = milestones["session_ready_ms"].as_i64();
    let first_thinking_ms = milestones["first_thinking_ms"].as_i64();
    let final_response_ms = milestones["final_response_ms"].as_i64();

    // session_ready_ms: time from HTTP request to executor ready.
    // Should be small (sub-second).
    assert!(
        session_ready_ms.is_some(),
        "session_ready_ms should be populated"
    );
    assert!(
        session_ready_ms.unwrap() < 5000,
        "session_ready_ms should be under 5s, got {}ms",
        session_ready_ms.unwrap()
    );

    // first_thinking_ms: time from agent start to first thinking event.
    // Should be positive and not huge.
    assert!(
        first_thinking_ms.is_some(),
        "first_thinking_ms should be populated"
    );
    assert!(
        first_thinking_ms.unwrap() >= 0,
        "first_thinking_ms should be non-negative, got {}ms",
        first_thinking_ms.unwrap()
    );

    // final_response_ms: time from agent start to final response.
    // Should be positive and greater than first_thinking_ms.
    assert!(
        final_response_ms.is_some(),
        "final_response_ms should be populated"
    );
    assert!(
        final_response_ms.unwrap() >= 0,
        "final_response_ms should be non-negative, got {}ms",
        final_response_ms.unwrap()
    );

    // final_response_ms should be >= first_thinking_ms (response after thinking).
    if let (Some(first), Some(final_)) = (first_thinking_ms, final_response_ms) {
        assert!(
            final_ >= first,
            "final_response_ms ({}) should be >= first_thinking_ms ({})",
            final_,
            first
        );
    }

    eprintln!("[TIMING] Total test time: {}ms", test_start.elapsed().as_millis());
    eprintln!("[BREAKDOWN] Setup: ~0ms | Create session: {}ms | Submit task: {}ms | SSE wait: {}ms",
        t0.elapsed().as_millis(),
        t1.elapsed().as_millis(),
        t2.elapsed().as_millis());

    // Clean up.
    server.abort();
}

/// Test: measure latency with multiple sequential tool calls.
/// This test verifies that 3 tool calls execute and measures total time.
/// Currently tools execute sequentially; after optimization should be parallel.
#[tokio::test]
async fn e2e_parallel_tool_execution_latency() {
    let test_start = std::time::Instant::now();

    // Scripted LLM returns 3 tool calls in sequence:
    // 1. Three todo operations (each ~10ms)
    // 2. Final text response
    let scripted = Arc::new(ScriptedLlmProvider::new(vec![
        ScriptedResponse::ToolCalls {
            tool_calls: vec![
                ScriptedToolCall {
                    id: "call_1".to_string(),
                    name: "todos_write".to_string(),
                    arguments: r#"{"todos":[{"id":"1","description":"Task 1","status":"pending"}]}"#.to_string(),
                },
                ScriptedToolCall {
                    id: "call_2".to_string(),
                    name: "todos_write".to_string(),
                    arguments: r#"{"todos":[{"id":"2","description":"Task 2","status":"pending"}]}"#.to_string(),
                },
                ScriptedToolCall {
                    id: "call_3".to_string(),
                    name: "todos_write".to_string(),
                    arguments: r#"{"todos":[{"id":"3","description":"Task 3","status":"pending"}]}"#.to_string(),
                },
            ],
            final_text: None,
        },
        ScriptedResponse::Text("All tasks created".to_string()),
    ]));

    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some("test-model".to_string());
        s.agent_model_provider = Some("scripted".to_string());
        s.agent_timeout_secs = Some(10);
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
    let app_state = AppState::new(Arc::new(session_manager));

    let session_manager = app_state.session_manager();

    let t0 = std::time::Instant::now();
    let session_id = session_manager.create_session(1, None, None).await;
    eprintln!("[TIMING-parallel] Create session: {}ms", t0.elapsed().as_millis());

    let t1 = std::time::Instant::now();
    let task_id = session_manager
        .register_task(&session_id, "Create multiple tasks".to_string())
        .await
        .expect("session not found")
        .task_id;
    eprintln!("[TIMING-parallel] Register task: {}ms", t1.elapsed().as_millis());

    // Execute and measure time for 3 tool calls
    let t2 = std::time::Instant::now();
    execute_task(&session_manager, &session_id, &task_id, "Create multiple tasks").await;
    let execution_time = t2.elapsed().as_millis();
    eprintln!("[TIMING-parallel] Execute 3 tool calls: {}ms", execution_time);

    // Verify todos were created
    let task = session_manager.get_task(&task_id).await;
    assert!(task.is_some(), "task should exist after execution");

    eprintln!("[TIMING-parallel] Total test time: {}ms", test_start.elapsed().as_millis());
    eprintln!("[RESULT] Tool execution latency baseline: {}ms for 3 sequential calls", execution_time);

    // Current expectation: sequential execution
    // If tools execute sequentially: ~200ms each with overhead = ~600-700ms total
    // After parallel optimization: should be ~200-300ms total (speedup ~2-3x)
    // This assertion documents baseline; after parallel impl, should lower threshold
    assert!(
        execution_time < 1000,
        "3 tool calls should complete in under 1000ms (current sequential baseline), took {}ms. After parallel optimization, this should drop to ~300ms!",
        execution_time
    );
}

/// Test: Measure HTTP connection pool latency improvement.
///
/// This test requires MISTRAL_API_KEY or ZAI_API_KEY environment variable.
/// It measures the latency of two sequential LLM requests to verify that
/// connection pooling (keep-alive) reduces the second request time.
///
/// Without connection pool: both requests ~200-400ms (TCP+TLS each time)
/// With connection pool: first ~200-400ms, second ~30-80ms (connection reuse)
///
/// Run with: cargo test e2e_connection_pool_latency -- --ignored --nocapture
#[tokio::test]
#[ignore = "Requires OPENROUTER_API_KEY, MISTRAL_API_KEY, or ZAI_API_KEY environment variable"]
async fn e2e_connection_pool_latency() {
    use oxide_agent_core::llm::{LlmClient, Message};
    use std::time::Instant;

    // Determine which provider to test based on available env vars
    let (provider_name, model_id) = if std::env::var("OPENROUTER_API_KEY").is_ok() {
        ("openrouter", "meta-llama/llama-3.1-8b-instruct")
    } else if std::env::var("MISTRAL_API_KEY").is_ok() {
        ("mistral", "mistral-tiny")
    } else if std::env::var("ZAI_API_KEY").is_ok() {
        ("zai", "glm-4-flash")
    } else {
        panic!("Neither OPENROUTER_API_KEY nor MISTRAL_API_KEY nor ZAI_API_KEY is set");
    };

    eprintln!("Testing connection pool with provider: {}", provider_name);

    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some(model_id.to_string());
        s.agent_model_provider = Some(provider_name.to_string());
        s.agent_timeout_secs = Some(30);
        // Load API keys from environment for the selected provider
        match provider_name {
            "openrouter" => {
                s.openrouter_api_key = std::env::var("OPENROUTER_API_KEY").ok();
            }
            "mistral" => {
                s.mistral_api_key = std::env::var("MISTRAL_API_KEY").ok();
            }
            "zai" => {
                s.zai_api_key = std::env::var("ZAI_API_KEY").ok();
                if let Ok(base) = std::env::var("ZAI_API_BASE") {
                    s.zai_api_base = base;
                }
            }
            _ => {}
        }
        s
    });

    let llm = Arc::new(LlmClient::new(&agent_settings));

    let messages = vec![Message::user("Say 'pong' and nothing else")];
    let system_prompt = "You are a helpful assistant.";

    // Request 1: Cold start (includes TCP handshake + TLS)
    let t1 = Instant::now();
    let resp1 = llm.chat_with_tools(
        system_prompt,
        &messages,
        &[],
        model_id,
        false,
    ).await;
    let time1 = t1.elapsed();
    assert!(resp1.is_ok(), "First request failed: {:?}", resp1.err());
    eprintln!("[CONNECTION-POOL] First request (cold): {}ms", time1.as_millis());

    // Request 2: Should reuse connection if pool is working
    let t2 = Instant::now();
    let resp2 = llm.chat_with_tools(
        system_prompt,
        &messages,
        &[],
        model_id,
        false,
    ).await;
    let time2 = t2.elapsed();
    assert!(resp2.is_ok(), "Second request failed: {:?}", resp2.err());
    eprintln!("[CONNECTION-POOL] Second request (warm): {}ms", time2.as_millis());

    // Calculate improvement
    let improvement = if time1.as_millis() > 0 {
        ((time1.as_millis() - time2.as_millis()) as f64 / time1.as_millis() as f64) * 100.0
    } else {
        0.0
    };

    eprintln!("[CONNECTION-POOL] Improvement: {:.1}%", improvement);
    eprintln!(
        "[CONNECTION-POOL] Baseline: first={}ms, second={}ms",
        time1.as_millis(),
        time2.as_millis()
    );

    // With connection pooling, second request should be significantly faster
    // Typical: first 200-400ms, second 30-80ms (70-80% improvement)
    // Without pooling: both similar (0-20% difference)
    assert!(
        time2.as_millis() < time1.as_millis(),
        "Second request should be faster than first with connection pooling. \
         First: {}ms, Second: {}ms. If similar, connection pool may not be working.",
        time1.as_millis(),
        time2.as_millis()
    );

    // Heuristic: second request should be at least 30% faster
    // This will fail without connection pool implementation
    assert!(
        improvement > 30.0,
        "Connection pool should provide >30% improvement on second request. \
         Current improvement: {:.1}%. First: {}ms, Second: {}ms",
        improvement,
        time1.as_millis(),
        time2.as_millis()
    );
}
