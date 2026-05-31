use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Capability module for OpenCode Go routes.
pub(crate) struct OpenCodeGoProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENVS: &[&str] = &[
    "OPENCODE_API_KEY",
    "OPENCODE_ZEN_API_KEY",
    "OPENCODE_GO_API_KEY",
];
const API_BASE_CONFIG_KEY: &str = "api_base";
const API_BASE_ENV: &str = "OPENCODE_GO_API_BASE";
const DEFAULT_API_BASE: &str = "https://opencode.ai/zen/go/v1/chat/completions";
const MESSAGES_API_BASE_CONFIG_KEY: &str = "messages_api_base";
const MESSAGES_API_BASE_ENV: &str = "OPENCODE_GO_MESSAGES_API_BASE";
const DEFAULT_MESSAGES_API_BASE: &str = "https://opencode.ai/zen/go/v1/messages";
const MODELS_URL_CONFIG_KEY: &str = "models_url";
const MODELS_URL_ENV: &str = "OPENCODE_GO_MODELS_URL";
const MODEL_CACHE_TTL_SECS_CONFIG_KEY: &str = "model_cache_ttl_secs";
const MODEL_CACHE_TTL_SECS_ENV: &str = "OPENCODE_GO_MODEL_CACHE_TTL_SECS";
const PROTOCOL_OVERRIDES_CONFIG_KEY: &str = "protocol_overrides";

pub(crate) fn build_model_catalog(
    settings: &AgentSettings,
    http_client: reqwest::Client,
) -> Option<Arc<super::discovery::OpenCodeGoModelCatalog>> {
    let api_key = configured_api_key(settings, OpenCodeGoProviderModule.provider_id())?;
    let catalog = Arc::new(super::discovery::OpenCodeGoModelCatalog::new(
        http_client,
        api_key,
        discovery_config(settings, OpenCodeGoProviderModule.provider_id()),
    ));
    Arc::clone(&catalog).spawn_background_refresh();
    Some(catalog)
}

impl LlmProviderModule for OpenCodeGoProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/opencode-go"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["opencode-go", "opencode_go"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        configured_api_key(settings, self.provider_id()).map(|api_key| {
            let api_base = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                API_BASE_CONFIG_KEY,
                API_BASE_ENV,
                DEFAULT_API_BASE,
            );
            let api_base_messages = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                MESSAGES_API_BASE_CONFIG_KEY,
                MESSAGES_API_BASE_ENV,
                DEFAULT_MESSAGES_API_BASE,
            );
            Arc::new(super::OpenCodeGoProvider::new_with_client_and_discovery(
                api_key,
                api_base,
                api_base_messages,
                ctx.http_client.clone(),
                discovery_config(settings, self.provider_id()),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn missing_route_config_message(&self, settings: &AgentSettings) -> Option<&'static str> {
        settings
            .module_string_value_or_envs(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENVS)
            .is_none()
            .then_some(
                "Critical: OPENCODE_API_KEY, OPENCODE_ZEN_API_KEY, or OPENCODE_GO_API_KEY is required for configured OpenCode Go routes",
            )
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
    }

    fn capabilities_for_model(&self, _model_info: &ModelInfo) -> ProviderCapabilities {
        // All opencode-go models share a unified tool and structured-output protocol.
        self.capabilities()
    }
}

fn configured_api_key(settings: &AgentSettings, module_id: &str) -> Option<String> {
    settings.module_string_value_or_envs(module_id, API_KEY_CONFIG_KEY, API_KEY_ENVS)
}

fn discovery_config(
    settings: &AgentSettings,
    module_id: &str,
) -> super::discovery::OpenCodeGoDiscoveryConfig {
    super::discovery::OpenCodeGoDiscoveryConfig::new(
        module_string_value_or_env_or_default(
            settings,
            module_id,
            MODELS_URL_CONFIG_KEY,
            MODELS_URL_ENV,
            super::discovery::DEFAULT_MODELS_URL,
        ),
        std::time::Duration::from_secs(module_u64_value_or_env_or_default(
            settings,
            module_id,
            MODEL_CACHE_TTL_SECS_CONFIG_KEY,
            MODEL_CACHE_TTL_SECS_ENV,
            super::discovery::DEFAULT_MODEL_DISCOVERY_TTL_SECS,
        )),
        protocol_overrides(settings, module_id),
    )
}

fn module_string_value_or_env_or_default(
    settings: &AgentSettings,
    module_id: &str,
    key: &str,
    env_name: &str,
    default: &str,
) -> String {
    settings
        .module_string_value(module_id, key)
        .unwrap_or_else(|| {
            std::env::var(env_name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| default.to_string())
        })
}

fn module_u64_value_or_env_or_default(
    settings: &AgentSettings,
    module_id: &str,
    key: &str,
    env_name: &str,
    default: u64,
) -> u64 {
    settings
        .module_config(module_id)
        .and_then(|config| config.value(key))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .or_else(|| {
            std::env::var(env_name)
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(default)
        .clamp(
            super::discovery::MIN_MODEL_DISCOVERY_TTL_SECS,
            super::discovery::MAX_MODEL_DISCOVERY_TTL_SECS,
        )
}

fn protocol_overrides(
    settings: &AgentSettings,
    module_id: &str,
) -> BTreeMap<String, super::discovery::ModelProtocol> {
    settings
        .module_config(module_id)
        .and_then(|config| config.value(PROTOCOL_OVERRIDES_CONFIG_KEY))
        .and_then(serde_json::Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(model_id, value)| {
                    value.as_str().and_then(|protocol| {
                        super::discovery::ModelProtocol::from_str(protocol)
                            .ok()
                            .map(|protocol| (model_id.clone(), protocol))
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}
