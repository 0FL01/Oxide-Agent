use crate::agent::compaction::CompactionService;
use crate::agent::persistent_memory::{MemoryClassificationDecision, PersistentMemoryCoordinator};
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{ManagerTopicLifecycle, SshApprovalRegistry, TodoList};
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::{
    AgentRunnerConfig, AgentRunnerContext, AgentRunnerContextBase, TimedRunResult,
};
use crate::agent::session::{AgentSession, PendingUserInput};
use crate::agent::skills::SkillRegistry;
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

impl PreparedExecution {
    pub(super) fn build_runner_context<'a>(
        &'a mut self,
        task: &'a str,
        task_id: &'a str,
        progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
        session: &'a mut AgentSession,
        skill_registry: Option<&'a mut SkillRegistry>,
        services: RunnerContextServices<'a>,
    ) -> AgentRunnerContext<'a> {
        let session_id = Some(session.session_id.to_string());
        let memory_scope = Some(session.memory_scope().clone());
        let memory_behavior = Some(session.memory_behavior_runtime());
        let mut ctx = AgentRunnerContext::new_base(
            AgentRunnerContextBase {
                task,
                system_prompt: &self.system_prompt,
                tools: &self.tools,
                registry: &self.registry,
                progress_tx,
                todos_arc: &self.todos_arc,
                task_id,
                messages: &mut self.messages,
                agent: session,
            },
            Some(services.compaction_service),
            self.runner_config.clone(),
        );

        ctx.skill_registry = skill_registry;
        ctx.persistent_memory = services.persistent_memory;
        ctx.session_id = session_id;
        ctx.memory_scope = memory_scope;
        ctx.memory_behavior = memory_behavior;
        ctx.memory_classification = self.memory_classification.clone();

        ctx
    }
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

impl From<TimedRunResult> for ExecutionTransition {
    fn from(result: TimedRunResult) -> Self {
        match result {
            TimedRunResult::Final(res) => Self::Completed(res),
            TimedRunResult::WaitingForApproval => Self::WaitingForApproval,
            TimedRunResult::WaitingForUserInput(request) => Self::WaitingForUserInput(request),
            TimedRunResult::Failed(error) => Self::Failed(error),
            TimedRunResult::TimedOut => Self::TimedOut,
        }
    }
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
