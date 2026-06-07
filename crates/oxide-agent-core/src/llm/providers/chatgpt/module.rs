use std::path::PathBuf;
use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::LlmProvider;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};

/// Capability module for ChatGPT/Codex OAuth routes.
pub(crate) struct ChatGptProviderModule;

const AUTH_PATH_CONFIG_KEY: &str = "auth_path";
const AUTH_PATH_ENV: &str = "CHATGPT_AUTH_PATH";

impl LlmProviderModule for ChatGptProviderModule {
    fn provider_id(&self) -> &'static str {
        "llm-provider/openai-chatgpt"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["chatgpt", "openai-chatgpt"]
    }

    fn build_provider(
        &self,
        settings: &AgentSettings,
        ctx: &LlmProviderBuildContext,
    ) -> Option<Arc<dyn LlmProvider>> {
        let auth_path = settings.module_string_value_or_env(
            self.provider_id(),
            AUTH_PATH_CONFIG_KEY,
            AUTH_PATH_ENV,
        )?;
        let resolved_auth_path = super::resolve_auth_file_path(Some(auth_path.as_str()))
            .unwrap_or_else(|_| PathBuf::from(auth_path));

        if !resolved_auth_path.exists() {
            return None;
        }

        Some(Arc::new(super::ChatGptProvider::new_with_client(
            resolved_auth_path,
            ctx.http_client.clone(),
        )))
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::new(ToolHistoryMode::BestEffort, true, false)
    }
}
