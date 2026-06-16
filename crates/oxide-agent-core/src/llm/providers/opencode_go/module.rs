use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use std::collections::BTreeMap;
use std::str::FromStr;

/// Capability module for OpenCode Go routes.
pub(crate) struct OpenCodeGoProviderModule;
/// Capability module for free OpenCode Zen routes.
pub(crate) struct OpenCodeZenProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const GO_API_KEY_ENVS: &[&str] = &[
    "OPENCODE_API_KEY",
    "OPENCODE_ZEN_API_KEY",
    "OPENCODE_GO_API_KEY",
];
const ZEN_API_KEY_ENVS: &[&str] = &[
    "OPENCODE_ZEN_API_KEY",
    "OPENCODE_API_KEY",
    "OPENCODE_GO_API_KEY",
];
const API_BASE_CONFIG_KEY: &str = "api_base";
const GO_API_BASE_ENV: &str = "OPENCODE_GO_API_BASE";
const DEFAULT_GO_API_BASE: &str = "https://opencode.ai/zen/go/v1/chat/completions";
const ZEN_API_BASE_ENV: &str = "OPENCODE_ZEN_API_BASE";
const DEFAULT_ZEN_API_BASE: &str = "https://opencode.ai/zen/v1/chat/completions";
const MESSAGES_API_BASE_CONFIG_KEY: &str = "messages_api_base";
const GO_MESSAGES_API_BASE_ENV: &str = "OPENCODE_GO_MESSAGES_API_BASE";
const DEFAULT_GO_MESSAGES_API_BASE: &str = "https://opencode.ai/zen/go/v1/messages";
const ZEN_MESSAGES_API_BASE_ENV: &str = "OPENCODE_ZEN_MESSAGES_API_BASE";
const DEFAULT_ZEN_MESSAGES_API_BASE: &str = "https://opencode.ai/zen/v1/messages";
const MODELS_URL_CONFIG_KEY: &str = "models_url";
const GO_MODELS_URL_ENV: &str = "OPENCODE_GO_MODELS_URL";
const ZEN_MODELS_URL_ENV: &str = "OPENCODE_ZEN_MODELS_URL";
const MODEL_CACHE_TTL_SECS_CONFIG_KEY: &str = "model_cache_ttl_secs";
const GO_MODEL_CACHE_TTL_SECS_ENV: &str = "OPENCODE_GO_MODEL_CACHE_TTL_SECS";
const ZEN_MODEL_CACHE_TTL_SECS_ENV: &str = "OPENCODE_ZEN_MODEL_CACHE_TTL_SECS";
const PROTOCOL_OVERRIDES_CONFIG_KEY: &str = "protocol_overrides";

pub(crate) fn build_model_catalog(
    settings: &AgentSettings,
    http_client: reqwest::Client,
) -> Option<Arc<super::discovery::OpenCodeGoModelCatalog>> {
    let api_key = configured_api_key(
        settings,
        OpenCodeGoProviderModule.provider_id(),
        GO_API_KEY_ENVS,
    )?;
    let catalog = Arc::new(super::discovery::OpenCodeGoModelCatalog::new(
        http_client,
        Some(api_key),
        go_discovery_config(settings, OpenCodeGoProviderModule.provider_id()),
    ));
    Arc::clone(&catalog).spawn_background_refresh();
    Some(catalog)
}

pub(crate) fn build_zen_model_catalog(
    settings: &AgentSettings,
    http_client: reqwest::Client,
) -> Option<Arc<super::discovery::OpenCodeGoModelCatalog>> {
    let api_key = configured_api_key(
        settings,
        OpenCodeZenProviderModule.provider_id(),
        ZEN_API_KEY_ENVS,
    )?;
    let catalog = Arc::new(super::discovery::OpenCodeGoModelCatalog::new(
        http_client,
        Some(api_key),
        zen_discovery_config(settings, OpenCodeZenProviderModule.provider_id()),
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
        configured_api_key(settings, self.provider_id(), GO_API_KEY_ENVS).map(|api_key| {
            let api_base = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                API_BASE_CONFIG_KEY,
                GO_API_BASE_ENV,
                DEFAULT_GO_API_BASE,
            );
            let api_base_messages = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                MESSAGES_API_BASE_CONFIG_KEY,
                GO_MESSAGES_API_BASE_ENV,
                DEFAULT_GO_MESSAGES_API_BASE,
            );
            Arc::new(super::OpenCodeGoProvider::new_with_client_and_discovery(
                api_key,
                api_base,
                api_base_messages,
                ctx.http_client.clone(),
                go_discovery_config(settings, self.provider_id()),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn missing_route_config_message(
        &self,
        _provider_name: &str,
        settings: &AgentSettings,
    ) -> Option<String> {
        settings
            .module_string_value_or_envs(self.provider_id(), API_KEY_CONFIG_KEY, GO_API_KEY_ENVS)
            .is_none()
            .then(|| "Critical: OPENCODE_API_KEY, OPENCODE_ZEN_API_KEY, or OPENCODE_GO_API_KEY is required for configured OpenCode Go routes".to_string())
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)
    }

    fn capabilities_for_model(&self, _model_info: &ModelInfo) -> ProviderCapabilities {
        // OpenCode Go routes use native tool calling, but model-side structured JSON
        // compliance is not reliable enough for mandatory agent envelopes.
        self.capabilities()
    }

    fn media_capabilities_for_model(&self, model_info: &ModelInfo) -> MediaCapabilities {
        MediaCapabilities::new(
            false,
            super::discovery::supports_image_input_for_model_id(&model_info.id),
            false,
        )
    }
}

impl LlmProviderModule for OpenCodeZenProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/opencode-zen"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["opencode-zen", "opencode_zen"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        configured_api_key(settings, self.provider_id(), ZEN_API_KEY_ENVS).map(|api_key| {
            let api_base = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                API_BASE_CONFIG_KEY,
                ZEN_API_BASE_ENV,
                DEFAULT_ZEN_API_BASE,
            );
            let api_base_messages = settings.module_string_value_or_env_or_default(
                self.provider_id(),
                MESSAGES_API_BASE_CONFIG_KEY,
                ZEN_MESSAGES_API_BASE_ENV,
                DEFAULT_ZEN_MESSAGES_API_BASE,
            );
            Arc::new(
                super::OpenCodeGoProvider::new_zen_with_client_and_discovery(
                    api_key,
                    api_base,
                    api_base_messages,
                    ctx.http_client.clone(),
                    zen_discovery_config(settings, self.provider_id()),
                ),
            ) as Arc<dyn LlmProvider>
        })
    }

    fn missing_route_config_message(
        &self,
        _provider_name: &str,
        settings: &AgentSettings,
    ) -> Option<String> {
        settings
            .module_string_value_or_envs(self.provider_id(), API_KEY_CONFIG_KEY, ZEN_API_KEY_ENVS)
            .is_none()
            .then(|| "Critical: OPENCODE_ZEN_API_KEY, OPENCODE_API_KEY, or OPENCODE_GO_API_KEY is required for configured OpenCode Zen routes".to_string())
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)
    }

    fn capabilities_for_model(&self, _model_info: &ModelInfo) -> ProviderCapabilities {
        self.capabilities()
    }
}

fn configured_api_key(
    settings: &AgentSettings,
    module_id: &str,
    env_names: &[&str],
) -> Option<String> {
    settings.module_string_value_or_envs(module_id, API_KEY_CONFIG_KEY, env_names)
}

fn go_discovery_config(
    settings: &AgentSettings,
    module_id: &str,
) -> super::discovery::OpenCodeGoDiscoveryConfig {
    super::discovery::OpenCodeGoDiscoveryConfig::new(
        module_string_value_or_env_or_default(
            settings,
            module_id,
            MODELS_URL_CONFIG_KEY,
            GO_MODELS_URL_ENV,
            super::discovery::DEFAULT_MODELS_URL,
        ),
        std::time::Duration::from_secs(module_u64_value_or_env_or_default(
            settings,
            module_id,
            MODEL_CACHE_TTL_SECS_CONFIG_KEY,
            GO_MODEL_CACHE_TTL_SECS_ENV,
            super::discovery::DEFAULT_MODEL_DISCOVERY_TTL_SECS,
        )),
        protocol_overrides(settings, module_id),
    )
}

fn zen_discovery_config(
    settings: &AgentSettings,
    module_id: &str,
) -> super::discovery::OpenCodeGoDiscoveryConfig {
    super::discovery::OpenCodeGoDiscoveryConfig::new_zen(
        module_string_value_or_env_or_default(
            settings,
            module_id,
            MODELS_URL_CONFIG_KEY,
            ZEN_MODELS_URL_ENV,
            "https://opencode.ai/zen/v1/models",
        ),
        std::time::Duration::from_secs(module_u64_value_or_env_or_default(
            settings,
            module_id,
            MODEL_CACHE_TTL_SECS_CONFIG_KEY,
            ZEN_MODEL_CACHE_TTL_SECS_ENV,
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
