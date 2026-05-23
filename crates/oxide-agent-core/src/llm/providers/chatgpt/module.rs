use std::path::PathBuf;
use std::sync::Arc;

use crate::config::AgentSettings;
use crate::llm::capabilities::{ProviderCapabilities, ToolHistoryMode};
use crate::llm::providers::modules::{LlmProviderBuildContext, LlmProviderModule};
use crate::llm::LlmProvider;

/// Capability module for ChatGPT/Codex OAuth routes.
pub(crate) struct ChatGptProviderModule;

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
        let auth_path = settings
            .chatgpt_auth_path
            .as_ref()
            .filter(|path| !path.trim().is_empty())?;
        let resolved_auth_path = super::resolve_auth_file_path(Some(auth_path))
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
