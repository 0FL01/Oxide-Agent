use super::*;
use crate::agent::profile::{AgentExecutionProfile, ToolAccessPolicy};
use crate::config::ModelInfo;
use std::collections::HashSet;

#[test]
fn v1_tool_runtime_model_detection_accepts_opencode_deepseek_route() {
    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "opencode-go/deepseek_v4_flash".to_string(),
            provider: "OpenCode Go".to_string(),
            ..ModelInfo::default()
        }
    ));
}

#[test]
fn v1_tool_runtime_model_detection_rejects_other_routes() {
    assert!(!AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "openrouter".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(!AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-chat".to_string(),
            provider: "opencode-go".to_string(),
            ..ModelInfo::default()
        }
    ));
}

#[test]
fn typed_runtime_registry_exposes_sandbox_tools() {
    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in [
        "execute_command",
        "list_files",
        "read_file",
        "recreate_sandbox",
        "send_file_to_user",
        "write_todos",
        "write_file",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime tool: {tool_name}"
        );
    }
    assert!(!tool_names.contains("compress"));
}

#[test]
fn typed_runtime_registry_exposes_manager_tools_when_manager_enabled() {
    let executor =
        build_executor().with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77);
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in [
        "topic_binding_set",
        "topic_binding_get",
        "agent_profile_upsert",
        "topic_agent_tools_get",
        "topic_agent_tools_enable",
        "topic_agent_tools_disable",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime manager tool: {tool_name}"
        );
    }
}

#[test]
fn typed_runtime_registry_exposes_manager_lifecycle_tools_when_lifecycle_is_attached() {
    let lifecycle = Arc::new(RecordingTopicLifecycle::new());
    let executor = build_executor()
        .with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77)
        .with_manager_topic_lifecycle(lifecycle);
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in ["forum_topic_create", "forum_topic_list"] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime lifecycle tool: {tool_name}"
        );
    }
}

#[test]
fn typed_runtime_registry_applies_execution_profile_tool_policy() {
    let mut executor =
        build_executor().with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77);
    executor.set_execution_profile(AgentExecutionProfile::new(
        None,
        None,
        ToolAccessPolicy::new(
            Some(HashSet::from(["execute_command".to_string()])),
            HashSet::default(),
        ),
    ));
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("execute_command"));
    assert!(!tool_names.contains("write_todos"));
    assert!(!tool_names.contains("topic_agent_tools_get"));
}

#[test]
fn current_tool_definitions_use_typed_runtime_specs_for_v1_route() {
    let settings = Arc::new(AgentSettings {
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let tool_names = executor
        .current_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("execute_command"));
    assert!(tool_names.contains("read_file"));
    assert!(tool_names.contains("write_todos"));
    assert!(tool_names.contains("write_file"));
    assert!(!tool_names.contains("compress"));
}

#[test]
fn current_tool_definitions_include_manager_tools_for_v1_route() {
    let settings = Arc::new(AgentSettings {
        agent_model_id: Some("deepseek-v4-flash".to_string()),
        agent_model_provider: Some("opencode-go".to_string()),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings)
        .with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77);

    let tool_names = executor
        .current_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("execute_command"));
    assert!(tool_names.contains("write_todos"));
    assert!(tool_names.contains("topic_agent_tools_get"));
    assert!(tool_names.contains("topic_agent_tools_enable"));
    assert!(tool_names.contains("agent_profile_upsert"));
}

#[tokio::test]
async fn manager_enabled_registry_executes_manager_tool() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_binding()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|user_id, topic_id| {
            Ok(Some(TopicBindingRecord {
                schema_version: 1,
                version: 3,
                user_id,
                topic_id,
                agent_id: "agent-a".to_string(),
                binding_kind: TopicBindingKind::Manual,
                chat_id: None,
                thread_id: None,
                expires_at: None,
                last_activity_at: Some(20),
                created_at: 10,
                updated_at: 20,
            }))
        });

    let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
        .await
        .expect("manager-enabled registry must route manager tool");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("manager tool response must be valid json");
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["binding"]["agent_id"], "agent-a");
}

#[tokio::test]
async fn manager_disabled_registry_rejects_manager_tool() {
    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let err = registry
        .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
        .await
        .expect_err("manager-disabled registry must reject manager tools");

    assert!(err.to_string().contains("Unknown tool"));
}

#[tokio::test]
async fn main_agent_registry_includes_explicit_media_and_tts_file_tools() {
    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    for tool in [
        "transcribe_audio_file",
        "describe_image_file",
        "describe_video_file",
        "text_to_speech_en_file",
        "text_to_speech_ru_file",
    ] {
        assert!(registry.can_handle(tool), "missing registry tool: {tool}");
    }

    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(tool_names.contains("transcribe_audio_file"));
    assert!(tool_names.contains("describe_video_file"));
    assert!(tool_names.contains("text_to_speech_en_file"));
    assert!(tool_names.contains("text_to_speech_ru_file"));
}

#[tokio::test]
async fn main_agent_registry_includes_stack_log_tools() {
    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(registry.can_handle("compress"));
    assert!(registry.can_handle("stack_logs_list_sources"));
    assert!(registry.can_handle("stack_logs_fetch"));

    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(tool_names.contains("compress"));
}

#[cfg(feature = "tool-browser-use")]
#[tokio::test]
async fn browser_use_enabled_registry_registers_browser_tools() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");

    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(registry.can_handle("browser_use_run_task"));
    assert!(registry.can_handle("browser_use_get_session"));
    assert!(registry.can_handle("browser_use_close_session"));
    assert!(registry.can_handle("browser_use_extract_content"));
    assert!(registry.can_handle("browser_use_screenshot"));

    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[cfg(feature = "tool-browser-use")]
#[tokio::test]
async fn browser_use_disabled_registry_skips_browser_tools() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("browser_use_run_task"));
    assert!(!registry.can_handle("browser_use_get_session"));
    assert!(!registry.can_handle("browser_use_close_session"));
    assert!(!registry.can_handle("browser_use_extract_content"));
    assert!(!registry.can_handle("browser_use_screenshot"));
}

#[cfg(feature = "tool-browser-use")]
#[test]
fn browser_use_profile_scope_uses_agents_md_topic() {
    let mut executor = build_executor();
    executor.set_agents_md_context(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
    );

    assert_eq!(
        executor.browser_use_profile_scope().as_deref(),
        Some("topic-a")
    );
}

#[cfg(feature = "tool-browser-use")]
#[test]
fn browser_use_profile_scope_prefers_reminder_context() {
    let mut executor = build_executor();
    executor.set_agents_md_context(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
    );
    executor.set_reminder_context(crate::agent::providers::ReminderContext {
        storage: Arc::new(MockStorageProvider::new()),
        user_id: 77,
        context_key: "topic-reminder".to_string(),
        flow_id: "flow-1".to_string(),
        chat_id: 77,
        thread_id: None,
        thread_kind: crate::storage::ReminderThreadKind::None,
        notifier: None,
    });

    assert_eq!(
        executor.browser_use_profile_scope().as_deref(),
        Some("topic-reminder")
    );
}

#[tokio::test]
async fn agents_md_context_enables_self_editing_tools() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .returning(|user_id, topic_id| {
            Ok(Some(crate::storage::TopicAgentsMdRecord {
                schema_version: 1,
                version: 4,
                user_id,
                topic_id,
                agents_md: "# Topic AGENTS\nCurrent instructions".to_string(),
                created_at: 10,
                updated_at: 20,
            }))
        });

    let mut executor = build_executor();
    executor.set_agents_md_context(Arc::new(mock), 77, "topic-a".to_string());
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let response = registry
        .execute("agents_md_get", "{}", None, None)
        .await
        .expect("agents_md_get must succeed when context is configured");

    let parsed: serde_json::Value =
        serde_json::from_str(&response).expect("tool response must be valid json");
    assert_eq!(parsed["found"], true);
    assert_eq!(parsed["topic_id"], "topic-a");
}

#[tokio::test]
async fn delegation_tool_inherits_agents_md_context_from_executor() {
    let mut mock = MockStorageProvider::new();
    mock.expect_get_topic_agents_md()
        .with(eq(77_i64), eq("topic-a".to_string()))
        .return_once(|_, _| {
            Err(crate::storage::StorageError::Config(
                "storage unavailable".to_string(),
            ))
        });

    let mut executor = build_executor();
    executor.set_agents_md_context(Arc::new(mock), 77, "topic-a".to_string());
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    let error = registry
        .execute(
            "spawn_sub_agents",
            &json!({
                "tasks": [{
                    "task": "Inspect the workspace.",
                    "tools": ["write_todos"]
                }]
            })
            .to_string(),
            None,
            None,
        )
        .await
        .expect_err("delegation should fail when inherited AGENTS.md cannot be loaded");

    assert!(error
        .to_string()
        .contains("Failed to load topic AGENTS.md for sub-agent bootstrap"));
}
