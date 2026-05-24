#![cfg(any(
    feature = "profile-embedded-opencode-local",
    feature = "profile-lite",
    feature = "profile-search-only",
    feature = "profile-no-sandbox",
    feature = "profile-media-enabled",
    feature = "profile-full",
))]

use oxide_agent_core::agent::{AgentExecutor, AgentSession};
use oxide_agent_core::capabilities::{
    compiled_capability_manifest, CapabilityKind, ModuleManifestEntry,
};
use oxide_agent_core::config::{AgentSettings, ModuleRuntimeConfig};
use oxide_agent_core::llm::LlmClient;
use serde::Serialize;
use std::sync::Arc;
use tempfile::NamedTempFile;

#[derive(Serialize)]
struct ModularRegistrySnapshot {
    profile: &'static str,
    compiled_manifest: serde_json::Value,
    enabled_manifest_default_config: serde_json::Value,
    registered_tool_names_default_config: Vec<String>,
    registered_llm_provider_ids_dummy_config: Vec<String>,
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
        registered_tool_names_default_config: executor
            .current_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect(),
        registered_llm_provider_ids_dummy_config: provider_client
            .configured_provider_names()
            .into_iter()
            .filter(|provider| provider.starts_with("llm-provider/"))
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

fn compiled_profile_label() -> &'static str {
    let active_profile_count = cfg!(feature = "profile-embedded-opencode-local") as usize
        + cfg!(feature = "profile-lite") as usize
        + cfg!(feature = "profile-search-only") as usize
        + cfg!(feature = "profile-no-sandbox") as usize
        + cfg!(feature = "profile-media-enabled") as usize
        + cfg!(feature = "profile-full") as usize;

    if active_profile_count != 1 {
        return "all-features";
    }

    if cfg!(feature = "profile-embedded-opencode-local") {
        "profile-embedded-opencode-local"
    } else if cfg!(feature = "profile-lite") {
        "profile-lite"
    } else if cfg!(feature = "profile-search-only") {
        "profile-search-only"
    } else if cfg!(feature = "profile-no-sandbox") {
        "profile-no-sandbox"
    } else if cfg!(feature = "profile-media-enabled") {
        "profile-media-enabled"
    } else {
        "profile-full"
    }
}
