use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for OpenRouter routes.
pub(crate) struct OpenRouterProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const SITE_URL_CONFIG_KEY: &str = "site_url";
const SITE_URL_ENV: &str = "OPENROUTER_SITE_URL";
const SITE_NAME_CONFIG_KEY: &str = "site_name";
const SITE_NAME_ENV: &str = "OPENROUTER_SITE_NAME";
const DEFAULT_SITE_NAME: &str = "Oxide Agent Bot";

impl LlmProviderModule for OpenRouterProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/openrouter"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["openrouter"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                let site_url = settings.module_string_value_or_env_or_default(
                    self.provider_id(),
                    SITE_URL_CONFIG_KEY,
                    SITE_URL_ENV,
                    "",
                );
                let site_name = settings.module_string_value_or_env_or_default(
                    self.provider_id(),
                    SITE_NAME_CONFIG_KEY,
                    SITE_NAME_ENV,
                    DEFAULT_SITE_NAME,
                );
                Arc::new(super::OpenRouterProvider::new_with_client(
                    api_key,
                    site_url,
                    site_name,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, false)
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(true, true, true)
    }
}
