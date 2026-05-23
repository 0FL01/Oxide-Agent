use crate::agent::compaction::CompactionController;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{ManagerTopicLifecycle, SshApprovalRegistry, TodoList};
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::{
    AgentRunnerConfig, AgentRunnerContext, AgentRunnerContextBase, TimedRunResult,
};
use crate::agent::session::{AgentSession, PendingUserInput};
use crate::agent::tool_runtime::ToolRegistry as RuntimeToolRegistry;
use crate::llm::{Message, ToolDefinition};
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
    pub(super) tool_runtime_registry: Option<Arc<RuntimeToolRegistry>>,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) system_prompt: String,
    pub(super) messages: Vec<Message>,
    pub(super) runner_config: AgentRunnerConfig,
}

pub(super) struct RunnerContextServices<'a> {
    pub(super) compaction_controller: &'a CompactionController,
}

impl PreparedExecution {
    pub(super) fn build_runner_context<'a>(
        &'a mut self,
        task: &'a str,
        task_id: &'a str,
        progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
        session: &'a mut AgentSession,
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
            Some(services.compaction_controller),
            self.runner_config.clone(),
        );

        ctx.session_id = session_id;
        ctx.memory_scope = memory_scope;
        ctx.memory_behavior = memory_behavior;
        ctx.tool_runtime_registry = self.tool_runtime_registry.as_ref().map(Arc::clone);

        ctx
    }
}

pub(super) enum ExecutionRequest {
    NewTask { task: String },
    ResumeUserInput { content: String },
    ContinueRuntimeContext,
}

pub(super) struct ResolvedExecutionRequest {
    pub(super) task: String,
    pub(super) append_user_message: bool,
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
