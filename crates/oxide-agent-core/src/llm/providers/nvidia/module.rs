use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for Nvidia hosted model routes.
pub(crate) struct NvidiaProviderModule;

impl LlmProviderModule for NvidiaProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/nvidia"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["nvidia"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings.nvidia_api_key.as_ref().map(|api_key| {
            Arc::new(super::NvidiaProvider::new_with_client(
                api_key.clone(),
                settings.nvidia_api_base.clone(),
                ctx.http_client.clone(),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, true)
    }

    fn capabilities_for_model(&self, model_info: &ModelInfo) -> ProviderCapabilities {
        let mut capabilities = self.capabilities();
        let model_capabilities = super::model_capabilities(&model_info.id);
        capabilities.supports_tool_calling = model_capabilities.supports_tool_calling;
        capabilities.supports_structured_output = model_capabilities.supports_structured_output;
        capabilities
    }
}
