//! Delegated sub-agent E2E tests.
//!
//! Regression tests for the sub-agent relay cleanup deadlock fix.
//! See: <https://github.com/your-org/oxide-agent/pull/XXX>

use std::sync::Arc;
use std::time::Duration;

use super::helpers::{
    create_session_http, create_task_http, fetch_task_events, fetch_task_progress,
    fetch_task_timeline, spawn_test_server, wait_for_task_status, wait_for_zai_calls,
};
use super::providers::SequencedZaiProvider;
use super::setup::{
    delegated_sub_agent_empty_content_responses, setup_web_test_with_custom_providers,
};

/// Test: after the sub-agent relay cleanup fix, a delegated sub-agent completes
/// without deadlock, even when the sub-agent returns empty content.
#[tokio::test]
#[cfg_attr(not(feature = "socket_e2e"), ignore = "requires local TCP listener")]
async fn e2e_delegated_sub_agent_empty_content_completes_after_relay_cleanup() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(
        delegated_sub_agent_empty_content_responses(),
    ));
    let app_state = setup_web_test_with_custom_providers(zai_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    let session_id = create_session_http(&client, &base_url).await;
    let task_id = create_task_http(&client, &base_url, &session_id).await;

    wait_for_task_status(
        session_manager.as_ref(),
        &task_id,
        oxide_agent_transport_web::session::TaskStatus::Completed,
        Duration::from_secs(2),
    )
    .await;
    wait_for_zai_calls(&zai_provider, 4, Duration::from_secs(2)).await;

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let progress_response = fetch_task_progress(&client, &base_url, &session_id, &task_id).await;
    assert!(progress_response.status().is_success());
    let progress: serde_json::Value = progress_response
        .json()
        .await
        .expect("failed to decode task progress");
    let timeline = fetch_task_timeline(&client, &base_url, &session_id, &task_id).await;

    let event_names: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .collect();
    assert!(event_names.contains(&"tool_call:delegate_to_sub_agent"));
    assert!(event_names.contains(&"tool_call:write_todos"));
    assert!(event_names.contains(&"finished"));
    assert!(progress.is_object());
    assert!(timeline["milestones"]["final_response_ms"].is_number());
    assert_eq!(
        zai_provider.model_log().await,
        vec!["main-model", "glm-4.7", "glm-4.7", "main-model"]
    );

    server.abort();
}
