use crate::agent::compaction::CompactionService;
use crate::agent::persistent_memory::{MemoryClassificationDecision, PersistentMemoryCoordinator};
use crate::agent::providers::{ManagerTopicLifecycle, SshApprovalRegistry, TodoList};
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::AgentRunnerConfig;
use crate::agent::session::PendingUserInput;
use crate::llm::{Message, ToolCall, ToolDefinition};
use crate::storage::{StorageProvider, TopicInfraConfigRecord};
use anyhow::Error;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub(super) struct AgentsMdContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_id: String,
}

#[derive(Clone)]
pub(super) struct ManagerControlPlaneContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

#[derive(Clone)]
pub(super) struct TopicInfraContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_id: String,
    pub(super) config: TopicInfraConfigRecord,
    pub(super) approvals: SshApprovalRegistry,
}

pub(super) struct PreparedExecution {
    pub(super) todos_arc: Arc<Mutex<TodoList>>,
    pub(super) registry: ToolRegistry,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) system_prompt: String,
    pub(super) messages: Vec<Message>,
    pub(super) memory_classification: Option<MemoryClassificationDecision>,
    pub(super) runner_config: AgentRunnerConfig,
}

pub(super) struct RunnerContextServices<'a> {
    pub(super) compaction_service: &'a CompactionService,
    pub(super) persistent_memory: Option<&'a PersistentMemoryCoordinator>,
}

pub(super) enum ExecutionRequest {
    NewTask { task: String },
    ResumeApproval { request_id: String },
    ResumeUserInput { content: String },
    ContinueRuntimeContext,
}

pub(super) struct ResolvedExecutionRequest {
    pub(super) task: String,
    pub(super) append_user_message: bool,
    pub(super) initial_tool_call: Option<ToolCall>,
    pub(super) clear_pending_request_id: Option<String>,
}

pub(super) enum ExecutionTransition {
    Completed(String),
    WaitingForApproval,
    WaitingForUserInput(PendingUserInput),
    Failed(Error),
    TimedOut,
}

pub(super) fn current_model_route(config: &AgentRunnerConfig) -> Option<crate::config::ModelInfo> {
    if let Some(route) = config.model_routes.iter().find(|route| {
        route.id == config.model_name
            && config
                .model_provider
                .as_deref()
                .is_none_or(|provider| route.provider == provider)
    }) {
        return Some(route.clone());
    }

    let provider = config.model_provider.clone()?;
    Some(crate::config::ModelInfo {
        id: config.model_name.clone(),
        provider,
        max_output_tokens: config.model_max_output_tokens,
        context_window_tokens: 0,
        weight: 1,
    })
}

pub(super) fn retrieval_fallback_classification() -> MemoryClassificationDecision {
    let mut decision = MemoryClassificationDecision::conservative_safe_mode();
    decision.read_policy.inject_prompt_memory = true;
    decision.read_policy.search_episodes = true;
    decision.read_policy.search_memories = true;
    decision.read_policy.allow_vector_only_memory = false;
    decision.read_policy.min_importance = 0.8;
    decision.read_policy.top_k = 3;
    decision.read_policy.allow_full_thread_read = false;
    decision
}

pub(super) enum TimedRunResult {
    Final(String),
    WaitingForApproval,
    WaitingForUserInput(PendingUserInput),
    Failed(Error),
    TimedOut,
}
