use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for MiniMax routes.
pub(crate) struct MiniMaxProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "MINIMAX_API_KEY";

impl LlmProviderModule for MiniMaxProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/minimax"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["minimax"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                Arc::new(super::MiniMaxProvider::new(
                    api_key,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)
    }
}
