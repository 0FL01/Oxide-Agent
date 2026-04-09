use super::types::{
    current_model_route, retrieval_fallback_classification, PreparedExecution,
    RunnerContextServices, TimedRunResult,
};
use super::{AgentExecutionOutcome, AgentExecutor};
use crate::agent::memory::AgentMessage;
use crate::agent::persistent_memory::{
    DurableMemoryRetrievalOptions, DurableMemoryRetriever, LlmMemoryEmbeddingGenerator,
    MemoryClassificationDecision,
};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_agent_system_prompt;
use crate::agent::providers::{
    inject_approval_credentials, SshApprovalGrant, SshApprovalRequestView,
    TopicInfraPreflightReport,
};
use crate::agent::runner::{AgentRunResult, AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use crate::agent::session::{AgentSession, RuntimeContextInbox, RuntimeContextInjection};
use crate::agent::skills::SkillRegistry;
use crate::agent::tool_bridge::{
    execute_single_tool_call, ToolExecutionContext, ToolExecutionResult,
};
use crate::agent::tool_runtime::scope_tool_model_route;
use crate::config::get_agent_max_iterations;
use crate::llm::{Message, ToolCall, ToolCallFunction};
use anyhow::{anyhow, Result};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

impl AgentExecutor {
    /// Inject safe topic infra preflight status into session memory once per change.
    pub fn set_topic_infra_preflight_status(
        &mut self,
        report: Option<&TopicInfraPreflightReport>,
        message: Option<String>,
    ) {
        if report.is_none() {
            self.last_topic_infra_preflight_summary = None;
            return;
        }

        let Some(message) = message else {
            return;
        };

        if self.last_topic_infra_preflight_summary.as_deref() == Some(message.as_str()) {
            return;
        }

        self.last_topic_infra_preflight_summary = Some(message.clone());
        self.inject_system_message(message);
    }

    /// Return pending SSH approvals that have not yet been surfaced to the transport.
    pub async fn take_pending_ssh_approvals(&self) -> Vec<SshApprovalRequestView> {
        match &self.topic_infra {
            Some(topic_infra) => topic_infra.approvals.take_unannounced().await,
            None => Vec::new(),
        }
    }

    /// Grant a pending SSH approval request and return the replay token.
    pub async fn grant_ssh_approval(&self, request_id: &str) -> Option<SshApprovalGrant> {
        let topic_infra = self.topic_infra.as_ref()?;
        topic_infra.approvals.grant(request_id).await
    }

    /// Reject a pending SSH approval request.
    pub async fn reject_ssh_approval(
        &mut self,
        request_id: &str,
    ) -> Option<SshApprovalRequestView> {
        let topic_infra = self.topic_infra.as_ref()?;
        let rejected = topic_infra.approvals.reject(request_id).await;
        if rejected.is_some() {
            let _ = self.session.take_pending_ssh_replay(request_id);
        }
        rejected
    }

    /// Inject transport-generated system context into the next run.
    pub fn inject_system_message(&mut self, content: String) {
        self.session
            .memory
            .add_message(AgentMessage::system_context(content));
    }

    /// Get a reference to the session
    #[must_use]
    pub const fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Get a mutable reference to the session
    pub const fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Disable loop detection for the next execution attempt.
    pub fn disable_loop_detection_next_run(&mut self) {
        self.runner.disable_loop_detection_next_run();
    }

    /// Whether manager control-plane tools are enabled for this executor.
    #[must_use]
    pub fn manager_control_plane_enabled(&self) -> bool {
        self.manager_control_plane.is_some()
    }

    /// Get the last task text, if available.
    #[must_use]
    pub fn last_task(&self) -> Option<&str> {
        self.session.last_task.as_deref()
    }

    /// Whether durable persistent-memory orchestration is configured.
    #[must_use]
    pub fn has_persistent_memory(&self) -> bool {
        self.persistent_memory.is_some()
            && self.memory_store.is_some()
            && self.memory_classifier.is_some()
    }

    /// Clone the runtime context inbox handle for concurrent transport writes.
    #[must_use]
    pub fn runtime_context_inbox(&self) -> RuntimeContextInbox {
        self.session.runtime_context_inbox()
    }

    /// Queue additional user context for the next safe iteration boundary.
    pub fn enqueue_runtime_context(&self, content: String) {
        self.session
            .push_runtime_context(RuntimeContextInjection { content });
    }

    /// Resume a paused task that is waiting for explicit user input.
    ///
    /// Returns `true` when a pending user-input request was consumed and the
    /// provided content was queued for the next execution attempt.
    #[must_use]
    pub fn resume_with_user_input(&mut self, content: String) -> bool {
        if self.session.pending_user_input().is_none() {
            return false;
        }

        self.session.clear_pending_user_input();
        self.enqueue_runtime_context(content);
        true
    }

    pub(super) async fn run_execution(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        append_user_message: bool,
        initial_tool_call: Option<ToolCall>,
        clear_pending_request_id: Option<&str>,
    ) -> Result<AgentExecutionOutcome> {
        if append_user_message {
            self.session.reset_memory_behavior_runtime();
        }
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        if append_user_message {
            self.session.remember_task(task);
        }
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

        if append_user_message {
            self.session
                .memory
                .add_message(AgentMessage::user_task(task));
        }

        let mut prepared = self.prepare_execution(task, progress_tx.as_ref()).await;
        Self::emit_milestone(progress_tx.as_ref(), "prepare_execution_done").await;

        if self
            .replay_initial_tool_call(
                initial_tool_call,
                clear_pending_request_id,
                &mut prepared,
                progress_tx.as_ref(),
            )
            .await?
        {
            self.session.complete();
            return Ok(AgentExecutionOutcome::WaitingForApproval);
        }

        let timeout_duration = self.agent_timeout_duration();
        let timeout_error_message = self.agent_timeout_error_message();

        let mut ctx = Self::prepare_runner_context(
            task,
            &task_id,
            &mut prepared,
            progress_tx.as_ref(),
            &mut self.session,
            self.skill_registry.as_mut(),
            RunnerContextServices {
                compaction_service: &self.compaction_service,
                persistent_memory: self.persistent_memory.as_ref(),
            },
        );

        match Self::run_with_outer_timeout(&mut self.runner, &mut ctx, timeout_duration).await {
            TimedRunResult::Final(res) => {
                self.session.complete();
                Ok(AgentExecutionOutcome::Completed(res))
            }
            TimedRunResult::WaitingForApproval => {
                self.session.complete();
                Ok(AgentExecutionOutcome::WaitingForApproval)
            }
            TimedRunResult::WaitingForUserInput(request) => {
                self.session.complete();
                self.session.set_pending_user_input(request.clone());
                Ok(AgentExecutionOutcome::WaitingForUserInput(request))
            }
            TimedRunResult::Failed(error) => {
                let error_message = error.to_string();
                if error_message.contains("cancelled") {
                    self.session.clear_todos();
                }
                self.session.fail(error_message);
                Err(error)
            }
            TimedRunResult::TimedOut => {
                self.session.timeout();
                Err(anyhow!(timeout_error_message))
            }
        }
    }

    pub(super) async fn prepare_execution(
        &mut self,
        task: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> PreparedExecution {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let registry = self.build_tool_registry(Arc::clone(&todos_arc), progress_tx);
        let tools = self
            .execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools());
        let model_routes = self.settings.get_configured_agent_model_routes();
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_agent_model());
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
        )
        .await;
        let mut messages =
            AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());
        let mut memory_classification = None;

        if let (Some(store), Some(classifier)) = (&self.memory_store, &self.memory_classifier) {
            let (classification, retrieval_classification) = match classifier.classify(task).await {
                Ok(decision) => (decision.clone(), decision),
                Err(error) => {
                    warn!(
                        error = %error,
                        task = %task,
                        "persistent-memory classifier failed; using conservative write mode and retrieval fallback"
                    );
                    (
                        MemoryClassificationDecision::conservative_safe_mode(),
                        retrieval_fallback_classification(),
                    )
                }
            };
            memory_classification = Some(classification.clone());

            let query_embeddings = self.runner.llm_client().is_embedding_available().then(|| {
                Arc::new(LlmMemoryEmbeddingGenerator::new(self.runner.llm_client()))
                    as Arc<dyn crate::agent::persistent_memory::MemoryEmbeddingGenerator>
            });
            let mut retriever = DurableMemoryRetriever::new_with_store(Arc::clone(store));
            if let Some(query_embeddings) = query_embeddings {
                retriever = retriever.with_query_embedding_generator(query_embeddings);
            }
            match retriever
                .render_prompt_context(
                    task,
                    &retrieval_classification,
                    self.session.memory_scope(),
                    DurableMemoryRetrievalOptions::default(),
                )
                .await
            {
                Ok(Some(context)) => messages.push(Message::system(&context)),
                Ok(None) => {}
                Err(error) => {
                    warn!(error = %error, task = %task, "durable memory retrieval failed")
                }
            }
        }

        PreparedExecution {
            todos_arc,
            registry,
            tools,
            system_prompt,
            messages,
            memory_classification,
            runner_config: AgentRunnerConfig::new(
                model.id.clone(),
                get_agent_max_iterations(),
                crate::config::AGENT_CONTINUATION_LIMIT,
                self.settings.get_agent_timeout_secs(),
                model.max_output_tokens,
            )
            .with_model_provider(model.provider.clone())
            .with_temperature(self.settings.get_configured_agent_temperature())
            .with_model_routes(model_routes),
        }
    }

    async fn replay_initial_tool_call(
        &mut self,
        initial_tool_call: Option<ToolCall>,
        clear_pending_request_id: Option<&str>,
        prepared: &mut PreparedExecution,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<bool> {
        let Some(tool_call) = initial_tool_call else {
            return Ok(false);
        };

        let cancellation_token = self.session.cancellation_token.clone();
        let active_route = current_model_route(&prepared.runner_config);
        let tool_result = {
            let mut tool_ctx = ToolExecutionContext {
                registry: &prepared.registry,
                progress_tx,
                todos_arc: &prepared.todos_arc,
                messages: &mut prepared.messages,
                agent: &mut self.session,
                cancellation_token,
            };
            let execution = execute_single_tool_call(tool_call, &mut tool_ctx);
            if let Some(route) = active_route {
                scope_tool_model_route(route, execution).await?
            } else {
                execution.await?
            }
        };

        if let Some(request_id) = clear_pending_request_id {
            let _ = self.session.take_pending_ssh_replay(request_id);
        }

        Ok(matches!(
            tool_result,
            ToolExecutionResult::WaitingForApproval { .. }
        ))
    }

    fn prepare_runner_context<'a>(
        task: &'a str,
        task_id: &'a str,
        prepared: &'a mut PreparedExecution,
        progress_tx: Option<&'a tokio::sync::mpsc::Sender<AgentEvent>>,
        session: &'a mut AgentSession,
        skill_registry: Option<&'a mut SkillRegistry>,
        services: RunnerContextServices<'a>,
    ) -> AgentRunnerContext<'a> {
        let session_id = Some(session.session_id.to_string());
        let memory_scope = Some(session.memory_scope().clone());
        let memory_behavior = Some(session.memory_behavior_runtime());
        AgentRunnerContext {
            task,
            system_prompt: &prepared.system_prompt,
            tools: &prepared.tools,
            registry: &prepared.registry,
            progress_tx,
            todos_arc: &prepared.todos_arc,
            task_id,
            messages: &mut prepared.messages,
            agent: session,
            skill_registry,
            compaction_service: Some(services.compaction_service),
            persistent_memory: services.persistent_memory,
            session_id,
            memory_scope,
            memory_behavior,
            memory_classification: prepared.memory_classification.clone(),
            config: prepared.runner_config.clone(),
        }
    }

    async fn run_with_outer_timeout(
        runner: &mut AgentRunner,
        ctx: &mut AgentRunnerContext<'_>,
        timeout_duration: Duration,
    ) -> TimedRunResult {
        match timeout(timeout_duration, runner.run(ctx)).await {
            Ok(Ok(AgentRunResult::Final(res))) => TimedRunResult::Final(res),
            Ok(Ok(AgentRunResult::WaitingForApproval)) => TimedRunResult::WaitingForApproval,
            Ok(Ok(AgentRunResult::WaitingForUserInput(request))) => {
                TimedRunResult::WaitingForUserInput(request)
            }
            Ok(Err(error)) => TimedRunResult::Failed(error),
            Err(_) => TimedRunResult::TimedOut,
        }
    }

    pub(super) async fn await_until_cancelled<T, F>(
        cancellation_token: tokio_util::sync::CancellationToken,
        future: F,
    ) -> Option<Result<T>>
    where
        F: Future<Output = Result<T>>,
    {
        tokio::pin!(future);

        tokio::select! {
            result = &mut future => Some(result),
            _ = cancellation_token.cancelled() => None,
        }
    }

    /// Execute a task with iterative tool calling (agentic loop)
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx, task), fields(session_id = %self.session.session_id))]
    pub async fn execute(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution(task, progress_tx, true, None, None)
            .await
    }

    /// Deterministically resume a paused SSH tool call after operator approval.
    pub async fn resume_ssh_approval(
        &mut self,
        request_id: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        let task = self
            .last_task()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("no saved task to resume"))?;
        let grant = self
            .grant_ssh_approval(request_id)
            .await
            .ok_or_else(|| anyhow!("SSH approval request not found or already handled"))?;
        let replay = self
            .session
            .pending_ssh_replay(request_id)
            .ok_or_else(|| anyhow!("pending SSH replay payload not found"))?;
        let arguments = inject_approval_credentials(
            &replay.arguments,
            &grant.request_id,
            &grant.approval_token,
        )?;
        let tool_call = ToolCall::new(
            replay.invocation_id.to_string(),
            ToolCallFunction {
                name: replay.tool_name,
                arguments,
            },
            false,
        );

        self.run_execution(&task, progress_tx, false, Some(tool_call), Some(request_id))
            .await
    }

    /// Resume a paused task after receiving the user input it requested.
    pub async fn resume_after_user_input(
        &mut self,
        content: String,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        let task = self
            .last_task()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("no saved task to resume"))?;

        if !self.resume_with_user_input(content) {
            return Err(anyhow!("session is not waiting for user input"));
        }

        self.run_execution(&task, progress_tx, false, None, None)
            .await
    }

    /// Continue the saved task after queuing additional runtime context.
    pub async fn continue_after_runtime_context(
        &mut self,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        let task = self
            .last_task()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("no saved task to continue"))?;

        if !self.session.has_pending_runtime_context() {
            return Err(anyhow!("session has no queued runtime context"));
        }

        self.run_execution(&task, progress_tx, false, None, None)
            .await
    }
}
