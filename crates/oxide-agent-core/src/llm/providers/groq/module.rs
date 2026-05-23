use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for Groq routes.
pub(crate) struct GroqProviderModule;

impl LlmProviderModule for GroqProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/groq"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["groq"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        _ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings.groq_api_key.as_ref().map(|api_key| {
            Arc::new(super::GroqProvider::new(api_key.clone())) as Arc<dyn LlmProvider>
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, true)
    }
}
