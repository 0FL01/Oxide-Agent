use crate::bot::agent_handlers::AgentTaskRuntime;
use crate::config::BotSettings;
use oxide_agent_core::agent::TaskId;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::TaskEventBroadcaster;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

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
    /// Shared runtime task event broadcaster.
    pub task_events: Arc<TaskEventBroadcaster>,
    /// Active task watcher guard to avoid duplicate subscriptions.
    pub task_watchers: Arc<Mutex<HashSet<TaskId>>>,
}
