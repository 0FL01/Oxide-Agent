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
