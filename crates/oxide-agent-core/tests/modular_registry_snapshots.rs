#![cfg(any(
    feature = "profile-embedded-opencode-local",
    feature = "profile-web-embedded-opencode-local",
    feature = "profile-lite",
    feature = "profile-search-only",
    feature = "profile-no-sandbox",
    feature = "profile-media-enabled",
    feature = "profile-host-bwrap",
    feature = "profile-full",
))]

use oxide_agent_core::agent::{AgentExecutor, AgentSession};
use oxide_agent_core::capabilities::{
    compiled_capability_manifest, CapabilityId, CapabilityKind, ModuleId, ModuleManifestEntry,
};
use oxide_agent_core::config::{AgentSettings, ModuleRuntimeConfig};
use oxide_agent_core::llm::LlmClient;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;
use tempfile::NamedTempFile;

#[derive(Serialize)]
struct ModularRegistrySnapshot {
    profile: &'static str,
    compiled_manifest: serde_json::Value,
    enabled_manifest_default_config: serde_json::Value,
    registered_tool_names_default_config: Vec<String>,
    registered_llm_provider_ids_dummy_config: Vec<String>,
    registered_llm_provider_aliases_dummy_config: Vec<String>,
    storage_backend_module_ids: Vec<&'static str>,
    sandbox_backend_module_ids: Vec<&'static str>,
    external_service_requirements: Vec<ExternalServiceRequirementSnapshot>,
}

#[derive(Serialize)]
struct ExternalServiceRequirementSnapshot {
    module_id: &'static str,
    kind: CapabilityKind,
    cargo_feature: &'static str,
    config_env: Vec<&'static str>,
    requires_capabilities: Vec<Vec<&'static str>>,
}

#[test]
fn modular_registry_snapshot_covers_manifest_and_tool_lists() {
    let profile = compiled_profile_label();
    let settings = Arc::new(AgentSettings::default());
    let compiled_manifest =
        compiled_capability_manifest().expect("compiled capability manifest should be valid");
    let enabled_manifest = compiled_manifest
        .enabled_manifest_from_configured_modules(
            settings
                .modules
                .iter()
                .map(|(module_id, config)| (module_id.as_str(), config.enabled_or_default())),
        )
        .expect("default module config should produce enabled manifest");
    let llm_client = Arc::new(LlmClient::new(settings.as_ref()));
    let session = AgentSession::new(9_i64.into());
    let executor = AgentExecutor::new(llm_client, session, settings);
    let (_chatgpt_auth_file, provider_settings) = settings_with_dummy_provider_config(
        compiled_manifest
            .modules()
            .iter()
            .filter(|module| module.kind() == CapabilityKind::LlmProvider),
    );
    let provider_client = LlmClient::new(&provider_settings);
    let registered_tool_names_default_config: Vec<_> = executor
        .current_tool_definitions()
        .into_iter()
        .map(|tool| tool.name)
        .collect();

    assert_tool_availability_contract(
        profile,
        compiled_manifest.modules(),
        enabled_manifest.modules(),
        enabled_manifest.capabilities(),
        &registered_tool_names_default_config,
    );
    let registered_provider_names = provider_client.configured_provider_names();
    assert_provider_alias_contract(
        profile,
        enabled_manifest.modules(),
        &registered_provider_names,
    );

    let snapshot = ModularRegistrySnapshot {
        profile,
        compiled_manifest: serde_json::from_str(
            &compiled_manifest
                .to_json_pretty()
                .expect("compiled manifest should serialize"),
        )
        .expect("compiled manifest JSON should parse"),
        enabled_manifest_default_config: serde_json::from_str(
            &enabled_manifest
                .to_json_pretty()
                .expect("enabled manifest should serialize"),
        )
        .expect("enabled manifest JSON should parse"),
        registered_tool_names_default_config,
        registered_llm_provider_ids_dummy_config: registered_provider_names
            .iter()
            .filter(|provider| provider.starts_with("llm-provider/"))
            .cloned()
            .collect(),
        registered_llm_provider_aliases_dummy_config: registered_provider_names
            .into_iter()
            .filter(|provider| !provider.starts_with("llm-provider/"))
            .collect(),
        storage_backend_module_ids: module_ids_by_kind(
            compiled_manifest.modules(),
            CapabilityKind::StorageBackend,
        ),
        sandbox_backend_module_ids: module_ids_by_kind(
            compiled_manifest.modules(),
            CapabilityKind::SandboxBackend,
        ),
        external_service_requirements: external_service_requirements(compiled_manifest.modules()),
    };

    insta::with_settings!({ snapshot_suffix => profile }, {
        insta::assert_snapshot!(
            "modular_registry_snapshot",
            serde_json::to_string_pretty(&snapshot).expect("snapshot should serialize")
        );
    });
}

fn settings_with_dummy_provider_config<'a>(
    provider_modules: impl IntoIterator<Item = &'a ModuleManifestEntry>,
) -> (Option<NamedTempFile>, AgentSettings) {
    let mut settings = AgentSettings::default();
    let mut chatgpt_auth_file = None;

    for module in provider_modules {
        let mut config = ModuleRuntimeConfig::default();
        for property in module.config_properties() {
            match property.name() {
                "api_key" => {
                    config = config.with_string_value(property.name(), "test-api-key");
                }
                "auth_path" => {
                    let auth_file =
                        NamedTempFile::new().expect("dummy ChatGPT auth file should be created");
                    config = config.with_string_value(
                        property.name(),
                        auth_file
                            .path()
                            .to_str()
                            .expect("dummy auth path should be UTF-8"),
                    );
                    chatgpt_auth_file = Some(auth_file);
                }
                _ => {}
            }
        }
        settings
            .modules
            .insert(module.id().as_str().to_string(), config);
    }

    (chatgpt_auth_file, settings)
}

fn module_ids_by_kind(modules: &[ModuleManifestEntry], kind: CapabilityKind) -> Vec<&'static str> {
    modules
        .iter()
        .filter(|module| module.kind() == kind)
        .map(|module| module.id().as_str())
        .collect()
}

fn external_service_requirements(
    modules: &[ModuleManifestEntry],
) -> Vec<ExternalServiceRequirementSnapshot> {
    modules
        .iter()
        .filter(|module| {
            !module.config_properties().is_empty()
                || matches!(
                    module.kind(),
                    CapabilityKind::Browser
                        | CapabilityKind::LlmProvider
                        | CapabilityKind::McpIntegration
                        | CapabilityKind::SandboxBackend
                        | CapabilityKind::Search
                        | CapabilityKind::Service
                        | CapabilityKind::StorageBackend
                )
        })
        .map(|module| ExternalServiceRequirementSnapshot {
            module_id: module.id().as_str(),
            kind: module.kind(),
            cargo_feature: module.cargo_feature(),
            config_env: module
                .config_properties()
                .iter()
                .filter_map(|property| property.env())
                .collect(),
            requires_capabilities: module
                .requires()
                .iter()
                .map(|requirement| {
                    requirement
                        .capability_options()
                        .into_iter()
                        .map(|capability| capability.as_str())
                        .collect()
                })
                .collect(),
        })
        .collect()
}

fn assert_tool_availability_contract(
    profile: &str,
    compiled_modules: &[ModuleManifestEntry],
    enabled_modules: &[ModuleId],
    enabled_capabilities: &[CapabilityId],
    registered_tool_names: &[String],
) {
    let compiled_module_ids: BTreeSet<_> = compiled_modules
        .iter()
        .map(|module| module.id().as_str())
        .collect();
    let enabled_module_ids: BTreeSet<_> = enabled_modules
        .iter()
        .map(|module_id| module_id.as_str())
        .collect();
    let enabled_capability_ids: BTreeSet<_> = enabled_capabilities
        .iter()
        .map(|capability_id| capability_id.as_str())
        .collect();
    let tool_names: BTreeSet<_> = registered_tool_names.iter().map(String::as_str).collect();

    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "tool/media-audio",
        &["transcribe_audio_file"],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "tool/media-image",
        &["describe_image_file"],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "tool/media-video",
        &["describe_video_file"],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "tool/tavily",
        &["web_search", "web_extract"],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "integration/mcp-jira",
        &["jira_read", "jira_write", "jira_schema"],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "integration/mcp-mattermost",
        &[
            "mattermost_list_teams",
            "mattermost_get_team",
            "mattermost_get_team_members",
            "mattermost_list_channels",
            "mattermost_get_channel",
            "mattermost_get_channel_by_name",
            "mattermost_create_channel",
            "mattermost_join_channel",
            "mattermost_create_direct_channel",
            "mattermost_post_message",
            "mattermost_get_channel_messages",
            "mattermost_search_messages",
            "mattermost_update_message",
            "mattermost_get_thread",
            "mattermost_get_me",
            "mattermost_get_user",
            "mattermost_get_user_by_username",
            "mattermost_search_users",
            "mattermost_upload_file",
        ],
    );
    assert_tools_absent_when_module_unavailable(
        &compiled_module_ids,
        &enabled_module_ids,
        &tool_names,
        "integration/ssh-mcp",
        &[
            "ssh_exec",
            "ssh_sudo_exec",
            "ssh_read_file",
            "ssh_apply_file_edit",
            "ssh_check_process",
            "ssh_send_file_to_user",
        ],
    );

    match profile {
        "profile-embedded-opencode-local" | "profile-web-embedded-opencode-local" => {
            assert!(
                enabled_module_ids.contains("sandbox-backend/bwrap"),
                "embedded-opencode-local profile must enable the bwrap sandbox backend"
            );
            if profile == "profile-web-embedded-opencode-local" {
                assert!(
                    enabled_module_ids.contains("transport/web"),
                    "web embedded profile must enable the web transport"
                );
                assert!(
                    !enabled_module_ids.contains("transport/telegram"),
                    "web embedded profile must not enable the Telegram transport"
                );
                assert!(
                    enabled_module_ids.contains("sandbox-backend/sandboxd-client"),
                    "web embedded profile must enable the sandboxd client backend"
                );
            } else {
                assert!(
                    enabled_module_ids.contains("transport/telegram"),
                    "embedded-opencode-local profile must enable the Telegram transport"
                );
                assert!(
                    enabled_module_ids.contains("sandbox-backend/docker-direct"),
                    "embedded-opencode-local profile must enable the direct Docker sandbox backend"
                );
            }
            assert_present_capabilities(
                &enabled_capability_ids,
                &[
                    "sandbox-backend/bwrap/exec",
                    "sandbox-backend/bwrap/fileops",
                    "sandbox-backend/bwrap/lifecycle",
                    "tool/compression",
                    "tool/delegation",
                    "tool/file-delivery",
                    "tool/media-audio-transcription",
                    "tool/media-image-description",
                    "tool/media-video-description",
                    "tool/sandbox-exec",
                    "tool/sandbox-fileops",
                    "tool/sandbox-list-files",
                    "tool/sandbox-recreate",
                    "tool/tavily-search",
                    "tool/tavily-extract",
                ],
                profile,
            );
            if profile == "profile-web-embedded-opencode-local" {
                assert_present_capabilities(
                    &enabled_capability_ids,
                    &[
                        "sandbox-backend/sandboxd-client/exec",
                        "sandbox-backend/sandboxd-client/fileops",
                        "sandbox-backend/sandboxd-client/lifecycle",
                    ],
                    profile,
                );
            } else {
                assert_present_capabilities(
                    &enabled_capability_ids,
                    &[
                        "sandbox-backend/docker-direct/exec",
                        "sandbox-backend/docker-direct/fileops",
                        "sandbox-backend/docker-direct/lifecycle",
                    ],
                    profile,
                );
            }
            assert_present_tools(
                &tool_names,
                &[
                    "apply_file_edit",
                    "execute_command",
                    "cancel_sub_agents",
                    "compress",
                    "describe_image_file",
                    "describe_video_file",
                    "list_files",
                    "read_file",
                    "recreate_sandbox",
                    "send_file_to_user",
                    "spawn_sub_agents",
                    "transcribe_audio_file",
                    "upload_file",
                    "wait_sub_agents",
                    "write_file",
                ],
                profile,
            );
            assert_absent_tool_prefix(&tool_names, "jira_", profile);
            assert_absent_tool_prefix(&tool_names, "mattermost_", profile);
            assert_absent_tool_prefix(&tool_names, "ssh_", profile);
        }
        "profile-lite" => {
            assert_absent_tools(&tool_names, &["execute_command"], profile);
            assert_absent_tool_prefix(&tool_names, "jira_", profile);
            assert_absent_tool_prefix(&tool_names, "mattermost_", profile);
            assert_absent_tool_prefix(&tool_names, "ssh_", profile);
        }
        "profile-search-only" => {
            assert!(
                enabled_module_ids.contains("tool/tavily"),
                "search-only profile must compile and enable Tavily search capabilities"
            );
            assert_present_capabilities(
                &enabled_capability_ids,
                &["tool/tavily-search", "tool/tavily-extract"],
                profile,
            );
            assert_present_tools(&tool_names, &["web_markdown"], profile);
            assert_absent_tool_prefix(&tool_names, "jira_", profile);
            assert_absent_tool_prefix(&tool_names, "mattermost_", profile);
            assert_absent_tool_prefix(&tool_names, "ssh_", profile);
        }
        "profile-media-enabled" => {
            assert_present_capabilities(
                &enabled_capability_ids,
                &[
                    "tool/media-audio-transcription",
                    "tool/media-image-description",
                    "tool/media-video-description",
                ],
                profile,
            );
            assert_present_tools(
                &tool_names,
                &[
                    "transcribe_audio_file",
                    "describe_image_file",
                    "describe_video_file",
                ],
                profile,
            );
            assert_absent_tools(&tool_names, &["execute_command"], profile);
            assert!(
                enabled_module_ids
                    .iter()
                    .all(|module_id| !module_id.starts_with("sandbox-backend/")),
                "media-enabled profile must expose media tools without selecting a sandbox backend"
            );
        }
        "profile-host-bwrap" => {
            assert!(
                enabled_module_ids.contains("sandbox-backend/bwrap"),
                "host-bwrap profile must enable the Bubblewrap sandbox backend"
            );
            assert!(
                !enabled_module_ids.contains("sandbox-backend/docker-direct"),
                "host-bwrap profile must not enable the direct Docker sandbox backend"
            );
            assert!(
                !enabled_module_ids.contains("sandbox-backend/sandboxd-client"),
                "host-bwrap profile must not enable the sandboxd client backend"
            );
            assert_present_capabilities(
                &enabled_capability_ids,
                &[
                    "sandbox-backend/bwrap/exec",
                    "sandbox-backend/bwrap/fileops",
                    "sandbox-backend/bwrap/lifecycle",
                    "tool/sandbox-exec",
                    "tool/sandbox-fileops",
                    "tool/sandbox-list-files",
                    "tool/sandbox-recreate",
                ],
                profile,
            );
            assert_present_tools(
                &tool_names,
                &[
                    "apply_file_edit",
                    "execute_command",
                    "list_files",
                    "read_file",
                    "recreate_sandbox",
                    "write_file",
                ],
                profile,
            );
            assert_absent_tool_prefix(&tool_names, "jira_", profile);
            assert_absent_tool_prefix(&tool_names, "mattermost_", profile);
            assert_absent_tool_prefix(&tool_names, "ssh_", profile);
        }
        _ => {}
    }
}

fn assert_provider_alias_contract(
    profile: &str,
    enabled_modules: &[ModuleId],
    registered_provider_names: &[String],
) {
    let enabled_module_ids: BTreeSet<_> = enabled_modules
        .iter()
        .map(|module_id| module_id.as_str())
        .collect();
    let provider_names: BTreeSet<_> = registered_provider_names
        .iter()
        .map(String::as_str)
        .collect();
    let allowed_provider_names = allowed_provider_names_for_enabled_modules(&enabled_module_ids);

    for direct_gemini_name in [
        "gemini",
        "google-gemini",
        "google_gemini",
        "llm-provider/gemini",
        "llm-provider/google-gemini",
        "llm-provider/google-gemini-direct",
    ] {
        assert!(
            !provider_names.contains(direct_gemini_name),
            "direct Gemini provider name must stay absent for {profile}: {direct_gemini_name}"
        );
    }

    for provider_name in &provider_names {
        assert!(
            allowed_provider_names.contains(provider_name),
            "registered provider name {provider_name} is not owned by an enabled provider module for {profile}; allowed={allowed_provider_names:?}"
        );
    }

    for module_id in enabled_module_ids
        .iter()
        .copied()
        .filter(|module_id| module_id.starts_with("llm-provider/"))
    {
        assert!(
            provider_names.contains(module_id),
            "enabled provider module {module_id} must register its canonical provider ID for {profile}"
        );
    }
}

fn allowed_provider_names_for_enabled_modules(
    enabled_module_ids: &BTreeSet<&str>,
) -> BTreeSet<&'static str> {
    let mut allowed = BTreeSet::new();

    for module_id in enabled_module_ids {
        match *module_id {
            "llm-provider/minimax" => {
                allowed.extend(["llm-provider/minimax", "minimax"]);
            }
            "llm-provider/mistral" => {
                allowed.extend(["llm-provider/mistral", "mistral"]);
            }
            "llm-provider/nvidia" => {
                allowed.extend(["llm-provider/nvidia", "nvidia"]);
            }
            "llm-provider/openai-chatgpt" => {
                allowed.extend(["llm-provider/openai-chatgpt", "chatgpt", "openai-chatgpt"]);
            }
            "llm-provider/opencode-go" => {
                allowed.extend([
                    "llm-provider/opencode-go",
                    "opencode-go",
                    "opencode_go",
                    "llm-provider/opencode-zen",
                    "opencode-zen",
                    "opencode_zen",
                ]);
            }
            "llm-provider/openrouter" => {
                allowed.extend(["llm-provider/openrouter", "openrouter"]);
            }
            "llm-provider/zai" => {
                allowed.extend(["llm-provider/zai", "zai"]);
            }
            _ => {}
        }
    }

    allowed
}

fn assert_tools_absent_when_module_unavailable(
    compiled_module_ids: &BTreeSet<&str>,
    enabled_module_ids: &BTreeSet<&str>,
    tool_names: &BTreeSet<&str>,
    module_id: &str,
    unavailable_tools: &[&str],
) {
    if compiled_module_ids.contains(module_id) && enabled_module_ids.contains(module_id) {
        return;
    }

    assert_absent_tools(tool_names, unavailable_tools, module_id);
}

fn assert_present_capabilities(
    enabled_capability_ids: &BTreeSet<&str>,
    capabilities: &[&str],
    profile: &str,
) {
    for capability in capabilities {
        assert!(
            enabled_capability_ids.contains(capability),
            "expected capability {capability} to be enabled for {profile}"
        );
    }
}

fn assert_present_tools(tool_names: &BTreeSet<&str>, expected_tools: &[&str], context: &str) {
    for tool_name in expected_tools {
        assert!(
            tool_names.contains(tool_name),
            "expected tool {tool_name} to be registered for {context}; registered={tool_names:?}"
        );
    }
}

fn assert_absent_tools(tool_names: &BTreeSet<&str>, forbidden_tools: &[&str], context: &str) {
    for tool_name in forbidden_tools {
        assert!(
            !tool_names.contains(tool_name),
            "tool {tool_name} must be absent for {context}; registered={tool_names:?}"
        );
    }
}

fn assert_absent_tool_prefix(tool_names: &BTreeSet<&str>, prefix: &str, context: &str) {
    assert!(
        tool_names
            .iter()
            .all(|tool_name| !tool_name.starts_with(prefix)),
        "tool prefix {prefix} must be absent for {context}; registered={tool_names:?}"
    );
}

fn compiled_profile_label() -> &'static str {
    let active_profile_count = cfg!(feature = "profile-embedded-opencode-local") as usize
        + cfg!(feature = "profile-web-embedded-opencode-local") as usize
        + cfg!(feature = "profile-lite") as usize
        + cfg!(feature = "profile-search-only") as usize
        + cfg!(feature = "profile-no-sandbox") as usize
        + cfg!(feature = "profile-media-enabled") as usize
        + cfg!(feature = "profile-host-bwrap") as usize
        + cfg!(feature = "profile-full") as usize;

    if active_profile_count != 1 {
        return "all-features";
    }

    if cfg!(feature = "profile-embedded-opencode-local") {
        "profile-embedded-opencode-local"
    } else if cfg!(feature = "profile-web-embedded-opencode-local") {
        "profile-web-embedded-opencode-local"
    } else if cfg!(feature = "profile-lite") {
        "profile-lite"
    } else if cfg!(feature = "profile-search-only") {
        "profile-search-only"
    } else if cfg!(feature = "profile-no-sandbox") {
        "profile-no-sandbox"
    } else if cfg!(feature = "profile-media-enabled") {
        "profile-media-enabled"
    } else if cfg!(feature = "profile-host-bwrap") {
        "profile-host-bwrap"
    } else {
        "profile-full"
    }
}
