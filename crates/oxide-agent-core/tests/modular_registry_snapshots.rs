#![cfg(any(
    feature = "profile-embedded-opencode-local",
    feature = "profile-lite",
    feature = "profile-search-only",
    feature = "profile-no-sandbox",
    feature = "profile-media-enabled",
    feature = "profile-full",
))]

use oxide_agent_core::agent::{AgentExecutor, AgentSession};
use oxide_agent_core::capabilities::compiled_capability_manifest;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct ModularRegistrySnapshot {
    profile: &'static str,
    compiled_manifest: serde_json::Value,
    enabled_manifest_default_config: serde_json::Value,
    registered_tool_names_default_config: Vec<String>,
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
    };

    insta::with_settings!({ snapshot_suffix => profile }, {
        insta::assert_snapshot!(
            "modular_registry_snapshot",
            serde_json::to_string_pretty(&snapshot).expect("snapshot should serialize")
        );
    });
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
