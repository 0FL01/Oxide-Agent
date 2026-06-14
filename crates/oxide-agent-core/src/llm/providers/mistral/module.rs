use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{MediaCapabilities, ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::providers::openai_base::{OpenAIBaseProvider, OpenAICompatibleProfile};

/// Capability module for Mistral routes.
///
/// Delegates to [`OpenAIBaseProvider`] with a Mistral-specific profile
/// that encodes all Mistral behavioral quirks (tool-call ID mapping,
/// strict history, temperatures, reasoning, audio transcription).
pub(crate) struct MistralProviderModule;

const API_KEY_CONFIG_KEY: &str = "api_key";
const API_KEY_ENV: &str = "MISTRAL_API_KEY";
const MISTRAL_API_BASE: &str = "https://api.mistral.ai/v1";

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
        settings
            .module_string_value_or_env(self.provider_id(), API_KEY_CONFIG_KEY, API_KEY_ENV)
            .map(|api_key| {
                Arc::new(OpenAIBaseProvider::new_with_client_and_profile(
                    Some(api_key),
                    MISTRAL_API_BASE.to_string(),
                    ctx.http_client.clone(),
                    OpenAICompatibleProfile::mistral(),
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
