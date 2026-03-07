use crate::bot::agent_handlers::AgentTaskRuntime;
use crate::config::BotSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::StorageProvider;
use std::sync::Arc;

/// Shared Telegram handler dependencies for the runtime-owned agent flow.
#[derive(Clone)]
pub struct TelegramHandlerContext {
    /// Shared storage backend.
    pub storage: Arc<dyn StorageProvider>,
    /// Shared LLM client.
    pub llm: Arc<LlmClient>,
    /// Shared transport settings.
    pub settings: Arc<BotSettings>,
    /// Shared runtime-owned task orchestration.
    pub task_runtime: Arc<AgentTaskRuntime>,
}
