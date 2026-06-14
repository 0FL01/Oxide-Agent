//! Helper functions for E2E tests: response builders, polling helpers, HTTP helpers.

use oxide_agent_core::llm::{
    ChatResponse, TokenUsage, ToolCall, ToolCallCorrelation, ToolCallFunction,
};
use oxide_agent_transport_web::AppState;
use oxide_agent_transport_web::auth::{login_user, register_user};
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_web_contracts::{LoginRequest, RegisterRequest};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const TEST_AUTH_COOKIE_NAME: &str = "oxide_web_session";
const TEST_USER_PASSWORD: &str = "correct horse battery staple e2e";

#[derive(Clone)]
struct TestAuthSession {
    user_id: i64,
    raw_token: String,
    csrf_token: String,
}

pub struct JsonHttpResponse {
    status: reqwest::StatusCode,
    body: serde_json::Value,
}

impl JsonHttpResponse {
    #[must_use]
    pub const fn status(&self) -> reqwest::StatusCode {
        self.status
    }

    pub async fn json<T: DeserializeOwned>(self) -> serde_json::Result<T> {
        serde_json::from_value(self.body)
    }
}

/// Build a tool-call ChatResponse.
pub fn tool_call_response(name: &str, arguments: serde_json::Value) -> ChatResponse {
    let invocation_id = format!("call-{name}");

    ChatResponse {
        content: None,
        tool_calls: vec![
            ToolCall::new(
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
            ),
        ],
        finish_reason: "tool_calls".to_string(),
        reasoning_content: None,
        usage: Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..TokenUsage::default()
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
            ..TokenUsage::default()
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

/// Wait until the LLM provider reaches at least `minimum_calls`.
pub async fn wait_for_llm_calls(
    llm_provider: &super::providers::SequencedLlmProvider,
    minimum_calls: usize,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        if llm_provider.model_log().await.len() >= minimum_calls {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "LLM provider did not reach {minimum_calls} calls in time"
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
            _ => {}
        }

        if Instant::now() >= deadline {
            panic!(
                "task {task_id} did not reach expected status {expected:?} in time; last_status={:?}",
                task.status
            );
        }
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
    let response: serde_json::Value = with_session_auth(
        client.get(format!(
            "{base_url}/api/v1/sessions/{session_id}/tasks/{task_id}/events"
        )),
        base_url,
        session_id,
        false,
    )
    .send()
    .await
    .expect("failed to fetch task events")
    .json()
    .await
    .expect("failed to decode task events");
    response["events"]
        .as_array()
        .expect("task events response should contain events")
        .iter()
        .map(normalize_persisted_event_for_legacy_assertions)
        .collect()
}

/// Fetch task progress via HTTP.
pub async fn fetch_task_progress(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    task_id: &str,
) -> JsonHttpResponse {
    let response = with_session_auth(
        client.get(format!(
            "{base_url}/api/v1/sessions/{session_id}/tasks/{task_id}/progress"
        )),
        base_url,
        session_id,
        false,
    )
    .send()
    .await
    .expect("failed to fetch task progress");
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .expect("failed to decode task progress");
    JsonHttpResponse {
        status,
        body: body.get("progress").cloned().unwrap_or(body),
    }
}

/// Fetch task timeline via HTTP.
pub async fn fetch_task_timeline(
    _client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    task_id: &str,
) -> serde_json::Value {
    let state = test_server_state(base_url);
    let timelines = state.task_timeline.read().await;
    let timeline = timelines
        .get(task_id)
        .cloned()
        .expect("task timeline should exist");
    serde_json::json!({
        "task_id": task_id,
        "session_id": session_id,
        "milestones": timeline.milestones,
        "tool_calls": timeline.tool_calls,
    })
}

/// Create a session via HTTP.
pub async fn create_session_http(client: &reqwest::Client, base_url: &str) -> String {
    create_session_http_with_user(client, base_url, 1).await
}

/// Create a session via HTTP for a specific user.
pub async fn create_session_http_with_user(
    client: &reqwest::Client,
    base_url: &str,
    legacy_user_id: i64,
) -> String {
    let auth = ensure_test_auth_session(base_url, legacy_user_id).await;
    let response: serde_json::Value = with_auth(
        client.post(format!("{base_url}/api/v1/sessions")),
        &auth,
        true,
    )
    .json(&serde_json::json!({}))
    .send()
    .await
    .expect("failed to create session")
    .json()
    .await
    .expect("failed to decode session response");

    let session_id = response["session"]["session_id"]
        .as_str()
        .expect("session_id missing")
        .to_string();
    remember_session_auth(base_url, &session_id, auth);
    session_id
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
    let response: serde_json::Value = with_session_auth(
        client.post(format!("{base_url}/api/v1/sessions/{session_id}/tasks")),
        base_url,
        session_id,
        true,
    )
    .json(&serde_json::json!({ "input_markdown": body }))
    .send()
    .await
    .expect("failed to create task")
    .json()
    .await
    .expect("failed to decode task response");

    response["task"]["task_id"]
        .as_str()
        .expect("task_id missing")
        .to_string()
}

pub async fn create_task_http_expect_conflict(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    body: &str,
) -> serde_json::Value {
    let response = with_session_auth(
        client.post(format!("{base_url}/api/v1/sessions/{session_id}/tasks")),
        base_url,
        session_id,
        true,
    )
    .json(&serde_json::json!({ "input_markdown": body }))
    .send()
    .await
    .expect("failed to submit conflicting task");
    assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
    response
        .json()
        .await
        .expect("failed to decode conflict response")
}

pub async fn delete_session_http(client: &reqwest::Client, base_url: &str, session_id: &str) {
    let status = with_session_auth(
        client.delete(format!("{base_url}/api/v1/sessions/{session_id}")),
        base_url,
        session_id,
        true,
    )
    .send()
    .await
    .expect("failed to delete session")
    .status();
    assert!(
        status.is_success(),
        "delete session should succeed, got {status}"
    );
}

pub fn session_user_id(base_url: &str, session_id: &str) -> i64 {
    auth_for_session(base_url, session_id).user_id
}

pub fn with_session_auth(
    request: reqwest::RequestBuilder,
    base_url: &str,
    session_id: &str,
    include_csrf: bool,
) -> reqwest::RequestBuilder {
    with_auth(
        request,
        &auth_for_session(base_url, session_id),
        include_csrf,
    )
}

/// Spawn an HTTP test server and return the server handle and base URL.
pub async fn spawn_test_server(app_state: AppState) -> (tokio::task::JoinHandle<()>, String) {
    let router = oxide_agent_transport_web::build_router(app_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind test server");
    let addr = listener
        .local_addr()
        .expect("failed to get test server addr");
    let base_url = format!("http://{addr}");
    remember_test_server_state(&base_url, app_state.clone());

    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("test server failed");
    });

    (server, base_url)
}

fn with_auth(
    request: reqwest::RequestBuilder,
    auth: &TestAuthSession,
    include_csrf: bool,
) -> reqwest::RequestBuilder {
    let request = request.header(
        reqwest::header::COOKIE,
        format!("{TEST_AUTH_COOKIE_NAME}={}", auth.raw_token),
    );
    if include_csrf {
        request.header("x-csrf-token", auth.csrf_token.clone())
    } else {
        request
    }
}

async fn ensure_test_auth_session(base_url: &str, legacy_user_id: i64) -> TestAuthSession {
    let state = test_server_state(base_url);
    let login = format!("e2e-{legacy_user_id}");
    let now = chrono::Utc::now();
    let login_request = || LoginRequest {
        login: login.clone(),
        password: TEST_USER_PASSWORD.to_string(),
    };
    let register_request = || RegisterRequest {
        login: login.clone(),
        password: TEST_USER_PASSWORD.to_string(),
    };

    let login_result = login_user(state.web_store.as_ref(), login_request(), now).await;
    let (user, auth_session, raw_token) = match login_result {
        Ok(login) => login,
        Err(_) => {
            register_user(state.web_store.as_ref(), register_request(), true, now)
                .await
                .expect("register e2e web user");
            login_user(state.web_store.as_ref(), login_request(), now)
                .await
                .expect("login e2e web user")
        }
    };

    TestAuthSession {
        user_id: user.user_id,
        raw_token,
        csrf_token: auth_session.csrf_token,
    }
}

fn remember_test_server_state(base_url: &str, app_state: AppState) {
    test_server_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(base_url.to_string(), app_state);
}

fn test_server_state(base_url: &str) -> AppState {
    test_server_states()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(base_url)
        .cloned()
        .expect("test server state should be registered")
}

fn remember_session_auth(base_url: &str, session_id: &str, auth: TestAuthSession) {
    session_auths()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(session_auth_key(base_url, session_id), auth);
}

fn auth_for_session(base_url: &str, session_id: &str) -> TestAuthSession {
    session_auths()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&session_auth_key(base_url, session_id))
        .cloned()
        .expect("session auth should be registered")
}

fn session_auth_key(base_url: &str, session_id: &str) -> String {
    format!("{base_url}|{session_id}")
}

fn test_server_states() -> &'static Mutex<HashMap<String, AppState>> {
    static STATES: OnceLock<Mutex<HashMap<String, AppState>>> = OnceLock::new();
    STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_auths() -> &'static Mutex<HashMap<String, TestAuthSession>> {
    static AUTHS: OnceLock<Mutex<HashMap<String, TestAuthSession>>> = OnceLock::new();
    AUTHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn normalize_persisted_event_for_legacy_assertions(event: &serde_json::Value) -> serde_json::Value {
    let mut event = event.clone();
    let event_name = legacy_event_name(&event);
    if let Some(object) = event.as_object_mut() {
        object
            .entry("event_name")
            .or_insert_with(|| serde_json::Value::String(event_name));
    }
    event
}

fn legacy_event_name(event: &serde_json::Value) -> String {
    let kind = event["kind"].as_str().unwrap_or("unknown");
    let summary = event["summary"].as_str().unwrap_or_default();
    match kind {
        "tool_call" => format!("tool_call:{summary}"),
        "tool_result" => format!("tool_result:{summary}"),
        "runtime_compaction_started" => "compaction_started".to_string(),
        "runtime_compaction_completed" => "compaction_completed".to_string(),
        "runtime_compaction_failed" => "compaction_failed".to_string(),
        "runtime_compaction_skipped" => "compaction_skipped".to_string(),
        "repeated_compaction_warning" => "repeated_compaction_warning".to_string(),
        other => other.to_string(),
    }
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
