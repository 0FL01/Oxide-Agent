//! Deterministic regressions for compaction observability.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Once};
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
    fetch_task_progress, tool_call_response, wait_for_task_status, wait_for_zai_calls,
};
use super::providers::{ControlledNarratorProvider, SequencedZaiProvider};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn derive_session_id(session_id: &str, user_id: i64) -> SessionId {
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    user_id.hash(&mut h);
    SessionId::from(h.finish() as i64)
}

fn init_test_tracing() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = tracing_subscriber::registry()
            .with(EnvFilter::from_default_env())
            .with(tracing_subscriber::fmt::layer().with_test_writer())
            .try_init();
    });
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall::new(
        id.to_string(),
        ToolCallFunction {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
        false,
    )
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
                tool_call_correlation: None,
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
                tool_call_correlation: None,
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

fn token_rich_payload(label: &str, words: usize) -> String {
    (0..words)
        .map(|index| format!("{label}_{index} signal_{index}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn append_seeded_tool_result(
    messages: &mut Vec<AgentMessage>,
    call_id: &str,
    tool_name: &str,
    arguments: serde_json::Value,
    output: impl Into<String>,
) {
    // Seed a valid assistant/tool pair so compaction sees the same correlation shape as runtime.
    let output = output.into();
    messages.push(AgentMessage::assistant_with_tools(
        format!("Seed {tool_name}"),
        vec![tool_call(call_id, tool_name, arguments)],
    ));
    messages.push(AgentMessage::tool(call_id, tool_name, &output));
}

#[tokio::test]
async fn e2e_compaction_post_run_deduplicates_superseded_read_file_results() {
    init_test_tracing();

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260404;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Inspect dependency versions with repeated file reads"),
            AgentMessage::assistant_with_tools(
                "Read Cargo.toml first",
                vec![tool_call(
                    "call-read-1",
                    "read_file",
                    serde_json::json!({"path":"Cargo.toml"}),
                )],
            ),
            AgentMessage::tool(
                "call-read-1",
                "read_file",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"",
            ),
            AgentMessage::assistant_with_tools(
                "Read README next",
                vec![tool_call(
                    "call-read-2",
                    "read_file",
                    serde_json::json!({"path":"README.md"}),
                )],
            ),
            AgentMessage::tool("call-read-2", "read_file", "# Demo repo\nSome notes here."),
            AgentMessage::assistant_with_tools(
                "Read Cargo.toml again",
                vec![tool_call(
                    "call-read-3",
                    "read_file",
                    serde_json::json!({"path":"Cargo.toml"}),
                )],
            ),
            AgentMessage::tool(
                "call-read-3",
                "read_file",
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"",
            ),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 1",
                vec![tool_call(
                    "recent-call-1",
                    "search",
                    serde_json::json!({"query":"recent filler 1"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-1",
                "search",
                &token_rich_payload("recent-1", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 2",
                vec![tool_call(
                    "recent-call-2",
                    "search",
                    serde_json::json!({"query":"recent filler 2"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-2",
                "search",
                &token_rich_payload("recent-2", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 3",
                vec![tool_call(
                    "recent-call-3",
                    "search",
                    serde_json::json!({"query":"recent filler 3"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-3",
                "search",
                &token_rich_payload("recent-3", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 4",
                vec![tool_call(
                    "recent-call-4",
                    "search",
                    serde_json::json!({"query":"recent filler 4"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-4",
                "search",
                &token_rich_payload("recent-4", 1_200),
            ),
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
    assert!(progress["last_compaction_status"].as_str().is_some());

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<String> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .map(str::to_string)
        .collect();
    eprintln!("[dedup-e2e] event_names={event_names:?}");
    assert!(event_names
        .iter()
        .any(|event| event == "compaction_completed"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let read_one = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-1"))
        .expect("first read_file result should exist");
    let read_two = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-2"))
        .expect("README read_file result should exist");
    let read_three = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-3"))
        .expect("second Cargo.toml read_file result should exist");

    eprintln!(
        "[dedup-e2e] call-read-1 content={:?}",
        read_one.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-read-2 content={:?}",
        read_two.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-read-3 content={:?}",
        read_three.content.chars().take(180).collect::<String>()
    );

    assert!(read_one.content.starts_with("[deduplicated tool result]"));
    assert!(read_one.content.contains("tool: read_file"));
    assert!(!read_one.is_externalized());
    assert!(!read_one.is_pruned());
    assert_eq!(read_two.content, "# Demo repo\nSome notes here.");
    assert_eq!(
        read_three.content,
        "[package]\nname = \"demo\"\nversion = \"0.1.0\""
    );

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_post_run_deduplicates_only_matching_read_file_paths() {
    init_test_tracing();

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260405;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Inspect file-specific dedup behavior"),
            AgentMessage::assistant_with_tools(
                "Read file A",
                vec![tool_call(
                    "call-read-a-1",
                    "read_file",
                    serde_json::json!({"path":"file_a.txt"}),
                )],
            ),
            AgentMessage::tool("call-read-a-1", "read_file", "alpha contents\nline 2"),
            AgentMessage::assistant_with_tools(
                "Read file B",
                vec![tool_call(
                    "call-read-b",
                    "read_file",
                    serde_json::json!({"path":"file_b.txt"}),
                )],
            ),
            AgentMessage::tool("call-read-b", "read_file", "beta contents\nline 2"),
            AgentMessage::assistant_with_tools(
                "Read file A again",
                vec![tool_call(
                    "call-read-a-2",
                    "read_file",
                    serde_json::json!({"path":"file_a.txt"}),
                )],
            ),
            AgentMessage::tool("call-read-a-2", "read_file", "alpha contents\nline 2"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 1",
                vec![tool_call(
                    "recent-call-1",
                    "search",
                    serde_json::json!({"query":"recent filler 1"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-1",
                "search",
                &token_rich_payload("recent-1", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 2",
                vec![tool_call(
                    "recent-call-2",
                    "search",
                    serde_json::json!({"query":"recent filler 2"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-2",
                "search",
                &token_rich_payload("recent-2", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 3",
                vec![tool_call(
                    "recent-call-3",
                    "search",
                    serde_json::json!({"query":"recent filler 3"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-3",
                "search",
                &token_rich_payload("recent-3", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 4",
                vec![tool_call(
                    "recent-call-4",
                    "search",
                    serde_json::json!({"query":"recent filler 4"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-4",
                "search",
                &token_rich_payload("recent-4", 1_200),
            ),
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
    assert!(progress["last_compaction_status"].as_str().is_some());

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<String> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .map(str::to_string)
        .collect();
    eprintln!("[dedup-e2e] event_names={event_names:?}");
    assert!(event_names
        .iter()
        .any(|event| event == "compaction_completed"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let read_a_1 = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-a-1"))
        .expect("first file_a read should exist");
    let read_b = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-b"))
        .expect("file_b read should exist");
    let read_a_2 = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-a-2"))
        .expect("second file_a read should exist");

    eprintln!(
        "[dedup-e2e] call-read-a-1 content={:?}",
        read_a_1.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-read-b content={:?}",
        read_b.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-read-a-2 content={:?}",
        read_a_2.content.chars().take(180).collect::<String>()
    );

    assert!(read_a_1.content.starts_with("[deduplicated tool result]"));
    assert!(read_a_1.content.contains("tool: read_file"));
    assert_eq!(read_b.content, "beta contents\nline 2");
    assert_eq!(read_a_2.content, "alpha contents\nline 2");

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_post_run_blocks_dedup_when_write_file_intervenes() {
    init_test_tracing();

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260406;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user_task("Inspect mutation blocking dedup"),
            AgentMessage::assistant_with_tools(
                "Read config first",
                vec![tool_call(
                    "call-read-1",
                    "read_file",
                    serde_json::json!({"path":"config.txt"}),
                )],
            ),
            AgentMessage::tool("call-read-1", "read_file", "setting_a=1\nsetting_b=2"),
            AgentMessage::assistant_with_tools(
                "Write config in between",
                vec![tool_call(
                    "call-write-1",
                    "write_file",
                    serde_json::json!({"path":"config.txt","content":"new content"}),
                )],
            ),
            AgentMessage::tool("call-write-1", "write_file", "ok"),
            AgentMessage::assistant_with_tools(
                "Read config again",
                vec![tool_call(
                    "call-read-2",
                    "read_file",
                    serde_json::json!({"path":"config.txt"}),
                )],
            ),
            AgentMessage::tool("call-read-2", "read_file", "setting_a=1\nsetting_b=2"),
            AgentMessage::summary("[Previous context compressed]\n- earlier work preserved"),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 1",
                vec![tool_call(
                    "recent-call-1",
                    "search",
                    serde_json::json!({"query":"recent filler 1"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-1",
                "search",
                &token_rich_payload("recent-1", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 2",
                vec![tool_call(
                    "recent-call-2",
                    "search",
                    serde_json::json!({"query":"recent filler 2"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-2",
                "search",
                &token_rich_payload("recent-2", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 3",
                vec![tool_call(
                    "recent-call-3",
                    "search",
                    serde_json::json!({"query":"recent filler 3"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-3",
                "search",
                &token_rich_payload("recent-3", 1_200),
            ),
            AgentMessage::assistant_with_tools(
                "Keep recent filler 4",
                vec![tool_call(
                    "recent-call-4",
                    "search",
                    serde_json::json!({"query":"recent filler 4"}),
                )],
            ),
            AgentMessage::tool(
                "recent-call-4",
                "search",
                &token_rich_payload("recent-4", 1_200),
            ),
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
    assert!(progress["last_compaction_status"].as_str().is_some());

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<String> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .map(str::to_string)
        .collect();
    eprintln!("[dedup-e2e] event_names={event_names:?}");
    assert!(event_names
        .iter()
        .any(|event| event == "compaction_completed"));
    assert!(!event_names.iter().any(|event| event.contains("dedup")));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let read_one = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-1"))
        .expect("first read_file result should exist");
    let write_one = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-write-1"))
        .expect("write_file result should exist");
    let read_two = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-read-2"))
        .expect("second read_file result should exist");

    eprintln!(
        "[dedup-e2e] call-read-1 content={:?}",
        read_one.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-write-1 content={:?}",
        write_one.content.chars().take(180).collect::<String>()
    );
    eprintln!(
        "[dedup-e2e] call-read-2 content={:?}",
        read_two.content.chars().take(180).collect::<String>()
    );

    assert_eq!(read_one.content, "setting_a=1\nsetting_b=2");
    assert_eq!(write_one.content, "ok");
    assert_eq!(read_two.content, "setting_a=1\nsetting_b=2");
    assert!(!read_one.content.starts_with("[deduplicated tool result]"));
    assert!(!read_two.content.starts_with("[deduplicated tool result]"));

    server.abort();
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

    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = vec![AgentMessage::user_task("Investigate old artifacts")];
        append_seeded_tool_result(
            &mut messages,
            "old-call",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/old-artifact"}),
            format!("{} OLD_ARTIFACT_MARKER", token_rich_payload("old", 2_500)),
        );
        for index in 1..=9 {
            append_seeded_tool_result(
                &mut messages,
                &format!("recent-{index}"),
                "web_markdown",
                serde_json::json!({"url": format!("https://example.test/recent-{index}")}),
                token_rich_payload(&format!("recent-{index}"), 2_500),
            );
        }
        messages
    })
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
        .find(|message| message.tool_call_id.as_deref() == Some("old-call"));
    if let Some(old_tool) = old_tool {
        assert!(old_tool.is_externalized() || old_tool.is_pruned());
        assert!(!old_tool.content.contains("OLD_ARTIFACT_MARKER"));
    }

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_post_run_preserves_delegate_results_while_cleaning_regular_tools() {
    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state =
        setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider.clone());
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260326;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = vec![AgentMessage::user_task(
            "Investigate delegated network findings",
        )];
        append_seeded_tool_result(
            &mut messages,
            "delegate-old",
            "delegate_to_sub_agent",
            serde_json::json!({"task":"delegate-old","tools":["write_todos"]}),
            format!(
                "delegated summary DELEGATE_MARKER {}",
                token_rich_payload("delegate", 2_500)
            ),
        );
        append_seeded_tool_result(
            &mut messages,
            "web-old",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/old"}),
            format!("{} WEB_MARKER", token_rich_payload("web-old", 2_500)),
        );
        for index in 1..=9 {
            append_seeded_tool_result(
                &mut messages,
                &format!("recent-{index}"),
                "ssh_exec",
                serde_json::json!({"command": format!("echo recent-{index}")}),
                token_rich_payload(&format!("recent-{index}"), 2_500),
            );
        }
        messages
    })
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

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();

    let delegate_tool = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("delegate-old"));
    let web_tool = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("web-old"));

    if let Some(delegate_tool) = delegate_tool {
        assert!(delegate_tool.content.contains("DELEGATE_MARKER"));
        assert!(!delegate_tool.is_externalized());
        assert!(!delegate_tool.is_pruned());
    }
    if let Some(web_tool) = web_tool {
        assert!(web_tool.is_externalized() || web_tool.is_pruned());
        assert!(!web_tool.content.contains("WEB_MARKER"));
    }

    server.abort();
}

#[tokio::test]
async fn e2e_compaction_initial_anchor_survives_many_small_followups() {
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

    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = vec![AgentMessage::user_task(
            "Investigate whether initial context survives cleanup",
        )];
        append_seeded_tool_result(
            &mut messages,
            "old-anchor",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/anchor"}),
            old_payload,
        );
        for index in 1..=8 {
            append_seeded_tool_result(
                &mut messages,
                &format!("recent-{index}"),
                "web_markdown",
                serde_json::json!({"url": format!("https://example.test/recent-{index}")}),
                format!("short-{index}"),
            );
        }
        messages
    })
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
    assert!(!old_tool.is_pruned());
    assert!(old_tool.content.contains(anchor));

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

    let old_payload = format!(
        "{} CRITICAL_DECISION_TOKEN",
        token_rich_payload("old-decision", 2_500)
    );
    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = vec![AgentMessage::user_task("Investigate context retention")];
        append_seeded_tool_result(
            &mut messages,
            "old-call",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/old-call"}),
            old_payload,
        );
        for index in 1..=9 {
            append_seeded_tool_result(
                &mut messages,
                &format!("recent-{index}"),
                "web_markdown",
                serde_json::json!({"url": format!("https://example.test/recent-{index}")}),
                token_rich_payload(&format!("recent-{index}"), 2_500),
            );
        }
        messages
    })
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
    assert!(old_tool.is_externalized() || old_tool.is_pruned());
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

    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = vec![AgentMessage::user_task("Trigger repeated cleanup")];
        append_seeded_tool_result(
            &mut messages,
            "old-large",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/old-large"}),
            format!("{} OLD_TOOL_MARKER", token_rich_payload("old-large", 1_200)),
        );
        for index in 1..=3 {
            append_seeded_tool_result(
                &mut messages,
                &format!("short-{index}"),
                "web_markdown",
                serde_json::json!({"url": format!("https://example.test/short-{index}")}),
                token_rich_payload(&format!("short-{index}"), 300),
            );
        }
        append_seeded_tool_result(
            &mut messages,
            "recent-large",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/recent-large"}),
            token_rich_payload("recent-large", 1_200),
        );
        messages
    })
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
    let old_large = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("old-large"));
    if let Some(old_large) = old_large {
        assert!(old_large.is_externalized() || old_large.is_pruned());
        assert!(!old_large.content.contains("OLD_TOOL_MARKER"));
    }

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

    seed_history(session_manager.as_ref(), &session_id, user_id, {
        let mut messages = Vec::new();
        append_seeded_tool_result(
            &mut messages,
            "old-before-summary",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/before-summary"}),
            format!(
                "{} BEFORE_SUMMARY_MARKER",
                token_rich_payload("old-before-summary", 1_200)
            ),
        );
        messages.push(AgentMessage::summary(
            "[Previous context compressed]\n- old web findings preserved",
        ));
        append_seeded_tool_result(
            &mut messages,
            "after-summary-1",
            "web_markdown",
            serde_json::json!({"url":"https://example.test/after-summary-1"}),
            token_rich_payload("after-summary-1", 1_200),
        );
        for index in 2..=5 {
            append_seeded_tool_result(
                &mut messages,
                &format!("after-summary-{index}"),
                "web_markdown",
                serde_json::json!({"url": format!("https://example.test/after-summary-{index}")}),
                token_rich_payload(&format!("after-summary-{index}"), 300),
            );
        }
        messages
    })
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
        .find(|message| message.tool_call_id.as_deref() == Some("old-before-summary"));
    let after_summary = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("after-summary-1"));

    if let Some(before_summary) = before_summary {
        assert!(before_summary.is_externalized() || before_summary.is_pruned());
        assert!(!before_summary.content.contains("BEFORE_SUMMARY_MARKER"));
    }
    if let Some(after_summary) = after_summary {
        assert!(!after_summary.is_pruned());
    }

    server.abort();
}

#[tokio::test]
async fn e2e_compress_tool_triggers_manual_compaction() {
    init_test_tracing();

    let zai_provider = Arc::new(SequencedZaiProvider::new(vec![
        tool_call_response("compress", serde_json::json!({})),
        super::helpers::unstructured_text_response("done"),
    ]));
    let narrator_provider = Arc::new(ControlledNarratorProvider::new(None));
    let app_state = setup_web_test_with_compaction_budget(zai_provider.clone(), narrator_provider);
    let session_manager = app_state.session_manager();
    let (server, base_url) = super::helpers::spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let user_id = 20260404;

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;

    seed_history(
        session_manager.as_ref(),
        &session_id,
        user_id,
        vec![
            AgentMessage::user("Older request with enough context to compact."),
            AgentMessage::assistant("Older response that should move into the summary."),
            AgentMessage::user("Recent request 1."),
            AgentMessage::assistant("Recent response 1."),
            AgentMessage::user("Recent request 2."),
            AgentMessage::assistant("Recent response 2."),
        ],
    )
    .await;

    let task_id = create_task_http_with_body(
        &client,
        &base_url,
        &session_id,
        "Compress the hot context and finish.",
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
    assert!(progress["last_compaction_status"].as_str().is_some());

    let events = fetch_task_events(&client, &base_url, &session_id, &task_id).await;
    let event_names: Vec<String> = events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .map(str::to_string)
        .collect();
    assert!(event_names
        .iter()
        .any(|event| event == "compaction_started"));
    assert!(event_names
        .iter()
        .any(|event| event == "compaction_completed"));

    let sid = derive_session_id(&session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");
    let executor = executor_arc.read().await;
    let messages = executor.session().memory.get_messages();
    let tool_result = messages
        .iter()
        .find(|message| message.tool_call_id.as_deref() == Some("call-compress"))
        .expect("compress tool result should exist");
    let tool_result_json: serde_json::Value = serde_json::from_str(&tool_result.content)
        .expect("compress tool result should be valid json");

    assert_eq!(tool_result_json["ok"], true);
    assert_eq!(tool_result_json["applied"], true);
    assert_eq!(tool_result_json["summary_updated"], true);

    server.abort();
}
