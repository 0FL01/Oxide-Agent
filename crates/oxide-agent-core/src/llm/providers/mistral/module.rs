use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for Mistral routes.
pub(crate) struct MistralProviderModule;

impl LlmProviderModule for MistralProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/mistral"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["mistral"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings.mistral_api_key.as_ref().map(|api_key| {
            Arc::new(super::MistralProvider::new_with_client(
                api_key.clone(),
                ctx.http_client.clone(),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(true, false, false)
    }
}
