use super::*;
#[cfg(feature = "tool-sandbox-exec")]
use crate::agent::profile::{AgentExecutionProfile, ToolAccessPolicy};
use crate::config::{ModelInfo, ModuleRuntimeConfig};
#[cfg(feature = "tool-sandbox-exec")]
use crate::storage::MockStorageProvider;
// Feature-gated tests below may not consume both helpers in every profile (e.g. profile-lite).
#[allow(unused_imports)]
use crate::testing::{test_remove_env, test_set_env};
#[cfg(feature = "tool-sandbox-exec")]
use std::collections::HashSet;

#[cfg(feature = "tool-wiki-memory")]
#[derive(Default)]
struct RegistryWikiBackend;

#[cfg(feature = "tool-wiki-memory")]
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

#[cfg(feature = "tool-wiki-memory")]
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
        created_at: 0,
        updated_at: 0,
    }
}

#[test]
fn v1_tool_runtime_model_detection_accepts_opencode_go_and_zen_routes() {
    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-v4-flash".to_string(),
            provider: "opencode-go".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "opencode-go/mimo-v2.5".to_string(),
            provider: "OpenCode Go".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-v4-pro".to_string(),
            provider: "llm-provider/opencode-go".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "kimi-k2.6".to_string(),
            provider: "opencode-go".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "opencode-zen/mimo-v2.5-free".to_string(),
            provider: "opencode-zen".to_string(),
            ..ModelInfo::default()
        }
    ));

    assert!(AgentExecutor::v1_tool_runtime_enabled_for_model(
        &ModelInfo {
            id: "deepseek-v4-flash-free".to_string(),
            provider: "llm-provider/opencode-zen".to_string(),
            ..ModelInfo::default()
        }
    ));
}

#[test]
fn v1_tool_runtime_model_detection_rejects_non_opencode_routes() {
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
            provider: "openrouter".to_string(),
            ..ModelInfo::default()
        }
    ));
}

#[cfg(all(
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-recreate",
    feature = "tool-file-delivery"
))]
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
        "apply_file_edit",
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
    #[cfg(feature = "tool-compression")]
    assert!(tool_names.contains("compress"));
    #[cfg(not(feature = "tool-compression"))]
    assert!(!tool_names.contains("compress"));
}

#[cfg(feature = "tool-delegation")]
#[test]
fn typed_runtime_registry_exposes_delegation_tools() {
    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry.tool_names();

    for tool_name in ["spawn_sub_agents", "wait_sub_agents", "cancel_sub_agents"] {
        assert!(
            tool_names.iter().any(|name| name == tool_name),
            "missing typed runtime delegation tool: {tool_name}"
        );
        assert_eq!(
            tool_names.iter().filter(|name| *name == tool_name).count(),
            1,
            "expected one typed registration for {tool_name}"
        );
    }
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

    for tool_name in [
        "ssh_exec",
        "ssh_sudo_exec",
        "ssh_read_file",
        "ssh_apply_file_edit",
        "ssh_check_process",
        "ssh_send_file_to_user",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime SSH MCP tool: {tool_name}"
        );
    }
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

    for tool_name in [
        "wiki_memory_list",
        "wiki_memory_read",
        "wiki_memory_search",
        "wiki_memory_delete",
    ] {
        assert!(
            tool_names.contains(tool_name),
            "missing typed runtime wiki memory tool: {tool_name}"
        );
    }
}

#[cfg(feature = "tool-webfetch-md")]
#[test]
fn typed_runtime_registry_exposes_webfetch_tool() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Ensure crawl4ai is not configured, so webfetch_md wins the precedence.
    test_remove_env("OXIDE_CRAWL4AI_BASE_URL");
    test_remove_env("OXIDE_CRAWL4AI_ENABLED");
    test_remove_env("WEBFETCH_MD_ENABLED");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("web_markdown"));
}

#[cfg(feature = "tool-crawl4ai-markdown")]
#[test]
fn typed_runtime_registry_exposes_crawl4ai_markdown_tool() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("OXIDE_CRAWL4AI_BASE_URL", "http://crawl4ai:11235");
    test_remove_env("OXIDE_CRAWL4AI_ENABLED");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("crawl4ai_markdown"));

    test_remove_env("OXIDE_CRAWL4AI_BASE_URL");
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

#[cfg(all(feature = "tool-webfetch-md", feature = "tool-crawl4ai-markdown"))]
#[test]
fn typed_runtime_registry_drops_webfetch_when_crawl4ai_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("OXIDE_CRAWL4AI_BASE_URL", "http://crawl4ai:11235");
    test_remove_env("OXIDE_CRAWL4AI_ENABLED");
    test_remove_env("WEBFETCH_MD_ENABLED");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(
        tool_names.contains("crawl4ai_markdown"),
        "crawl4ai_markdown should be registered when OXIDE_CRAWL4AI_BASE_URL is set"
    );
    assert!(
        !tool_names.contains("web_markdown"),
        "web_markdown must be suppressed when crawl4ai is configured to avoid duplicate attention cost"
    );

    test_remove_env("OXIDE_CRAWL4AI_BASE_URL");
}

#[cfg(all(feature = "tool-webfetch-md", feature = "tool-crawl4ai-markdown"))]
#[test]
fn typed_runtime_registry_keeps_webfetch_when_crawl4ai_unconfigured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_remove_env("OXIDE_CRAWL4AI_BASE_URL");
    test_remove_env("OXIDE_CRAWL4AI_ENABLED");
    test_remove_env("WEBFETCH_MD_ENABLED");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(
        tool_names.contains("web_markdown"),
        "web_markdown should be the lightweight fallback when crawl4ai is not configured"
    );
    assert!(
        !tool_names.contains("crawl4ai_markdown"),
        "crawl4ai_markdown must not be registered without OXIDE_CRAWL4AI_BASE_URL"
    );
}

#[cfg(all(feature = "tool-webfetch-md", feature = "tool-crawl4ai-markdown"))]
#[test]
fn typed_runtime_registry_respects_webfetch_md_enabled_override() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Crawl4AI is configured...
    test_set_env("OXIDE_CRAWL4AI_BASE_URL", "http://crawl4ai:11235");
    // ...but operator explicitly disabled webfetch_md as a belt-and-braces override.
    test_set_env("WEBFETCH_MD_ENABLED", "false");

    let executor = build_executor();
    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("crawl4ai_markdown"));
    assert!(!tool_names.contains("web_markdown"));

    test_remove_env("OXIDE_CRAWL4AI_BASE_URL");
    test_remove_env("WEBFETCH_MD_ENABLED");
}

#[cfg(all(feature = "tool-tts-kokoro", feature = "tool-tts-silero"))]
#[test]
fn typed_runtime_registry_exposes_tts_tools() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("KOKORO_TTS_URL", "http://kokoro-tts:8880");
    test_set_env("SILERO_TTS_URL", "http://silero-tts:8000");

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

    test_remove_env("SILERO_TTS_URL");
    test_remove_env("KOKORO_TTS_URL");
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

#[cfg(feature = "tool-sandbox-exec")]
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

#[cfg(feature = "tool-crawl4ai-markdown")]
#[test]
fn typed_runtime_registry_skips_disabled_crawl4ai_markdown_module() {
    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/crawl4ai-markdown".to_string(),
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

    assert!(!tool_names.contains("crawl4ai_markdown"));
}

#[cfg(feature = "tool-compression")]
#[test]
fn typed_runtime_registry_skips_disabled_compression_module() {
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

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("compress"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-tts-kokoro")]
#[test]
fn typed_runtime_registry_skips_disabled_kokoro_tts_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("KOKORO_TTS_URL", "http://kokoro-tts:8880");

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

    test_remove_env("KOKORO_TTS_URL");
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
    test_set_env("SILERO_TTS_URL", "http://silero-tts:8000");

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

    test_remove_env("SILERO_TTS_URL");
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

#[cfg(feature = "tool-delegation")]
#[test]
fn typed_runtime_registry_skips_disabled_delegation_module() {
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

    let registry =
        executor.build_tool_runtime_registry(Arc::new(Mutex::new(TodoList::new())), None);
    let tool_names = registry
        .tool_names()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    assert!(!tool_names.contains("spawn_sub_agents"));
    assert!(!tool_names.contains("wait_sub_agents"));
    assert!(!tool_names.contains("cancel_sub_agents"));
    assert!(tool_names.contains("write_todos"));
}

#[cfg(feature = "tool-tavily")]
#[test]
fn typed_runtime_registry_skips_disabled_tavily_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("TAVILY_API_KEY", "dummy-key");
    test_set_env("TAVILY_ENABLED", "true");

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

    test_remove_env("TAVILY_ENABLED");
    test_remove_env("TAVILY_API_KEY");
}

#[cfg(feature = "tool-duckduckgo")]
#[test]
fn typed_runtime_registry_skips_disabled_duckduckgo_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("DUCKDUCKGO_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/duckduckgo".to_string(),
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

    assert!(!tool_names.contains("duckduckgo_search"));
    assert!(!tool_names.contains("duckduckgo_news"));
    #[cfg(feature = "tool-todos")]
    assert!(tool_names.contains("write_todos"));

    test_remove_env("DUCKDUCKGO_ENABLED");
}

#[cfg(feature = "tool-searxng")]
#[test]
fn typed_runtime_registry_skips_disabled_searxng_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("SEARXNG_URL", "http://searxng:8080");
    test_set_env("SEARXNG_ENABLED", "true");

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
    #[cfg(feature = "tool-todos")]
    assert!(tool_names.contains("write_todos"));

    test_remove_env("SEARXNG_ENABLED");
    test_remove_env("SEARXNG_URL");
}

#[cfg(feature = "tool-brave-search")]
#[test]
fn typed_runtime_registry_skips_disabled_brave_search_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("BRAVE_SEARCH_API_KEY", "dummy-key");
    test_set_env("BRAVE_SEARCH_ENABLED", "true");

    let settings = Arc::new(AgentSettings {
        modules: std::collections::BTreeMap::from([(
            "tool/brave-search".to_string(),
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

    assert!(!tool_names.contains("brave_search"));
    #[cfg(feature = "tool-todos")]
    assert!(tool_names.contains("write_todos"));

    test_remove_env("BRAVE_SEARCH_ENABLED");
    test_remove_env("BRAVE_SEARCH_API_KEY");
}

#[cfg(feature = "tool-brave-search")]
#[test]
fn current_tool_definitions_include_brave_search_when_key_is_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("BRAVE_SEARCH_API_KEY", "dummy-key");
    test_set_env("BRAVE_SEARCH_ENABLED", "true");

    let tool_names = build_executor()
        .current_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect::<std::collections::BTreeSet<_>>();

    assert!(tool_names.contains("brave_search"));

    test_remove_env("BRAVE_SEARCH_ENABLED");
    test_remove_env("BRAVE_SEARCH_API_KEY");
}

#[cfg(all(feature = "tool-tavily", feature = "tool-duckduckgo"))]
#[test]
fn typed_runtime_registry_registers_search_modules_once() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("TAVILY_API_KEY", "dummy-key");
    test_set_env("TAVILY_ENABLED", "true");
    test_set_env("DUCKDUCKGO_ENABLED", "true");

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
            .filter(|name| *name == "duckduckgo_search")
            .count(),
        1
    );
    assert_eq!(
        tool_names
            .iter()
            .filter(|name| *name == "duckduckgo_news")
            .count(),
        1
    );

    test_remove_env("DUCKDUCKGO_ENABLED");
    test_remove_env("TAVILY_ENABLED");
    test_remove_env("TAVILY_API_KEY");
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

#[cfg(all(
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-recreate"
))]
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

#[cfg(all(
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-recreate",
    feature = "tool-file-delivery"
))]
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

    for file_tool in ["write_file", "read_file", "apply_file_edit", "list_files"] {
        assert!(!tool_names.contains(file_tool));
    }
    assert!(tool_names.contains("send_file_to_user"));
    assert!(tool_names.contains("upload_file"));
    assert!(tool_names.contains("execute_command"));
    assert!(tool_names.contains("recreate_sandbox"));
}

#[cfg(feature = "tool-sandbox-fileops")]
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
    assert!(tool_names.contains("apply_file_edit"));
    assert!(tool_names.contains("list_files"));
}

#[cfg(feature = "integration-mcp-jira")]
#[test]
fn typed_runtime_registry_exposes_jira_mcp_tools_when_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("JIRA_URL", "https://jira.example.test");
    test_set_env("JIRA_EMAIL", "bot@example.test");
    test_set_env("JIRA_API_TOKEN", "dummy-token");
    test_set_env("JIRA_MCP_BINARY_PATH", "jira-mcp");

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

    test_remove_env("JIRA_MCP_BINARY_PATH");
    test_remove_env("JIRA_API_TOKEN");
    test_remove_env("JIRA_EMAIL");
    test_remove_env("JIRA_URL");
}

#[cfg(feature = "integration-mcp-mattermost")]
#[test]
fn typed_runtime_registry_exposes_mattermost_mcp_tools_when_configured() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("MATTERMOST_URL", "https://mattermost.example.test");
    test_set_env("MATTERMOST_TOKEN", "dummy-token");
    test_set_env("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

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

    test_remove_env("MATTERMOST_MCP_BINARY_PATH");
    test_remove_env("MATTERMOST_TOKEN");
    test_remove_env("MATTERMOST_URL");
}

#[cfg(feature = "integration-mcp-jira")]
#[test]
fn typed_runtime_registry_skips_disabled_jira_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("JIRA_URL", "https://jira.example.test");
    test_set_env("JIRA_EMAIL", "bot@example.test");
    test_set_env("JIRA_API_TOKEN", "dummy-token");
    test_set_env("JIRA_MCP_BINARY_PATH", "jira-mcp");

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

    test_remove_env("JIRA_MCP_BINARY_PATH");
    test_remove_env("JIRA_API_TOKEN");
    test_remove_env("JIRA_EMAIL");
    test_remove_env("JIRA_URL");
}

#[cfg(feature = "integration-mcp-mattermost")]
#[test]
fn typed_runtime_registry_skips_disabled_mattermost_mcp_module() {
    let _guard = crate::config::test_env_mutex()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    test_set_env("MATTERMOST_URL", "https://mattermost.example.test");
    test_set_env("MATTERMOST_TOKEN", "dummy-token");
    test_set_env("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

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

    test_remove_env("MATTERMOST_MCP_BINARY_PATH");
    test_remove_env("MATTERMOST_TOKEN");
    test_remove_env("MATTERMOST_URL");
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
    test_set_env("JIRA_URL", "https://jira.example.test");
    test_set_env("JIRA_EMAIL", "bot@example.test");
    test_set_env("JIRA_API_TOKEN", "dummy-token");
    test_set_env("JIRA_MCP_BINARY_PATH", "jira-mcp");
    test_set_env("MATTERMOST_URL", "https://mattermost.example.test");
    test_set_env("MATTERMOST_TOKEN", "dummy-token");
    test_set_env("MATTERMOST_MCP_BINARY_PATH", "mattermost-mcp");

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

    test_remove_env("MATTERMOST_MCP_BINARY_PATH");
    test_remove_env("MATTERMOST_TOKEN");
    test_remove_env("MATTERMOST_URL");
    test_remove_env("JIRA_MCP_BINARY_PATH");
    test_remove_env("JIRA_API_TOKEN");
    test_remove_env("JIRA_EMAIL");
    test_remove_env("JIRA_URL");
}

#[cfg(all(
    feature = "tool-sandbox-fileops",
    feature = "tool-sandbox-exec",
    feature = "tool-file-delivery"
))]
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
    assert!(tool_names.contains("apply_file_edit"));
    #[cfg(feature = "tool-compression")]
    assert!(tool_names.contains("compress"));
    #[cfg(not(feature = "tool-compression"))]
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
