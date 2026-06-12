use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for generic OpenAI-compatible routes.
pub(crate) struct OpenAIBaseProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "OPENAI_BASE_API_KEY";
const API_BASE_CONFIG_KEY: &str = "api_base";
const API_BASE_ENV: &str = "OPENAI_BASE_API_BASE";
#[cfg(feature = "llm-opencode-go")]
const MODELS_URL_CONFIG_KEY: &str = "models_url";
#[cfg(feature = "llm-opencode-go")]
const MODELS_URL_ENV: &str = "OPENAI_BASE_MODELS_URL";
#[cfg(feature = "llm-opencode-go")]
const MODEL_CACHE_TTL_SECS_CONFIG_KEY: &str = "model_cache_ttl_secs";
#[cfg(feature = "llm-opencode-go")]
const MODEL_CACHE_TTL_SECS_ENV: &str = "OPENAI_BASE_MODEL_CACHE_TTL_SECS";

#[cfg(feature = "llm-opencode-go")]
pub(crate) fn build_model_catalog(
    settings: &AgentSettings,
    http_client: reqwest::Client,
) -> Option<Arc<crate::llm::providers::opencode_go::discovery::OpenCodeGoModelCatalog>> {
    let api_base = settings.module_string_value_or_env(
        OpenAIBaseProviderModule.provider_id(),
        API_BASE_CONFIG_KEY,
        API_BASE_ENV,
    )?;
    let api_key = settings.module_string_value_or_env(
        OpenAIBaseProviderModule.provider_id(),
        API_KEY_CONFIG_KEY,
        API_KEY_ENV,
    );
    let catalog = Arc::new(
        crate::llm::providers::opencode_go::discovery::OpenCodeGoModelCatalog::new(
            http_client,
            api_key,
            openai_base_discovery_config(settings, &api_base),
        ),
    );
    Arc::clone(&catalog).spawn_background_refresh();
    Some(catalog)
}

#[cfg(feature = "llm-opencode-go")]
fn openai_base_discovery_config(
    settings: &AgentSettings,
    api_base: &str,
) -> crate::llm::providers::opencode_go::discovery::OpenCodeGoDiscoveryConfig {
    crate::llm::providers::opencode_go::discovery::OpenCodeGoDiscoveryConfig::new_openai_base(
        settings
            .module_string_value(
                OpenAIBaseProviderModule.provider_id(),
                MODELS_URL_CONFIG_KEY,
            )
            .unwrap_or_else(|| {
                std::env::var(MODELS_URL_ENV)
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| models_url_from_api_base(api_base))
            }),
        std::time::Duration::from_secs(openai_base_model_cache_ttl_secs(settings)),
    )
}

#[cfg(feature = "llm-opencode-go")]
fn openai_base_model_cache_ttl_secs(settings: &AgentSettings) -> u64 {
    settings
        .module_config(OpenAIBaseProviderModule.provider_id())
        .and_then(|config| config.value(MODEL_CACHE_TTL_SECS_CONFIG_KEY))
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .or_else(|| {
            std::env::var(MODEL_CACHE_TTL_SECS_ENV)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(crate::llm::providers::opencode_go::discovery::DEFAULT_MODEL_DISCOVERY_TTL_SECS)
        .clamp(
            crate::llm::providers::opencode_go::discovery::MIN_MODEL_DISCOVERY_TTL_SECS,
            crate::llm::providers::opencode_go::discovery::MAX_MODEL_DISCOVERY_TTL_SECS,
        )
}

#[cfg(feature = "llm-opencode-go")]
fn models_url_from_api_base(api_base: &str) -> String {
    let chat_url = super::chat_completions_url(api_base);
    chat_url
        .trim_end_matches("/chat/completions")
        .trim_end_matches('/')
        .to_string()
        + "/models"
}

impl LlmProviderModule for OpenAIBaseProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/openai-base"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["openai-base", "openai_base"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        let api_base = settings.module_string_value_or_env(
            self.provider_id(),
            API_BASE_CONFIG_KEY,
            API_BASE_ENV,
        )?;
        let api_key = settings.module_string_value_or_env(
            self.provider_id(),
            API_KEY_CONFIG_KEY,
            API_KEY_ENV,
        );

        Some(Arc::new(super::OpenAIBaseProvider::new_with_client(
            api_key,
            api_base,
            ctx.http_client.clone(),
        )) as Arc<dyn LlmProvider>)
    }

    fn missing_route_config_message(&self, settings: &AgentSettings) -> Option<&'static str> {
        settings
            .module_string_value_or_env(self.provider_id(), API_BASE_CONFIG_KEY, API_BASE_ENV)
            .is_none()
            .then_some(
                "Critical: OPENAI_BASE_API_BASE or modules.llm-provider/openai-base.api_base is required when openai-base routes are configured",
            )
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true)
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(false, true, false)
    }
}
