use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for MiniMax routes.
pub(crate) struct MiniMaxProviderModule;

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
        _ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings.minimax_api_key.as_ref().map(|api_key| {
            Arc::new(super::MiniMaxProvider::new(api_key.clone())) as Arc<dyn LlmProvider>
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, false)
    }
}
