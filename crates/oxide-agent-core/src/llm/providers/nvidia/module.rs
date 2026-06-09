use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for Nvidia hosted model routes.
pub(crate) struct NvidiaProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "NVIDIA_API_KEY";
const API_BASE_CONFIG_KEY: &str = "api_base";
const API_BASE_ENV: &str = "NVIDIA_API_BASE";
const DEFAULT_API_BASE: &str = "https://integrate.api.nvidia.com/v1";

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
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                let api_base = settings.module_string_value_or_env_or_default(
                    self.provider_id(),
                    API_BASE_CONFIG_KEY,
                    API_BASE_ENV,
                    DEFAULT_API_BASE,
                );
                Arc::new(super::NvidiaProvider::new_with_client(
                    api_key,
                    api_base,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, false)
    }

    fn capabilities_for_model(&self, model_info: &ModelInfo) -> ProviderCapabilities {
        let mut capabilities = self.capabilities();
        let model_capabilities = super::model_capabilities(&model_info.id);
        capabilities.supports_tool_calling = model_capabilities.supports_tool_calling;
        capabilities.supports_structured_output = model_capabilities.supports_structured_output;
        capabilities
    }
}
