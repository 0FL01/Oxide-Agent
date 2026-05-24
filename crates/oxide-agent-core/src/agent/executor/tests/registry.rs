use super::*;
use crate::agent::profile::{AgentExecutionProfile, ToolAccessPolicy};
use crate::config::{ModelInfo, ModuleRuntimeConfig};
use std::collections::HashSet;

#[derive(Default)]
struct RegistryWikiBackend;

#[async_trait::async_trait]
impl crate::agent::wiki_memory::WikiObjectBackend for RegistryWikiBackend {
    async fn get_text(&self, _key: &str) -> Result<Option<String>, crate::storage::StorageError> {
        Ok(None)
    }

    async fn put_text(
        &self,
        _key: &str,
        _content: &str,
    ) -> Result<(), crate::storage::StorageError> {
        Ok(())
    }

    async fn delete_text(&self, _key: &str) -> Result<(), crate::storage::StorageError> {
        Ok(())
    }
}

fn registry_wiki_store() -> crate::agent::wiki_memory::WikiStore {
    crate::agent::wiki_memory::WikiStore::new(Arc::new(RegistryWikiBackend), "prod")
}

#[cfg(feature = "integration-ssh-mcp")]
fn registry_topic_infra_config() -> crate::storage::TopicInfraConfigRecord {
    crate::storage::TopicInfraConfigRecord {
        schema_version: 1,
        version: 1,
        user_id: 77,
        topic_id: "topic-a".to_string(),
        target_name: "prod-app".to_string(),
        host: "prod.example.test".to_string(),
        port: 22,
        remote_user: "deploy".to_string(),
        auth_mode: crate::storage::TopicInfraAuthMode::PrivateKey,
        secret_ref: Some("storage:ssh/key".to_string()),
        sudo_secret_ref: None,
        environment: Some("prod".to_string()),
        tags: Vec::new(),
        allowed_tool_modes: vec![
            crate::storage::TopicInfraToolMode::Exec,
            crate::storage::TopicInfraToolMode::SudoExec,
            crate::storage::TopicInfraToolMode::ReadFile,
            crate::storage::TopicInfraToolMode::ApplyFileEdit,
            crate::storage::TopicInfraToolMode::CheckProcess,
            crate::storage::TopicInfraToolMode::Transfer,
        ],
        approval_required_modes: Vec::new(),
        created_at: 0,
        updated_at: 0,
    }
}

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
        "upload_file",
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

#[cfg(feature = "manager-control-plane")]
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

#[cfg(feature = "manager-control-plane")]
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

#[cfg(feature = "integration-ssh-mcp")]
#[test]
fn typed_runtime_registry_exposes_ssh_mcp_tools_when_topic_infra_configured() {
    let mut executor = build_executor();
    executor.set_topic_infra(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
        Some(registry_topic_infra_config()),
    );
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in ["ssh_exec", "ssh_sudo_exec", "ssh_check_process"] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime SSH MCP tool: {tool_name}"
        );
    }
    assert!(!tool_names.contains("ssh_read_file"));
    assert!(!tool_names.contains("ssh_apply_file_edit"));
    assert!(!tool_names.contains("ssh_send_file_to_user"));
}

#[cfg(feature = "tool-wiki-memory")]
#[test]
fn typed_runtime_registry_exposes_wiki_memory_tools_when_store_configured() {
    let executor = build_executor().with_wiki_memory_store(registry_wiki_store());
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in ["wiki_memory_list", "wiki_memory_read", "wiki_memory_delete"] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime wiki memory tool: {tool_name}"
        );
    }
}

#[cfg(feature = "tool-webfetch-md")]
#[test]
fn typed_runtime_registry_exposes_webfetch_tool() {
    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("web_markdown"));
}

#[cfg(feature = "tool-ytdlp")]
#[test]
fn typed_runtime_registry_exposes_ytdlp_tools() {
    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in [
        "ytdlp_get_video_metadata",
        "ytdlp_download_transcript",
        "ytdlp_search_videos",
        "ytdlp_download_video",
        "ytdlp_download_audio",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime yt-dlp tool: {tool_name}"
        );
    }
}

#[cfg(all(feature = "tool-tts-kokoro", feature = "tool-tts-silero"))]
#[test]
fn typed_runtime_registry_exposes_tts_tools() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("KOKORO_TTS_URL", "http://kokoro-tts:8880");
    std::env::set_var("SILERO_TTS_URL", "http://silero-tts:8000");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in [
        "text_to_speech_en",
        "text_to_speech_en_file",
        "text_to_speech_ru",
        "text_to_speech_ru_file",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime TTS tool: {tool_name}"
        );
    }

    std::env::remove_var("SILERO_TTS_URL");
    std::env::remove_var("KOKORO_TTS_URL");
}

#[cfg(all(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
#[test]
fn typed_runtime_registry_exposes_media_tools() {
    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry.tool_names();

    for tool_name in [
        "transcribe_audio_file",
        "describe_image_file",
        "describe_video_file",
    ] {
        assert!(
            tool_names.iter().any(|name| name == tool_name),
            "missing typed runtime media tool: {tool_name}"
        );
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
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
fn typed_runtime_registry_skips_disabled_todos_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/todos".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-webfetch-md")]
#[test]
fn typed_runtime_registry_skips_disabled_webfetch_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/webfetch-md".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("web_markdown"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-tts-kokoro")]
#[test]
fn typed_runtime_registry_skips_disabled_kokoro_tts_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("KOKORO_TTS_URL", "http://kokoro-tts:8880");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tts-kokoro".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("text_to_speech_en"));
    assert!(!tool_names.contains("text_to_speech_en_file"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("KOKORO_TTS_URL");
}

#[cfg(feature = "tool-media-audio")]
#[test]
fn typed_runtime_registry_skips_disabled_media_audio_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-audio".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("transcribe_audio_file"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-media-image")]
#[test]
fn typed_runtime_registry_skips_disabled_media_image_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-image".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("describe_image_file"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-media-video")]
#[test]
fn typed_runtime_registry_skips_disabled_media_video_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-video".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("describe_video_file"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-tts-silero")]
#[test]
fn typed_runtime_registry_skips_disabled_silero_tts_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("SILERO_TTS_URL", "http://silero-tts:8000");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tts-silero".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("text_to_speech_ru"));
    assert!(!tool_names.contains("text_to_speech_ru_file"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("SILERO_TTS_URL");
}

#[cfg(feature = "tool-ytdlp")]
#[test]
fn typed_runtime_registry_skips_disabled_ytdlp_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/ytdlp".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("ytdlp_get_video_metadata"));
    assert!(!tool_names.contains("ytdlp_download_video"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-tavily")]
#[test]
fn typed_runtime_registry_skips_disabled_tavily_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("TAVILY_API_KEY", "dummy-key");
    std::env::set_var("TAVILY_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tavily".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("web_search"));
    assert!(!tool_names.contains("web_extract"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("TAVILY_ENABLED");
    std::env::remove_var("TAVILY_API_KEY");
}

#[cfg(feature = "tool-searxng")]
#[test]
fn typed_runtime_registry_skips_disabled_searxng_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("SEARXNG_URL", "http://searxng:8080");
    std::env::set_var("SEARXNG_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/searxng".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("searxng_search"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("SEARXNG_ENABLED");
    std::env::remove_var("SEARXNG_URL");
}

#[cfg(all(feature = "tool-tavily", feature = "tool-searxng"))]
#[test]
fn typed_runtime_registry_registers_search_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("TAVILY_API_KEY", "dummy-key");
    std::env::set_var("TAVILY_ENABLED", "true");
    std::env::set_var("SEARXNG_URL", "http://searxng:8080");
    std::env::set_var("SEARXNG_ENABLED", "true");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry.tool_names();

    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "web_search")
            .count(),
        1
    );
    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "web_extract")
            .count(),
        1
    );
    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "searxng_search")
            .count(),
        1
    );

    std::env::remove_var("SEARXNG_ENABLED");
    std::env::remove_var("SEARXNG_URL");
    std::env::remove_var("TAVILY_ENABLED");
    std::env::remove_var("TAVILY_API_KEY");
}

#[cfg(feature = "manager-control-plane")]
#[test]
fn typed_runtime_registry_skips_disabled_manager_control_plane_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "manager/control-plane".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings)
        .with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("topic_binding_get"));
    assert!(!tool_names.contains("topic_agent_tools_get"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "integration-ssh-mcp")]
#[test]
fn typed_runtime_registry_skips_disabled_ssh_mcp_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/ssh-mcp".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(llm, session, settings);
    executor.set_topic_infra(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
        Some(registry_topic_infra_config()),
    );

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("ssh_exec"));
    assert!(!tool_names.contains("ssh_read_file"));
    assert!(tool_names.contains("write_todos"));
}

#[test]
fn typed_runtime_registry_skips_disabled_sandbox_exec_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/sandbox-exec".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("execute_command"));
    assert!(tool_names.contains("read_file"));
    assert!(tool_names.contains("recreate_sandbox"));
}

#[test]
fn typed_runtime_registry_skips_disabled_sandbox_fileops_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/sandbox-fileops".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for file_tool in ["write_file", "read_file", "list_files"] {
        assert!(!tool_names.contains(file_tool));
    }
    assert!(tool_names.contains("send_file_to_user"));
    assert!(tool_names.contains("upload_file"));
    assert!(tool_names.contains("execute_command"));
    assert!(tool_names.contains("recreate_sandbox"));
}

#[test]
fn typed_runtime_registry_skips_disabled_file_delivery_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/file-delivery".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("send_file_to_user"));
    assert!(!tool_names.contains("upload_file"));
    assert!(tool_names.contains("write_file"));
    assert!(tool_names.contains("read_file"));
    assert!(tool_names.contains("list_files"));
}

#[test]
fn legacy_registry_skips_disabled_todos_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/todos".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("write_todos"));
    assert!(registry.can_handle("execute_command"));
}

#[cfg(feature = "manager-control-plane")]
#[test]
fn legacy_registry_skips_disabled_manager_control_plane_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "manager/control-plane".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings)
        .with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("topic_binding_get"));
    assert!(!registry.can_handle("topic_agent_tools_get"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "integration-ssh-mcp")]
#[test]
fn legacy_registry_skips_disabled_ssh_mcp_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/ssh-mcp".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(llm, session, settings);
    executor.set_topic_infra(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
        Some(registry_topic_infra_config()),
    );

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("ssh_exec"));
    assert!(!registry.can_handle("ssh_read_file"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_compression_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/compression".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("compress"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "tool-delegation")]
#[test]
fn legacy_registry_skips_disabled_delegation_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/delegation".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("spawn_sub_agents"));
    assert!(!registry.can_handle("wait_sub_agents"));
    assert!(!registry.can_handle("cancel_sub_agents"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_file_delivery_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/file-delivery".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("upload_file"));
    assert!(!registry.can_handle("send_file_to_user"));
    assert!(registry.can_handle("write_file"));
    assert!(registry.can_handle("read_file"));
    assert!(registry.can_handle("list_files"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_ytdlp_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/ytdlp".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("ytdlp_get_video_metadata"));
    assert!(!registry.can_handle("ytdlp_download_video"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_stack_logs_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/stack-logs".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("stack_logs_list_sources"));
    assert!(!registry.can_handle("stack_logs_fetch"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_webfetch_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/webfetch-md".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("web_markdown"));
    assert!(registry.can_handle("write_todos"));
}

#[test]
fn legacy_registry_skips_disabled_tavily_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("TAVILY_API_KEY", "dummy-key");
    std::env::set_var("TAVILY_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tavily".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("web_search"));
    assert!(!registry.can_handle("web_extract"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("TAVILY_ENABLED");
    std::env::remove_var("TAVILY_API_KEY");
}

#[test]
fn legacy_registry_skips_disabled_searxng_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("SEARXNG_URL", "http://searxng:8080");
    std::env::set_var("SEARXNG_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/searxng".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("searxng_search"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("SEARXNG_ENABLED");
    std::env::remove_var("SEARXNG_URL");
}

#[cfg(all(feature = "tool-tavily", feature = "tool-searxng"))]
#[test]
fn legacy_registry_registers_search_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("TAVILY_API_KEY", "dummy-key");
    std::env::set_var("TAVILY_ENABLED", "true");
    std::env::set_var("SEARXNG_URL", "http://searxng:8080");
    std::env::set_var("SEARXNG_ENABLED", "true");

    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "web_search")
            .count(),
        1
    );
    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "web_extract")
            .count(),
        1
    );
    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "searxng_search")
            .count(),
        1
    );

    std::env::remove_var("SEARXNG_ENABLED");
    std::env::remove_var("SEARXNG_URL");
    std::env::remove_var("TAVILY_ENABLED");
    std::env::remove_var("TAVILY_API_KEY");
}

#[cfg(feature = "integration-mcp-jira")]
#[test]
fn typed_runtime_registry_exposes_jira_mcp_tools_when_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("JIRA_URL", "https://jira.example.test");
    std::env::set_var("JIRA_EMAIL", "bot@example.test");
    std::env::set_var("JIRA_API_TOKEN", "dummy-token");
    std::env::set_var("JIRA_MCP_BINARY_PATH", "jira-mcp");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in ["jira_read", "jira_write", "jira_schema"] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime Jira MCP tool: {tool_name}"
        );
    }

    std::env::remove_var("JIRA_MCP_BINARY_PATH");
    std::env::remove_var("JIRA_API_TOKEN");
    std::env::remove_var("JIRA_EMAIL");
    std::env::remove_var("JIRA_URL");
}

#[cfg(feature = "integration-mcp-mattermost")]
#[test]
fn typed_runtime_registry_exposes_mattermost_mcp_tools_when_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("MATTERMOST_URL", "https://mattermost.example.test");
    std::env::set_var("MATTERMOST_TOKEN", "dummy-token");
    std::env::set_var("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    for tool_name in [
        "mattermost_list_teams",
        "mattermost_post_message",
        "mattermost_upload_file",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime Mattermost MCP tool: {tool_name}"
        );
    }

    std::env::remove_var("MATTERMOST_MCP_BINARY_PATH");
    std::env::remove_var("MATTERMOST_TOKEN");
    std::env::remove_var("MATTERMOST_URL");
}

#[cfg(feature = "integration-mcp-jira")]
#[test]
fn typed_runtime_registry_skips_disabled_jira_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("JIRA_URL", "https://jira.example.test");
    std::env::set_var("JIRA_EMAIL", "bot@example.test");
    std::env::set_var("JIRA_API_TOKEN", "dummy-token");
    std::env::set_var("JIRA_MCP_BINARY_PATH", "jira-mcp");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/mcp-jira".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("jira_read"));
    assert!(!tool_names.contains("jira_write"));
    assert!(!tool_names.contains("jira_schema"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("JIRA_MCP_BINARY_PATH");
    std::env::remove_var("JIRA_API_TOKEN");
    std::env::remove_var("JIRA_EMAIL");
    std::env::remove_var("JIRA_URL");
}

#[cfg(feature = "integration-mcp-mattermost")]
#[test]
fn typed_runtime_registry_skips_disabled_mattermost_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("MATTERMOST_URL", "https://mattermost.example.test");
    std::env::set_var("MATTERMOST_TOKEN", "dummy-token");
    std::env::set_var("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/mcp-mattermost".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("mattermost_list_teams"));
    assert!(!tool_names.contains("mattermost_post_message"));
    assert!(tool_names.contains("write_todos"));

    std::env::remove_var("MATTERMOST_MCP_BINARY_PATH");
    std::env::remove_var("MATTERMOST_TOKEN");
    std::env::remove_var("MATTERMOST_URL");
}

#[cfg(all(
    feature = "integration-mcp-jira",
    feature = "integration-mcp-mattermost"
))]
#[test]
fn typed_runtime_registry_registers_mcp_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("JIRA_URL", "https://jira.example.test");
    std::env::set_var("JIRA_EMAIL", "bot@example.test");
    std::env::set_var("JIRA_API_TOKEN", "dummy-token");
    std::env::set_var("JIRA_MCP_BINARY_PATH", "jira-mcp");
    std::env::set_var("MATTERMOST_URL", "https://mattermost.example.test");
    std::env::set_var("MATTERMOST_TOKEN", "dummy-token");
    std::env::set_var("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry.tool_names();

    for tool_name in [
        "jira_read",
        "jira_write",
        "jira_schema",
        "mattermost_list_teams",
        "mattermost_post_message",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one typed registration for {tool_name}"
        );
    }

    std::env::remove_var("MATTERMOST_MCP_BINARY_PATH");
    std::env::remove_var("MATTERMOST_TOKEN");
    std::env::remove_var("MATTERMOST_URL");
    std::env::remove_var("JIRA_MCP_BINARY_PATH");
    std::env::remove_var("JIRA_API_TOKEN");
    std::env::remove_var("JIRA_EMAIL");
    std::env::remove_var("JIRA_URL");
}

#[cfg(feature = "integration-mcp-jira")]
#[test]
fn legacy_registry_skips_disabled_jira_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("JIRA_URL", "https://jira.example.test");
    std::env::set_var("JIRA_EMAIL", "bot@example.test");
    std::env::set_var("JIRA_API_TOKEN", "dummy-token");
    std::env::set_var("JIRA_MCP_BINARY_PATH", "jira-mcp");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/mcp-jira".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("jira_read"));
    assert!(!registry.can_handle("jira_write"));
    assert!(!registry.can_handle("jira_schema"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("JIRA_MCP_BINARY_PATH");
    std::env::remove_var("JIRA_API_TOKEN");
    std::env::remove_var("JIRA_EMAIL");
    std::env::remove_var("JIRA_URL");
}

#[cfg(feature = "integration-mcp-mattermost")]
#[test]
fn legacy_registry_skips_disabled_mattermost_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("MATTERMOST_URL", "https://mattermost.example.test");
    std::env::set_var("MATTERMOST_TOKEN", "dummy-token");
    std::env::set_var("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "integration/mcp-mattermost".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("mattermost_list_teams"));
    assert!(!registry.can_handle("mattermost_post_message"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("MATTERMOST_MCP_BINARY_PATH");
    std::env::remove_var("MATTERMOST_TOKEN");
    std::env::remove_var("MATTERMOST_URL");
}

#[cfg(all(
    feature = "integration-mcp-jira",
    feature = "integration-mcp-mattermost"
))]
#[test]
fn legacy_registry_registers_mcp_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("JIRA_URL", "https://jira.example.test");
    std::env::set_var("JIRA_EMAIL", "bot@example.test");
    std::env::set_var("JIRA_API_TOKEN", "dummy-token");
    std::env::set_var("JIRA_MCP_BINARY_PATH", "jira-mcp");
    std::env::set_var("MATTERMOST_URL", "https://mattermost.example.test");
    std::env::set_var("MATTERMOST_TOKEN", "dummy-token");
    std::env::set_var("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "jira_read",
        "jira_write",
        "jira_schema",
        "mattermost_list_teams",
        "mattermost_post_message",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }

    std::env::remove_var("MATTERMOST_MCP_BINARY_PATH");
    std::env::remove_var("MATTERMOST_TOKEN");
    std::env::remove_var("MATTERMOST_URL");
    std::env::remove_var("JIRA_MCP_BINARY_PATH");
    std::env::remove_var("JIRA_API_TOKEN");
    std::env::remove_var("JIRA_EMAIL");
    std::env::remove_var("JIRA_URL");
}

#[cfg(feature = "tool-tts-kokoro")]
#[test]
fn legacy_registry_skips_disabled_kokoro_tts_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("KOKORO_TTS_URL", "http://kokoro-tts:8880");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tts-kokoro".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("text_to_speech_en"));
    assert!(!registry.can_handle("text_to_speech_en_file"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("KOKORO_TTS_URL");
}

#[cfg(feature = "tool-tts-silero")]
#[test]
fn legacy_registry_skips_disabled_silero_tts_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("SILERO_TTS_URL", "http://silero-tts:8000");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/tts-silero".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("text_to_speech_ru"));
    assert!(!registry.can_handle("text_to_speech_ru_file"));
    assert!(registry.can_handle("write_todos"));

    std::env::remove_var("SILERO_TTS_URL");
}

#[cfg(all(feature = "tool-tts-kokoro", feature = "tool-tts-silero"))]
#[test]
fn legacy_registry_registers_tts_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("KOKORO_TTS_URL", "http://kokoro-tts:8880");
    std::env::set_var("SILERO_TTS_URL", "http://silero-tts:8000");

    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "text_to_speech_en",
        "text_to_speech_en_file",
        "text_to_speech_ru",
        "text_to_speech_ru_file",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }

    std::env::remove_var("SILERO_TTS_URL");
    std::env::remove_var("KOKORO_TTS_URL");
}

#[cfg(feature = "tool-media-audio")]
#[test]
fn legacy_registry_skips_disabled_media_audio_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-audio".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("transcribe_audio_file"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "tool-media-image")]
#[test]
fn legacy_registry_skips_disabled_media_image_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-image".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("describe_image_file"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "tool-media-video")]
#[test]
fn legacy_registry_skips_disabled_media_video_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/media-video".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("describe_video_file"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(all(
    feature = "tool-media-audio",
    feature = "tool-media-image",
    feature = "tool-media-video"
))]
#[test]
fn legacy_registry_registers_media_modules_once() {
    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "transcribe_audio_file",
        "describe_image_file",
        "describe_video_file",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
}

#[test]
fn legacy_registry_skips_disabled_sandbox_exec_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/sandbox-exec".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("execute_command"));
    assert!(registry.can_handle("read_file"));
    assert!(registry.can_handle("recreate_sandbox"));
}

#[test]
fn legacy_registry_skips_disabled_sandbox_fileops_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/sandbox-fileops".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    for file_tool in ["write_file", "read_file", "list_files"] {
        assert!(!registry.can_handle(file_tool));
    }
    assert!(registry.can_handle("send_file_to_user"));
    assert!(registry.can_handle("upload_file"));
    assert!(registry.can_handle("execute_command"));
    assert!(registry.can_handle("recreate_sandbox"));
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

#[cfg(feature = "manager-control-plane")]
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

#[cfg(feature = "manager-control-plane")]
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

#[cfg(feature = "manager-control-plane")]
#[test]
fn legacy_registry_registers_manager_control_plane_module_once() {
    let lifecycle = Arc::new(RecordingTopicLifecycle::new());
    let executor = build_executor()
        .with_manager_control_plane(Arc::new(MockStorageProvider::new()), 77)
        .with_manager_topic_lifecycle(lifecycle);
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "topic_binding_get",
        "topic_agent_tools_get",
        "agent_profile_upsert",
        "forum_topic_create",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
}

#[cfg(feature = "integration-ssh-mcp")]
#[test]
fn legacy_registry_registers_ssh_mcp_module_once() {
    let mut executor = build_executor();
    executor.set_topic_infra(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
        Some(registry_topic_infra_config()),
    );
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "ssh_exec",
        "ssh_sudo_exec",
        "ssh_read_file",
        "ssh_apply_file_edit",
        "ssh_check_process",
        "ssh_send_file_to_user",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
}

#[cfg(feature = "tool-delegation")]
#[test]
fn legacy_registry_registers_delegation_module_once() {
    let executor = build_executor();
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in ["spawn_sub_agents", "wait_sub_agents", "cancel_sub_agents"] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
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

    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    for tool_name in [
        "browser_use_run_task",
        "browser_use_get_session",
        "browser_use_close_session",
        "browser_use_extract_content",
        "browser_use_screenshot",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }

    std::env::remove_var("BROWSER_USE_ENABLED");
    std::env::remove_var("BROWSER_USE_URL");
}

#[cfg(feature = "tool-browser-use")]
#[tokio::test]
async fn legacy_registry_skips_disabled_browser_use_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
    std::env::set_var("BROWSER_USE_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/browser-use".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm, session, settings);

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("browser_use_run_task"));
    assert!(!registry.can_handle("browser_use_get_session"));
    assert!(!registry.can_handle("browser_use_close_session"));
    assert!(!registry.can_handle("browser_use_extract_content"));
    assert!(!registry.can_handle("browser_use_screenshot"));

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

#[cfg(feature = "tool-agents-md")]
#[test]
fn legacy_registry_skips_disabled_agents_md_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/agents-md".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(llm, session, settings);
    executor.set_agents_md_context(
        Arc::new(MockStorageProvider::new()),
        77,
        "topic-a".to_string(),
    );

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("agents_md_get"));
    assert!(!registry.can_handle("agents_md_update"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "tool-reminder")]
#[test]
fn legacy_registry_skips_disabled_reminder_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/reminder".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let mut executor = AgentExecutor::new(llm, session, settings);
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

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("reminder_schedule"));
    assert!(!registry.can_handle("reminder_list"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(all(feature = "tool-agents-md", feature = "tool-reminder"))]
#[test]
fn legacy_registry_registers_topic_modules_once() {
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

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in [
        "agents_md_get",
        "agents_md_update",
        "reminder_schedule",
        "reminder_list",
    ] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
}

#[cfg(feature = "tool-wiki-memory")]
#[test]
fn legacy_registry_skips_disabled_wiki_memory_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/wiki-memory".to_string(),
            ModuleRuntimeConfig::disabled(),
        )]),
        ..AgentSettings::default()
    });
    let llm = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor =
        AgentExecutor::new(llm, session, settings).with_wiki_memory_store(registry_wiki_store());

    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

    assert!(!registry.can_handle("wiki_memory_list"));
    assert!(!registry.can_handle("wiki_memory_read"));
    assert!(!registry.can_handle("wiki_memory_delete"));
    assert!(registry.can_handle("write_todos"));
}

#[cfg(feature = "tool-wiki-memory")]
#[test]
fn legacy_registry_registers_wiki_memory_module_once() {
    let executor = build_executor().with_wiki_memory_store(registry_wiki_store());
    let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .all_tools()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();

    for tool_name in ["wiki_memory_list", "wiki_memory_read", "wiki_memory_delete"] {
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one registration for {tool_name}"
        );
    }
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

#[cfg(feature = "tool-agents-md")]
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

#[cfg(all(feature = "tool-agents-md", feature = "tool-delegation"))]
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
