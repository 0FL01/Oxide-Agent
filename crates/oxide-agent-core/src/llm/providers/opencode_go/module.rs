use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for OpenCode Go routes.
pub(crate) struct OpenCodeGoProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "OPENCODE_GO_API_KEY";
const API_BASE_CONFIG_KEY: &str = "api_base";
const API_BASE_ENV: &str = "OPENCODE_GO_API_BASE";
const DEFAULT_API_BASE: &str = "https://opencode.ai/zen/go/v1/chat/completions";

impl LlmProviderModule for OpenCodeGoProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/opencode-go"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["opencode-go", "opencode_go"]
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
                Arc::new(super::OpenCodeGoProvider::new_with_client(
                    api_key,
                    api_base,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn missing_route_config_message(&self, settings: &AgentSettings) -> Option<&'static str> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .is_none()
            .then_some(
                "Critical: OPENCODE_GO_API_KEY is required for configured OpenCode Go routes",
            )
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
    }

    fn capabilities_for_model(&self, _model_info: &ModelInfo) -> ProviderCapabilities {
        // All opencode-go models share a unified tool and structured-output protocol.
        self.capabilities()
    }
}
