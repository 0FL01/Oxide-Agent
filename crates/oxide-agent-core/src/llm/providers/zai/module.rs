use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for ZAI/Zhipu routes.
pub(crate) struct ZaiProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "ZAI_API_KEY";
const API_BASE_CONFIG_KEY: &str = "api_base";
const API_BASE_ENV: &str = "ZAI_API_BASE";
const DEFAULT_API_BASE: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";

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
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                let api_base = settings.module_string_value_or_env_or_default(
                    self.provider_id(),
                    API_BASE_CONFIG_KEY,
                    API_BASE_ENV,
                    DEFAULT_API_BASE,
                );
                Arc::new(super::ZaiProvider::new_with_client(
                    api_key,
                    api_base,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn missing_route_config_message(
        &self,
        _provider_name: &str,
        settings: &AgentSettings,
    ) -> Option<String> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .is_none()
            .then(|| "Critical: ZAI_API_KEY is required for configured ZAI routes".to_string())
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
