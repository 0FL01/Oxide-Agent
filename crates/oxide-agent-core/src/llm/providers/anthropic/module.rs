use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for generic Anthropic Messages API routes.
pub(crate) struct AnthropicProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const API_BASE_CONFIG_KEY: &str = "api_base";
const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";

impl LlmProviderModule for AnthropicProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/anthropic"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["anthropic"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                let api_base = settings.module_string_value_or_env_or_default(
                    self.provider_id(),
                    API_BASE_CONFIG_KEY,
                    "",
                    DEFAULT_ANTHROPIC_URL,
                );
                Arc::new(super::AnthropicProvider::new(
                    api_key,
                    ctx.http_client.clone(),
                    api_base,
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)
    }
}
