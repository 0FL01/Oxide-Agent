//! Helper functions for E2E tests: response builders, polling helpers, HTTP helpers.

use oxide_agent_core::llm::{
    ChatResponse, TokenUsage, ToolCall, ToolCallCorrelation, ToolCallFunction,
};
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::AppState;
use std::time::{Duration, Instant};

/// Build a tool-call ChatResponse.
pub fn tool_call_response(name: &str, arguments: serde_json::Value) -> ChatResponse {
    let invocation_id = format!("call-{name}");

    ChatResponse {
        content: None,
        tool_calls: vec![ToolCall::new(
            invocation_id.clone(),
            ToolCallFunction {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
            false,
        )
        .with_correlation(
            ToolCallCorrelation::new(invocation_id)
                .with_provider_tool_call_id(format!("sequenced-{name}")),
        )],
        finish_reason: "tool_calls".to_string(),
        reasoning_content: None,
        usage: Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }),
    }
}

/// Build an unstructured text ChatResponse.
pub fn unstructured_text_response(content: &str) -> ChatResponse {
    ChatResponse {
        content: Some(content.to_string()),
        tool_calls: Vec::new(),
        finish_reason: "stop".to_string(),
        reasoning_content: None,
        usage: Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }),
    }
}

/// Build a structured final-answer ChatResponse.
pub fn structured_final_answer_response(final_answer: &str) -> ChatResponse {
    unstructured_text_response(
        &serde_json::json!({
            "thought": "done",
            "tool_call": null,
            "final_answer": final_answer,
            "awaiting_user_input": null,
        })
        .to_string(),
    )
}

/// Build a structured awaiting-user-input ChatResponse.
pub fn structured_awaiting_user_input_response(kind: &str, prompt: &str) -> ChatResponse {
    unstructured_text_response(
        &serde_json::json!({
            "thought": "blocked_on_user",
            "tool_call": null,
            "final_answer": null,
            "awaiting_user_input": {
                "kind": kind,
                "prompt": prompt,
            },
        })
        .to_string(),
    )
}

/// Wait until the narrator provider reaches at least `minimum_calls`.
pub async fn wait_for_narrator_calls(
    narrator_provider: &super::providers::ControlledNarratorProvider,
    minimum_calls: usize,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while narrator_provider.call_count() < minimum_calls {
        assert!(
            Instant::now() < deadline,
            "narrator did not reach {minimum_calls} calls in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until the ZAI provider reaches at least `minimum_calls`.
pub async fn wait_for_zai_calls(
    zai_provider: &super::providers::SequencedZaiProvider,
    minimum_calls: usize,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if zai_provider.model_log().await.len() >= minimum_calls {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "zai provider did not reach {minimum_calls} calls in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until a task reaches the expected status.
pub async fn wait_for_task_status(
    session_manager: &WebSessionManager,
    task_id: &str,
    expected: oxide_agent_transport_web::session::TaskStatus,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let task = session_manager
            .get_task(task_id)
            .await
            .expect("task metadata should exist while polling");

        match (&task.status, &expected) {
            (
                oxide_agent_transport_web::session::TaskStatus::Completed,
                oxide_agent_transport_web::session::TaskStatus::Completed,
            ) => return,
            (
                oxide_agent_transport_web::session::TaskStatus::Running,
                oxide_agent_transport_web::session::TaskStatus::Running,
            ) => return,
            (
                oxide_agent_transport_web::session::TaskStatus::Cancelled,
                oxide_agent_transport_web::session::TaskStatus::Cancelled,
            ) => return,
            (
                oxide_agent_transport_web::session::TaskStatus::Failed,
                oxide_agent_transport_web::session::TaskStatus::Failed,
            ) => return,
            (
                oxide_agent_transport_web::session::TaskStatus::Completed
                | oxide_agent_transport_web::session::TaskStatus::Cancelled
                | oxide_agent_transport_web::session::TaskStatus::Failed,
                _,
            ) => {
                panic!(
                    "task {task_id} reached terminal status {:?} while waiting for {:?}",
                    task.status, expected
                );
            }
            _ => {}
        }

        assert!(
            Instant::now() < deadline,
            "task {task_id} did not reach expected status in time; current status={:?}",
            task.status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Fetch task events via HTTP.
pub async fn fetch_task_events(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    task_id: &str,
) -> Vec<serde_json::Value> {
    client
        .get(format!(
            "{base_url}/sessions/{session_id}/tasks/{task_id}/events"
        ))
        .send()
        .await
        .expect("failed to fetch task events")
        .json()
        .await
        .expect("failed to decode task events")
}

/// Fetch task progress via HTTP.
pub async fn fetch_task_progress(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    task_id: &str,
) -> reqwest::Response {
    client
        .get(format!(
            "{base_url}/sessions/{session_id}/tasks/{task_id}/progress"
        ))
        .send()
        .await
        .expect("failed to fetch task progress")
}

/// Fetch task timeline via HTTP.
pub async fn fetch_task_timeline(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    task_id: &str,
) -> serde_json::Value {
    client
        .get(format!(
            "{base_url}/sessions/{session_id}/tasks/{task_id}/timeline"
        ))
        .send()
        .await
        .expect("failed to fetch task timeline")
        .json()
        .await
        .expect("failed to decode task timeline")
}

/// Create a session via HTTP.
pub async fn create_session_http(client: &reqwest::Client, base_url: &str) -> String {
    create_session_http_with_user(client, base_url, 1).await
}

/// Create a session via HTTP for a specific user.
pub async fn create_session_http_with_user(
    client: &reqwest::Client,
    base_url: &str,
    user_id: i64,
) -> String {
    create_session_http_with_user_and_context(client, base_url, user_id, None).await
}

/// Create a session via HTTP for a specific user and context.
pub async fn create_session_http_with_user_and_context(
    client: &reqwest::Client,
    base_url: &str,
    user_id: i64,
    context_key: Option<&str>,
) -> String {
    let response: serde_json::Value = client
        .post(format!("{base_url}/sessions"))
        .json(&serde_json::json!({
            "user_id": user_id,
            "context_key": context_key,
        }))
        .send()
        .await
        .expect("failed to create session")
        .json()
        .await
        .expect("failed to decode session response");

    response["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string()
}

/// Create a task via HTTP.
pub async fn create_task_http(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
) -> String {
    create_task_http_with_body(client, base_url, session_id, "Investigate package status").await
}

/// Create a task via HTTP with a custom plain-text body.
pub async fn create_task_http_with_body(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    body: &str,
) -> String {
    let response: serde_json::Value = client
        .post(format!("{base_url}/sessions/{session_id}/tasks"))
        .body(body.to_string())
        .send()
        .await
        .expect("failed to create task")
        .json()
        .await
        .expect("failed to decode task response");

    response["task_id"]
        .as_str()
        .expect("task_id missing")
        .to_string()
}

/// Spawn an HTTP test server and return the server handle and base URL.
pub async fn spawn_test_server(app_state: AppState) -> (tokio::task::JoinHandle<()>, String) {
    let router = oxide_agent_transport_web::build_router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test server");
    let addr = listener
        .local_addr()
        .expect("failed to get test server addr");
    let base_url = format!("http://{addr}");

    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("test server failed");
    });

    (server, base_url)
}

#[cfg(test)]
mod tests {
    use super::tool_call_response;

    #[test]
    fn tool_call_response_attaches_explicit_correlation() {
        let response = tool_call_response("reminder_schedule", serde_json::json!({"kind": "once"}));
        let tool_call = response.tool_calls.first().expect("tool call present");
        let correlation = tool_call.correlation();

        assert_eq!(tool_call.invocation_id().as_str(), "call-reminder_schedule");
        assert_eq!(
            correlation.wire_tool_call_id(),
            "sequenced-reminder_schedule"
        );
    }
}
