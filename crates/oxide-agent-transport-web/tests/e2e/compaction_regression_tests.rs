//! Deterministic regressions for compaction observability.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::agent::SessionId;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::{ChatResponse, LlmClient, TokenUsage, ToolCall, ToolCallFunction};
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::AppState;

use super::helpers::{
    create_session_http_with_user, create_task_http_with_body, fetch_task_events,
    fetch_task_progress, wait_for_task_status, wait_for_zai_calls,
};
use super::providers::{ControlledNarratorProvider, SequencedZaiProvider};

fn derive_session_id(session_id: &str, user_id: i64) -> SessionId {
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    user_id.hash(&mut h);
    SessionId::from(h.finish() as i64)
}

async fn seed_history(
    session_manager: &oxide_agent_transport_web::session::WebSessionManager,
    session_id: &str,
    user_id: i64,
    messages: Vec<AgentMessage>,
) {
    let sid = derive_session_id(session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");

    let mut executor = executor_arc.write().await;
    for message in messages {
        executor.session_mut().memory.add_message(message);
    }
}

fn setup_web_test_with_budget(
    zai_provider: Arc<SequencedZaiProvider>,
    narrator_provider: Arc<ControlledNarratorProvider>,
    model_max_output_tokens: u32,
    context_window_tokens: u32,
) -> AppState {
    let agent_settings = Arc::new(AgentSettings {
        agent_model_id: Some("main-model".to_string()),
        agent_model_provider: Some("zai".to_string()),
        agent_model_max_output_tokens: Some(model_max_output_tokens),
        agent_model_context_window_tokens: Some(context_window_tokens),
        sub_agent_model_id: Some("glm-4.7".to_string()),
        sub_agent_model_provider: Some("zai".to_string()),
        narrator_model_id: Some("narrator-model".to_string()),
        narrator_model_provider: Some("narrator".to_string()),
        agent_timeout_secs: Some(5),
        sub_agent_timeout_secs: Some(5),
        ..AgentSettings::default()
    });

    let llm = {
        let mut llm = LlmClient::new(&agent_settings);
        llm.register_provider("zai".to_string(), zai_provider);
        llm.register_provider("narrator".to_string(), narrator_provider);
        Arc::new(llm)
    };

    let registry = SessionRegistry::new();
    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    AppState::new(Arc::new(session_manager))
}

fn setup_web_test_with_compaction_budget(
    zai_provider: Arc<SequencedZaiProvider>,
    narrator_provider: Arc<ControlledNarratorProvider>,
) -> AppState {
    setup_web_test_with_budget(zai_provider, narrator_provider, 32_000, 200_000)
}

fn setup_web_test_with_pressure_budget(
    zai_provider: Arc<SequencedZaiProvider>,
    narrator_provider: Arc<ControlledNarratorProvider>,
) -> AppState {
    setup_web_test_with_budget(zai_provider, narrator_provider, 1_024, 4_096)
}

fn two_todo_tool_calls_response() -> ChatResponse {
    ChatResponse {
        content: None,
        tool_calls: vec![
            ToolCall {
                id: "call-todo-1".to_string(),
                function: ToolCallFunction {
                    name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "todos": [
                            {
                                "description": "First task",
                                "status": "in_progress"
                            }
                        ]
                    })
                    .to_string(),
                },
                is_recovered: false,
            },
            ToolCall {
                id: "call-todo-2".to_string(),
                function: ToolCallFunction {
                    name: "write_todos".to_string(),
                    arguments: serde_json::json!({
                        "todos": [
                            {
                                "description": "Second task",
                                "status": "completed"
                            }
                        ]
                    })
                    .to_string(),
                },
                is_recovered: false,
            },
        ],
        finish_reason: "tool_calls".to_string(),
        reasoning_content: None,
        usage: Some(TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
        }),
    }
}

fn request_contains(request: &super::providers::RecordedToolRequest, needle: &str) -> bool {
    request.system_prompt.contains(needle)
        || request
            .messages
            .iter()
            .any(|message| message.content.contains(needle))
}

#[tokio::test]
async fn e2e_compaction_post_run_prunes_old_artifact_on_healthy_budget() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260321;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Investigate old artifacts"),
            AgentMessage::tool("old-call", "web_markdown", &"A".repeat(1_500)),
            AgentMessage::tool("recent-1", "web_markdown", "short-1"),
            AgentMessage::tool("recent-2", "web_markdown", "short-2"),
            AgentMessage::tool("recent-3", "web_markdown", "short-3"),
            AgentMessage::tool("recent-4", "web_markdown", "short-4"),
        ],
    )
    .await;

    let task_id = create_task_http_with_body(&client, &base_url, &session_id, "Return done").await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(3),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 1, Duration::from_secs(2)).await;

    let progress_resp = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    assert!(progress_resp.status().is_success());
    let progress: serde_json::Value = progress_resp
        .json()
        .await
        .expect("failed to decode task progress");
    assert_eq!(progress["latest_token_snapshot"]["budget_state"], "Healthy");

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect();
    assert!(event_names.contains(&"pruning_applied"));
    assert!(event_names.contains(&"compaction_completed"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();
    let old_tool = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("old-call"))
        .expect("old tool message should exist");
    assert!(old_tool.is_pruned());
    assert!(old_tool.content.contains("[pruned tool result]"));

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_initial_anchor_survives_next_llm_call() {
    let anchor = "ANCHOR_CTX_9f3a9a4bc7f14d60b2a6e8c14529f0aa";
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        two_todo_tool_calls_response(),
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260324;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
    let old_payload = format!("{}{}", "x".repeat(1_200), anchor);

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Investigate whether initial context survives cleanup"),
            AgentMessage::tool("old-anchor", "web_markdown", &old_payload),
            AgentMessage::tool("recent-1", "web_markdown", "short-1"),
            AgentMessage::tool("recent-2", "web_markdown", "short-2"),
            AgentMessage::tool("recent-3", "web_markdown", "short-3"),
            AgentMessage::tool("recent-4", "web_markdown", "short-4"),
        ],
    )
    .await;

    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Update todos once, then finish and mention completion.",
    )
    .await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(3),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 2, Duration::from_secs(2)).await;

    let progress_resp = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    assert!(progress_resp.status().is_success());
    let progress: serde_json::Value = progress_resp
        .json()
        .await
        .expect("failed to decode task progress");
    assert_eq!(progress["latest_token_snapshot"]["budget_state"], "Healthy");
    assert!(progress["last_compaction_status"]
        .as_str()
        .is_some_and(|value| value.contains("Cleanup:")));

    let request_log = zai_provider.request_log().await;
    assert!(request_log.len() >= 2, "expected at least two LLM calls");
    assert!(
        request_contains(&request_log[0], anchor),
        "anchor missing from first LLM request despite healthy budget"
    );
    assert!(
        request_contains(request_log.last().expect("last request"), anchor),
        "anchor missing from later LLM request despite healthy budget"
    );

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();
    let old_tool = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("old-anchor"))
        .expect("old tool message should exist after post-run cleanup");
    assert!(old_tool.is_pruned());
    assert!(!old_tool.content.contains(anchor));

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_post_run_prunes_old_data_without_summary() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260322;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    let old_payload = format!("{}CRITICAL_DECISION_TOKEN", "x".repeat(1_200));
    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Investigate context retention"),
            AgentMessage::tool("old-call", "web_markdown", &old_payload),
            AgentMessage::tool("recent-1", "web_markdown", "short-1"),
            AgentMessage::tool("recent-2", "web_markdown", "short-2"),
            AgentMessage::tool("recent-3", "web_markdown", "short-3"),
            AgentMessage::tool("recent-4", "web_markdown", "short-4"),
        ],
    )
    .await;

    let task_id =
        create_task_http_with_body(&client, &base_url, &session_id, "Acknowledge completion").await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(3),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 1, Duration::from_secs(2)).await;

    let progress_resp = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    assert!(progress_resp.status().is_success());
    let progress: serde_json::Value = progress_resp
        .json()
        .await
        .expect("failed to decode task progress");
    assert_eq!(progress["latest_token_snapshot"]["budget_state"], "Healthy");
    assert!(progress["last_compaction_status"]
        .as_str()
        .is_some_and(|value| value.contains("Cleanup:")));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let old_tool = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("old-call"))
        .expect("old tool message should exist after post-run cleanup");
    assert!(old_tool.is_pruned());
    assert!(!old_tool.content.contains("CRITICAL_DECISION_TOKEN"));
    assert!(!messages
        .iter()
        .any(|message| message.summary_payload().is_some()));

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_pressure_budget_applies_post_run_cleanup_without_summary_boundary() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        two_todo_tool_calls_response(),
        two_todo_tool_calls_response(),
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_pressure_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260323;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Trigger repeated cleanup"),
            AgentMessage::tool(
                "old-large",
                "web_markdown",
                &format!("{}OLD_TOOL_MARKER", "A".repeat(1_500)),
            ),
            AgentMessage::tool("short-1", "web_markdown", "short-1"),
            AgentMessage::tool("short-2", "web_markdown", "short-2"),
            AgentMessage::tool("short-3", "web_markdown", "short-3"),
            AgentMessage::tool("recent-large", "web_markdown", &"B".repeat(1_500)),
        ],
    )
    .await;

    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Update todo list twice and finish",
    )
    .await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(3),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 3, Duration::from_secs(2)).await;

    let progress_resp = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    assert!(progress_resp.status().is_success());
    let progress: serde_json::Value = progress_resp
        .json()
        .await
        .expect("failed to decode task progress");
    assert!(matches!(
        progress["latest_token_snapshot"]["budget_state"].as_str(),
        Some("ShouldPrune") | Some("ShouldCompact") | Some("OverLimit")
    ));

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect();
    assert!(
        progress["last_compaction_status"].as_str().is_some(),
        "unexpected progress payload: {}; event_names={:?}",
        serde_json::to_string_pretty(&progress).expect("serialize progress"),
        event_names
    );
    assert!(event_names.contains(&"compaction_completed"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();
    assert!(!messages.iter().any(
        |message| message.tool_call_id.as_deref() == Some("old-large")
            && message.content.contains("OLD_TOOL_MARKER")
    ));

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_pressure_budget_prunes_only_before_summary_boundary() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state = setup_web_test_with_pressure_budget(zai_provider.clone(), narrator_provider);
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260325;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::tool("old-before-summary", "web_markdown", &"A".repeat(1_500)),
            AgentMessage::summary("[Previous context compressed]\n- old web findings preserved"),
            AgentMessage::tool("after-summary-1", "web_markdown", &"B".repeat(1_500)),
            AgentMessage::tool("after-summary-2", "web_markdown", "short-1"),
            AgentMessage::tool("after-summary-3", "web_markdown", "short-2"),
            AgentMessage::tool("after-summary-4", "web_markdown", "short-3"),
            AgentMessage::tool("after-summary-5", "web_markdown", "short-4"),
        ],
    )
    .await;

    let task_id = create_task_http_with_body(&client, &base_url, &session_id, "Return done").await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(3),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 1, Duration::from_secs(2)).await;

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect();
    assert!(event_names.contains(&"pruning_applied"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let before_summary = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("old-before-summary"))
        .expect("old before-summary tool should exist");
    let after_summary = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("after-summary-1"))
        .expect("after-summary tool should exist");

    assert!(before_summary.is_pruned());
    assert!(!after_summary.is_pruned());

    server.abort();
}
