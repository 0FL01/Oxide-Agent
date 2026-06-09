use std::sync::Arc;

use crate::config::{AgentSettings, ModelInfo};
use crate::llm::LlmProvider;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for OpenRouter routes.
pub(crate) struct OpenRouterProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "OPENROUTER_API_KEY";

impl LlmProviderModule for OpenRouterProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/openrouter"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["openrouter"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                Arc::new(super::OpenRouterProvider::new_with_client(
                    api_key,
                    ctx.http_client.clone(),
                )) as Arc<dyn LlmProvider>
            })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, false, false)
    }

    fn capabilities_for_model(&self, model_info: &ModelInfo) -> ProviderCapabilities {
        let Some(policy) = openrouter_model_policy(&model_info.id) else {
            return self.capabilities();
        };

        ProviderCapabilities::new(
            ToolHistoryMode::BestEffort,
            policy.approved_for_main_agent && policy.supports_tools_parameter,
            policy.supports_structured_outputs,
        )
    }

    fn media_capabilities(&self) -> MediaCapabilities {
        MediaCapabilities::new(false, false, false)
    }

    fn media_capabilities_for_model(&self, model_info: &ModelInfo) -> MediaCapabilities {
        let Some(policy) = openrouter_model_policy(&model_info.id) else {
            return self.media_capabilities();
        };

        MediaCapabilities::new(
            policy.approved_for_media_audio && policy.input_audio,
            policy.approved_for_media_image && policy.input_image,
            policy.approved_for_media_video && policy.input_video,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct OpenRouterModelPolicy {
    supports_tools_parameter: bool,
    supports_structured_outputs: bool,
    input_audio: bool,
    input_image: bool,
    input_video: bool,
    approved_for_main_agent: bool,
    approved_for_media_audio: bool,
    approved_for_media_image: bool,
    approved_for_media_video: bool,
}

fn openrouter_model_policy(model_id: &str) -> Option<OpenRouterModelPolicy> {
    let normalized = model_id.trim().to_ascii_lowercase();
    if normalized.starts_with("google/gemini-2") || normalized.starts_with("google/gemini-3") {
        return Some(OpenRouterModelPolicy {
            supports_tools_parameter: true,
            supports_structured_outputs: true,
            input_audio: true,
            input_image: true,
            input_video: true,
            approved_for_main_agent: false,
            approved_for_media_audio: true,
            approved_for_media_image: true,
            approved_for_media_video: true,
        });
    }

    match normalized.as_str() {
        "deepseek/deepseek-v4-flash" | "deepseek/deepseek-v4-pro" => Some(OpenRouterModelPolicy {
            supports_tools_parameter: true,
            supports_structured_outputs: true,
            input_audio: false,
            input_image: false,
            input_video: false,
            approved_for_main_agent: true,
            approved_for_media_audio: false,
            approved_for_media_image: false,
            approved_for_media_video: false,
        }),
        _ => None,
    }
}
