use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for OpenCode Go routes.
pub(crate) struct OpenCodeGoProviderModule;

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
        settings.opencode_go_api_key.as_ref().map(|api_key| {
            Arc::new(super::OpenCodeGoProvider::new_with_client(
                api_key.clone(),
                settings.opencode_go_api_base.clone(),
                ctx.http_client.clone(),
            )) as Arc<dyn LlmProvider>
        })
    }

    fn missing_route_config_message(&self, settings: &AgentSettings) -> Option<&'static str> {
        settings
            .opencode_go_api_key
            .as_ref()
            .is_none_or(|key| key.trim().is_empty())
            .then_some(
                "Critical: OPENCODE_GO_API_KEY is required for configured OpenCode Go routes",
            )
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::Strict, true, true)
    }

    fn capabilities_for_model(&self, model_info: &ModelInfo) -> ProviderCapabilities {
        let mut capabilities = self.capabilities();
        capabilities.supports_structured_output =
            opencode_go_supports_structured_output(&model_info.id);
        capabilities
    }
}

fn normalize_opencode_go_model_id(model_id: &str) -> String {
    let trimmed = model_id.trim();
    trimmed
        .strip_prefix("opencode-go/")
        .unwrap_or(trimmed)
        .to_string()
}

fn opencode_go_supports_structured_output(model_id: &str) -> bool {
    matches!(
        normalize_opencode_go_model_id(model_id).as_str(),
        "deepseek-v4-flash" | "deepseek-v4-pro"
    )
}
