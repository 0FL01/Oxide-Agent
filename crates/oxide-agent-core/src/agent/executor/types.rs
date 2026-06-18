use crate::agent::compaction::CompactionController;
use crate::agent::progress::AgentEvent;
use crate::agent::providers::{ManagerTopicLifecycle, TodoList};
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

use super::{AgentExecutionOptions, AgentUserInput};

#[derive(Clone)]
#[cfg_attr(not(feature = "tool-agents-md"), allow(dead_code))]
pub(super) struct AgentsMdContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_id: String,
}

#[derive(Clone)]
#[cfg_attr(not(feature = "manager-control-plane"), allow(dead_code))]
pub(super) struct ManagerControlPlaneContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

#[derive(Clone)]
#[cfg_attr(not(feature = "integration-ssh-mcp"), allow(dead_code))]
pub(super) struct TopicInfraContext {
    pub(super) storage: Arc<dyn StorageProvider>,
    pub(super) user_id: i64,
    pub(super) topic_id: String,
    pub(super) config: TopicInfraConfigRecord,
}

pub(super) struct PreparedExecution {
    pub(super) todos_arc: Arc<Mutex<TodoList>>,
    pub(super) tool_runtime_registry: Arc<RuntimeToolRegistry>,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) system_prompt: String,
    pub(super) date_suffix: String,
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
        storage: Option<Arc<dyn crate::storage::StorageProvider>>,
    ) -> AgentRunnerContext<'a> {
        let session_id = Some(session.session_id.to_string());
        let memory_scope = Some(session.memory_scope().clone());
        let memory_behavior = Some(session.memory_behavior_runtime());
        let mut ctx = AgentRunnerContext::new_base(
            AgentRunnerContextBase {
                task,
                system_prompt: &self.system_prompt,
                date_suffix: &self.date_suffix,
                tools: &self.tools,
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
        ctx.storage = storage;
        ctx.tool_runtime_registry = Some(Arc::clone(&self.tool_runtime_registry));

        ctx
    }
}

pub(super) enum ExecutionRequest {
    NewTask {
        input: AgentUserInput,
        options: AgentExecutionOptions,
    },
    ResumeUserInput {
        input: AgentUserInput,
        options: AgentExecutionOptions,
    },
    ContinueRuntimeContext,
}

pub(super) struct ResolvedExecutionRequest {
    pub(super) task: String,
    pub(super) user_input: Option<AgentUserInput>,
    pub(super) options: AgentExecutionOptions,
}

pub(super) enum ExecutionTransition {
    Completed(String),
    WaitingForUserInput(PendingUserInput),
    Failed(Error),
    TimedOut,
}

impl From<TimedRunResult> for ExecutionTransition {
    fn from(result: TimedRunResult) -> Self {
        match result {
            TimedRunResult::Final(res) => Self::Completed(res),
            TimedRunResult::WaitingForUserInput(request) => Self::WaitingForUserInput(request),
            TimedRunResult::Failed(error) => Self::Failed(error),
            TimedRunResult::TimedOut => Self::TimedOut,
        }
    }
}
