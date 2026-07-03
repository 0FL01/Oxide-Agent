use super::types::{
    ExecutionRequest, ExecutionTransition, PreparedExecution, ResolvedExecutionRequest,
    RunnerContextServices,
};
use super::{
    AgentExecutionEffort, AgentExecutionOptions, AgentExecutionOutcome, AgentExecutor,
    AgentUserInput,
};
use crate::agent::compaction::{
    AdmissionBudget, AdmissionDecision, ContextAdmission, PayloadDescriptor, PayloadKind,
};
use crate::agent::memory::AgentMessage;
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::{PromptToolContext, create_agent_system_prompt};
use crate::agent::providers::TopicInfraPreflightReport;
use crate::agent::runner::{AgentRunner, AgentRunnerConfig, run_with_timeout};
use crate::agent::session::{AgentSession, RuntimeContextInbox, RuntimeContextInjection};
use crate::config::{
    get_agent_continuation_limit, get_agent_max_iterations, get_agent_search_limit,
};
use crate::llm::ToolDefinition;
use anyhow::{Result, anyhow};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{debug, info};

const AGENT_LATENCY_TARGET: &str = "oxide_agent_core::agent_latency";

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

    /// Clone the runtime context inbox handle for concurrent transport writes.
    #[must_use]
    pub fn runtime_context_inbox(&self) -> RuntimeContextInbox {
        self.session.runtime_context_inbox()
    }

    /// Queue additional user context for the next safe iteration boundary.
    pub fn enqueue_runtime_context(&self, content: String) {
        self.enqueue_runtime_user_input(AgentUserInput::new(content));
    }

    /// Queue additional user input, including safe attachment refs, for the next safe boundary.
    pub fn enqueue_runtime_user_input(&self, input: AgentUserInput) {
        self.session
            .push_runtime_context(runtime_context_from_user_input(input));
    }

    /// Resume a paused task that is waiting for explicit user input.
    ///
    /// Returns `true` when a pending user-input request was consumed and the
    /// provided content was queued for the next execution attempt.
    #[must_use]
    pub fn resume_with_user_input(&mut self, content: String) -> bool {
        self.resume_with_agent_user_input(AgentUserInput::new(content))
    }

    /// Resume a paused task with structured user input and safe attachment refs.
    #[must_use]
    pub fn resume_with_agent_user_input(&mut self, input: AgentUserInput) -> bool {
        if self.session.pending_user_input().is_none() {
            return false;
        }

        self.session.clear_pending_user_input();
        self.enqueue_runtime_user_input(input);
        true
    }

    pub(super) async fn run_execution(
        &mut self,
        request: ResolvedExecutionRequest,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<ExecutionTransition> {
        let ResolvedExecutionRequest {
            task,
            user_input,
            options,
        } = request;
        let task_id = self.prime_session_for_execution(&task, user_input.as_ref());
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

        let prepare_started_at = Instant::now();
        let mut prepared = self
            .prepare_execution(&task, progress_tx.as_ref(), options)
            .await;
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id = %task_id,
            model = %prepared.runner_config.model_name,
            provider = ?prepared.runner_config.model_provider,
            tool_count = prepared.tools.len(),
            message_count = prepared.messages.len(),
            prepare_ms = prepare_started_at.elapsed().as_millis(),
            "Agent execution prepared"
        );
        Self::emit_milestone(progress_tx.as_ref(), "prepare_execution_done").await;

        let timeout_duration = self.agent_timeout_duration(options);

        let mut ctx = prepared.build_runner_context(
            &task,
            &task_id,
            progress_tx.as_ref(),
            &mut self.session,
            RunnerContextServices {
                compaction_controller: &self.compaction_controller,
            },
            self.storage.clone(),
        );
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id = %task_id,
            timeout_secs = timeout_duration.as_secs(),
            "Dispatching agent runner"
        );

        let outcome = run_with_timeout(&mut self.runner, &mut ctx, timeout_duration).await;

        // RAII cleanup: close any browser sessions the agent left open.
        // Runs on every outcome (success, timeout, cancel, error) to prevent
        // Chromium process leaks at the sidecar.
        if let Some(cleanup) = &prepared.browser_cleanup {
            cleanup.close_all_sessions().await;
        }

        Ok(outcome.into())
    }

    fn apply_execution_transition(
        &mut self,
        transition: ExecutionTransition,
        timeout_error_message: &str,
    ) -> Result<AgentExecutionOutcome> {
        match transition {
            ExecutionTransition::Completed(response) => {
                self.session.complete();
                Ok(AgentExecutionOutcome::Completed(response))
            }
            ExecutionTransition::WaitingForUserInput(request) => {
                self.session.complete();
                self.session.set_pending_user_input(request.clone());
                Ok(AgentExecutionOutcome::WaitingForUserInput(request))
            }
            ExecutionTransition::Failed(error) => {
                let error_message = error.to_string();
                if error_message.contains("cancelled") {
                    self.session.clear_todos();
                }
                self.session.fail(error_message);
                Err(error)
            }
            ExecutionTransition::TimedOut => {
                self.session.timeout();
                Err(anyhow!(timeout_error_message.to_string()))
            }
        }
    }

    fn prime_session_for_execution(
        &mut self,
        task: &str,
        user_input: Option<&AgentUserInput>,
    ) -> String {
        if user_input.is_some() {
            self.session.clear_todos();
        }
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        if let Some(user_input) = user_input {
            self.session.remember_task(task);
            let user_text = user_input.text_projection().to_string();

            // Admission gate: evaluate user task before hot-memory mutation.
            // At executor time we don't have route/system-prompt/tool-schema info,
            // so we use memory.max_tokens() as the context window estimate with
            // zero overhead — conservative (overestimates available space).
            // The pre-LLM budget trigger re-checks with accurate numbers before
            // the first LLM call.
            let budget = AdmissionBudget {
                rendered_tokens: self.session.memory.rendered_token_count(),
                route_context_window: self.session.memory.max_tokens(),
                system_prompt_tokens: 0,
                tool_schema_tokens: 0,
                hard_reserve: 8_192,
            };
            let descriptor = PayloadDescriptor {
                kind: PayloadKind::NewTask,
                content: user_text.clone(),
                source: None,
                size_bytes: user_text.len(),
            };
            let user_message = match ContextAdmission::evaluate(&descriptor, &budget) {
                AdmissionDecision::Inline => AgentMessage::user_task(user_text)
                    .with_user_attachments(user_input.attachments.clone()),
                AdmissionDecision::Manifest(spec) => {
                    let mut msg = AgentMessage::user_task(spec.manifest_content.clone())
                        .with_user_attachments(user_input.attachments.clone());
                    msg.externalized_payload = Some(spec.externalized_payload);
                    msg
                }
                AdmissionDecision::ControlledPause(blocker) => {
                    let placeholder = format!(
                        "[User task withheld — context budget exceeded]\n{}",
                        blocker.reason()
                    );
                    AgentMessage::user_task(placeholder)
                }
            };
            if let Some(context) = self
                .session
                .memory
                .soft_temporal_boundary_before_user_task(&user_message)
            {
                self.session
                    .memory
                    .add_message(AgentMessage::system_context(context));
            }
            self.session.memory.add_message(user_message);
        }
        task_id
    }

    fn saved_task(&self, missing_task_error: &'static str) -> Result<String> {
        self.last_task()
            .map(str::to_string)
            .ok_or_else(|| anyhow!(missing_task_error))
    }

    async fn resolve_execution_request(
        &mut self,
        request: ExecutionRequest,
    ) -> Result<ResolvedExecutionRequest> {
        match request {
            ExecutionRequest::NewTask { input, options } => {
                let task = input.content.clone();
                Ok(ResolvedExecutionRequest {
                    task,
                    user_input: Some(input),
                    options,
                })
            }
            ExecutionRequest::ResumeUserInput { input, options } => {
                let task = self.saved_task("no saved task to resume")?;
                if !self.resume_with_agent_user_input(input) {
                    return Err(anyhow!("session is not waiting for user input"));
                }

                Ok(ResolvedExecutionRequest {
                    task,
                    user_input: None,
                    options,
                })
            }
            ExecutionRequest::ContinueRuntimeContext => {
                let task = self.saved_task("no saved task to continue")?;
                if !self.session.has_pending_runtime_context() {
                    return Err(anyhow!("session has no queued runtime context"));
                }

                Ok(ResolvedExecutionRequest {
                    task,
                    user_input: None,
                    options: AgentExecutionOptions::default(),
                })
            }
        }
    }

    async fn run_execution_request(
        &mut self,
        request: ExecutionRequest,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        let request = self.resolve_execution_request(request).await?;
        let timeout_error_message = self.agent_timeout_error_message(request.options);
        let transition = self.run_execution(request, progress_tx).await?;
        let outcome =
            self.apply_execution_transition(transition, timeout_error_message.as_str())?;
        Ok(outcome)
    }

    pub(super) async fn prepare_execution(
        &mut self,
        task: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> PreparedExecution {
        let prepare_started_at = Instant::now();
        let mut phase_started_at = prepare_started_at;
        let task_id = self
            .session
            .current_task_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            phase = "prepare_started",
            elapsed_ms = 0_u128,
            "Agent prepare execution latency"
        );

        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            phase = "todos_snapshot_created",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let model_routes = self
            .model_routes_override
            .clone()
            .unwrap_or_else(|| self.settings.get_configured_agent_model_routes());
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_agent_model());
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            model = %model.id,
            provider = ?model.provider,
            route_count = model_routes.len(),
            phase = "model_routes_resolved",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let tool_build =
            self.build_tool_runtime_registry_with_cleanup(Arc::clone(&todos_arc), progress_tx);
        let tool_runtime_registry = Arc::new(tool_build.registry);
        let tool_surface_handle = tool_build.surface_handle;
        let tool_catalog = tool_build.catalog;
        let browser_cleanup = tool_build.browser_cleanup;
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            phase = "tool_runtime_registry_built",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        // Compute the initial model-visible tool surface (bootstrap only).
        // Deferred tools are hidden until the model activates their group via
        // `retrieve_tools`.  The surface is refreshed each iteration by the runner.
        //
        // Profiles with `pre_activate_all_tools` (e.g. search probe) bypass
        // the lazy surface — all deferred groups are activated at startup so
        // the model sees the full catalog from turn 1.  This is appropriate
        // for specialized agents with a tiny tool set where the lazy
        // protocol adds unnecessary overhead.
        if self.execution_profile.pre_activate_all_tools() {
            for group in tool_catalog.activatable_groups() {
                let _ = tool_surface_handle.activate_group(group);
            }
        }
        let tools = tool_surface_handle.visible_specs(&tool_catalog);
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            visible_tool_count = tools.len(),
            catalog_tool_count = tool_catalog.len(),
            phase = "tool_specs_collected",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            structured_output,
            phase = "structured_output_resolved",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let prompt_instructions =
            effort_prompt_instructions(self.execution_profile.prompt_instructions(), options);
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            prompt_instructions_chars = prompt_instructions.as_ref().map_or(0, String::len),
            phase = "prompt_instructions_resolved",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        // Build prompt tool context from the full catalog: workflow hints and
        // date context reflect all compiled tools, while the category list
        // block tells the model which groups it can retrieve.
        let catalog_specs: Vec<ToolDefinition> =
            tool_catalog.entries().map(|e| e.spec.clone()).collect();
        let available_groups = tool_catalog.activatable_groups();
        let tool_ctx = PromptToolContext::new(&catalog_specs, &available_groups);

        let system_prompt = create_agent_system_prompt(
            task,
            tool_ctx,
            structured_output,
            &mut self.session,
            prompt_instructions.as_deref(),
        )
        .await;
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            system_prompt_chars = system_prompt.base.len(),
            date_suffix_chars = system_prompt.date_suffix.len(),
            structured_output,
            phase = "system_prompt_assembled",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let messages = AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            source_message_count = self.session.memory.get_messages().len(),
            message_count = messages.len(),
            phase = "memory_messages_converted",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );
        phase_started_at = Instant::now();

        let max_iterations = options
            .min_max_iterations()
            .map_or_else(get_agent_max_iterations, |minimum| {
                get_agent_max_iterations().max(minimum)
            });
        let continuation_limit = options
            .min_continuation_limit()
            .map_or_else(get_agent_continuation_limit, |minimum| {
                get_agent_continuation_limit().max(minimum)
            });
        let timeout_secs = options.timeout_secs.unwrap_or_else(|| {
            options.min_timeout_secs().map_or_else(
                || self.settings.get_agent_timeout_secs(),
                |minimum| self.settings.get_agent_timeout_secs().max(minimum),
            )
        });
        let search_limit = options.search_limit.unwrap_or_else(|| {
            options
                .min_search_limit()
                .map_or_else(get_agent_search_limit, |minimum| {
                    get_agent_search_limit().max(minimum)
                })
        });
        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            max_iterations,
            continuation_limit,
            timeout_secs,
            search_limit,
            phase = "runner_limits_resolved",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );

        debug!(
            target: AGENT_LATENCY_TARGET,
            task_id,
            model = %model.id,
            provider = ?model.provider,
            tool_count = tools.len(),
            message_count = messages.len(),
            phase = "prepare_assembled",
            elapsed_ms = prepare_started_at.elapsed().as_millis(),
            "Agent prepare execution latency"
        );

        PreparedExecution {
            todos_arc,
            tool_runtime_registry,
            tools,
            tool_catalog,
            tool_surface_handle,
            system_prompt: system_prompt.base,
            date_suffix: system_prompt.date_suffix,
            messages,
            runner_config: AgentRunnerConfig::new(
                model.id.clone(),
                max_iterations,
                continuation_limit,
                timeout_secs,
                model.max_output_tokens,
            )
            .with_model_provider(model.provider.clone())
            .with_temperature(self.settings.get_configured_agent_temperature())
            .with_model_routes(model_routes)
            .with_search_limit(search_limit)
            .with_reasoning_effort(options.reasoning_effort()),
            browser_cleanup,
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
        self.execute_with_options(task, progress_tx, AgentExecutionOptions::default())
            .await
    }

    /// Execute a task with per-run execution options.
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx, task), fields(session_id = %self.session.session_id, effort = ?options.effort))]
    pub async fn execute_with_options(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> Result<AgentExecutionOutcome> {
        self.execute_user_input_with_options(AgentUserInput::new(task), progress_tx, options)
            .await
    }

    /// Execute attachment-aware user input with per-run execution options.
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx, input), fields(session_id = %self.session.session_id, effort = ?options.effort))]
    pub async fn execute_user_input_with_options(
        &mut self,
        input: AgentUserInput,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution_request(ExecutionRequest::NewTask { input, options }, progress_tx)
            .await
    }

    /// Resume a paused task after receiving the user input it requested.
    pub async fn resume_after_user_input(
        &mut self,
        content: String,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        self.resume_after_user_input_with_options(
            content,
            progress_tx,
            AgentExecutionOptions::default(),
        )
        .await
    }

    /// Resume a paused task with per-run execution options.
    pub async fn resume_after_user_input_with_options(
        &mut self,
        content: String,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> Result<AgentExecutionOutcome> {
        self.resume_user_input_with_options(AgentUserInput::new(content), progress_tx, options)
            .await
    }

    /// Resume a paused task with attachment-aware user input and per-run execution options.
    pub async fn resume_user_input_with_options(
        &mut self,
        input: AgentUserInput,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution_request(
            ExecutionRequest::ResumeUserInput { input, options },
            progress_tx,
        )
        .await
    }

    /// Continue the saved task after queuing additional runtime context.
    pub async fn continue_after_runtime_context(
        &mut self,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution_request(ExecutionRequest::ContinueRuntimeContext, progress_tx)
            .await
    }
}

fn effort_prompt_instructions(
    base: Option<&str>,
    options: AgentExecutionOptions,
) -> Option<String> {
    let effort_guidance = match options.effort {
        AgentExecutionEffort::Standard => return base.map(str::to_string),
        AgentExecutionEffort::Extended => Some(
            "[EFFORT: Extended]\nFor web research tasks, use multiple targeted searches, read selected primary sources, cross-check important claims, and state blockers instead of stopping early.",
        ),
        AgentExecutionEffort::Heavy => Some(concat!(
            "[EFFORT: Heavy]\n",
            "For current factual, comparative, market, technical, legal, scientific, product, API, benchmark, or best/latest/top/current research tasks:\n",
            "- Create a source plan before final synthesis.\n",
            "- If `spawn_sub_agents` is available, start by delegating 2-4 independent research branches before final synthesis unless the task is clearly simple or strictly sequential.\n",
            "- Recommended branches: primary/official sources; recent independent secondary sources; contradictory evidence, criticism, and limitations; technical docs, benchmarks, repos, or changelogs when relevant.\n",
            "- Give each sub-agent a narrow task and an explicit tools whitelist using only available tools, for example `web_search`, `web_crawler`, and `web_markdown`.\n",
            "- For web-research sub-agents, include `web_crawler` when available for JS-rendered pages (use render:\"lightpanda\" or render:\"playwright\"); keep `web_markdown` as the lightweight HTTP-only fetch path.\n",
            "- Use `wait_sub_agents` before relying on delegated findings. Treat sub-agent output as leads, not final truth; cross-check important claims in the parent synthesis.\n",
            "- Use search plus extraction rather than snippets only, prioritize primary sources, and continue until evidence is sufficient or blockers are explicit.\n",
            "Before final answer, verify internally: current sources were used; selected URLs were read; primary sources and contradictions were checked; independent branches were delegated when useful and available; if not delegated, the task was simple/sequential or delegation was unavailable."
        )),
    }?;

    Some(
        match base.map(str::trim).filter(|value| !value.is_empty()) {
            Some(base) => format!("{base}\n\n{effort_guidance}"),
            None => effort_guidance.to_string(),
        },
    )
}

fn runtime_context_from_user_input(input: AgentUserInput) -> RuntimeContextInjection {
    RuntimeContextInjection::text(input.content).with_attachments(input.attachments)
}
