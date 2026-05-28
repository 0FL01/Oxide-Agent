//! SSE streaming and milestone tracking E2E tests.

use futures_util::StreamExt;

use crate::helpers::{
    create_session_http, create_task_http_with_body, fetch_task_progress, fetch_task_timeline,
    spawn_test_server, with_session_auth,
};
use crate::setup::setup_test;

/// Test: SSE endpoint streams events as a task executes and milestones are recorded.
#[tokio::test]
#[cfg_attr(not(feature = "socket_e2e"), ignore = "requires local TCP listener")]
async fn e2e_sse_stream() {
    let test_start = std::time::Instant::now();

    let app_state = setup_test().await;
    let (server, base_url) = spawn_test_server(app_state.clone()).await;

    let client = reqwest::Client::new();
    eprintln!(
        "[TIMING] Setup completed: {}ms",
        test_start.elapsed().as_millis()
    );

    let t0 = std::time::Instant::now();
    let session_id = create_session_http(&client, &base_url).await;
    eprintln!("[TIMING] Session created: {}ms", t0.elapsed().as_millis());

    let t1 = std::time::Instant::now();
    let task_id = create_task_http_with_body(&client, &base_url, &session_id, "Hello").await;
    eprintln!(
        "[TIMING] Task submitted: {}ms (since create: {}ms)",
        t1.elapsed().as_millis(),
        t0.elapsed().as_millis()
    );
    eprintln!("Task submitted: {task_id}");

    let progress_response = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    let progress_status = progress_response.status();
    eprintln!("Progress endpoint status: {}", progress_status);

    let t2 = std::time::Instant::now();
    let sse_url = format!(
        "{}/api/v1/sessions/{}/tasks/{}/stream",
        base_url, session_id, task_id
    );
    eprintln!("SSE URL: {}", sse_url);

    let response = with_session_auth(client.get(&sse_url), &base_url, &session_id, false)
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

    let timeline = fetch_task_timeline(&client, &base_url, &session_id, &task_id).await;
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
