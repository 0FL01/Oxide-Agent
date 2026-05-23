use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for ZAI/Zhipu routes.
pub(crate) struct ZaiProviderModule;

impl LlmProviderModule for ZaiProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/zai"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["zai"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings.zai_api_key.as_ref().map(|api_key| {
            Arc::new(super::ZaiProvider::new_with_client(
                api_key.clone(),
                settings.zai_api_base.clone(),
                ctx.http_client.clone(),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn missing_route_config_message(&self, settings: &AgentSettings) -> Option<&'static str> {
        settings
            .zai_api_key
            .as_ref()
            .is_none_or(|key| key.trim().is_empty())
            .then_some("Critical: ZAI_API_KEY is required for configured ZAI routes")
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, false)
    }

    fn capabilities_for_model(&self, model_info: &ModelInfo) -> ProviderCapabilities {
        let mut capabilities = self.capabilities();
        capabilities.supports_structured_output = zai_supports_structured_output(&model_info.id);
        capabilities
    }
}

fn zai_supports_structured_output(model_id: &str) -> bool {
    matches!(
        model_id.trim().to_ascii_lowercase().as_str(),
        "glm-4.7" | "glm-4" | "mainagent" | "glm-4.6" | "glm-4.5-air" | "glm-4-air" | "subagent"
    )
}
