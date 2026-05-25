use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for OpenRouter routes.
pub(crate) struct OpenRouterProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "OPENROUTER_API_KEY";

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
                Arc::new(super::OpenRouterProvider::new_with_client(
                    api_key,
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
