//! SSE streaming and milestone tracking E2E tests.

use futures_util::StreamExt;
use oxide_agent_transport_web::build_router;

use crate::setup::setup_test;

/// Test: SSE endpoint streams events as a task executes and milestones are recorded.
#[tokio::test]
async fn e2e_sse_stream() {
    let test_start = std::time::Instant::now();

    let app_state = setup_test().await;
    let router = build_router(app_state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind");
    let addr = listener.local_addr().expect("failed to get local addr");
    let base_url = format!("http://{}", addr);

    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });

    let client = reqwest::Client::new();
    eprintln!(
        "[TIMING] Setup completed: {}ms",
        test_start.elapsed().as_millis()
    );

    let debug_before: Vec<String> = client
        .get(format!("{}/debug/event_logs", base_url))
        .send()
        .await
        .expect("failed to get event logs")
        .json()
        .await
        .expect("failed to parse event logs");
    eprintln!("EVENT_LOGS before task: {:?}", debug_before);

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
    eprintln!(
        "[TIMING] Task submitted: {}ms (since create: {}ms)",
        t1.elapsed().as_millis(),
        t0.elapsed().as_millis()
    );
    eprintln!("Task response: {}", task_resp);
    let task_id = task_resp["task_id"].as_str().expect("no task_id");

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
    eprintln!(
        "[TIMING] SSE connected: {}ms (since task submit: {}ms)",
        t2.elapsed().as_millis(),
        t1.elapsed().as_millis()
    );
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
                            eprintln!(
                                "[TIMING] First SSE event received: {:?} since SSE connect",
                                first_event_time
                            );
                        }
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
        serde_json::to_string_pretty(&timeline).expect("timeline should serialize to JSON")
    );

    let milestones = &timeline["milestones"];
    let session_ready_ms = milestones["session_ready_ms"].as_i64();
    let first_thinking_ms = milestones["first_thinking_ms"].as_i64();
    let final_response_ms = milestones["final_response_ms"].as_i64();

    assert!(
        session_ready_ms.is_some(),
        "session_ready_ms should be populated"
    );
    let session_ready_ms = session_ready_ms.expect("session_ready_ms must be present");
    assert!(
        session_ready_ms < 5000,
        "session_ready_ms should be under 5s, got {}ms",
        session_ready_ms
    );

    assert!(
        first_thinking_ms.is_some(),
        "first_thinking_ms should be populated"
    );
    let first_thinking_ms = first_thinking_ms.expect("first_thinking_ms must be present");
    assert!(
        first_thinking_ms >= 0,
        "first_thinking_ms should be non-negative, got {}ms",
        first_thinking_ms
    );

    assert!(
        final_response_ms.is_some(),
        "final_response_ms should be populated"
    );
    let final_response_ms = final_response_ms.expect("final_response_ms must be present");
    assert!(
        final_response_ms >= 0,
        "final_response_ms should be non-negative, got {}ms",
        final_response_ms
    );

    if first_thinking_ms > 0 {
        assert!(
            final_response_ms >= first_thinking_ms,
            "final_response_ms ({}) should be >= first_thinking_ms ({})",
            final_response_ms,
            first_thinking_ms
        );
    }

    eprintln!(
        "[TIMING] Total test time: {}ms",
        test_start.elapsed().as_millis()
    );
    eprintln!(
        "[BREAKDOWN] Setup: ~0ms | Create session: {}ms | Submit task: {}ms | SSE wait: {}ms",
        t0.elapsed().as_millis(),
        t1.elapsed().as_millis(),
        t2.elapsed().as_millis()
    );

    server.abort();
}
