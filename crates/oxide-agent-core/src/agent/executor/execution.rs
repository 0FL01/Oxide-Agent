use super::types::{
    ExecutionRequest, ExecutionTransition, PreparedExecution, ResolvedExecutionRequest,
    RunnerContextServices,
};
use super::{
    AgentExecutionEffort, AgentExecutionOptions, AgentExecutionOutcome, AgentExecutor,
    AgentUserInput,
};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::agent::memory_behavior::{ToolDerivedMemoryDraft, ToolDerivedMemoryKind};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_agent_system_prompt;
use crate::agent::providers::{SshApprovalRequestView, TopicInfraPreflightReport};
use crate::agent::runner::{run_with_timeout, AgentRunner, AgentRunnerConfig};
use crate::agent::session::{AgentSession, RuntimeContextInbox, RuntimeContextInjection};
use crate::agent::wiki_memory::planner::{
    extract_explicit_remember_payload, has_explicit_remember_intent,
};
use crate::agent::wiki_memory::{
    wiki_context_id, WikiContextAssembler, WikiContextAssemblerConfig, WikiPatchOperation,
    WikiPatchPlanner, WikiPatchSet, WikiPatchValidator, WikiPatchValidatorConfig, WikiSessionCache,
    WikiStore,
};
use crate::config::{
    get_agent_continuation_limit, get_agent_max_iterations, get_agent_search_limit, ModelInfo,
};
use crate::llm::{InternalTextPurpose, LlmClient};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

static WIKI_MEMORY_BACKGROUND_WRITER_SEMAPHORE: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(1);
const WIKI_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES: usize = 12;
const WIKI_MEMORY_WRITER_MAX_MESSAGE_CHARS: usize = 1200;
const WIKI_MEMORY_WRITER_MAX_DRAFT_CHARS: usize = 1800;
const WIKI_MEMORY_WRITER_MAX_TITLE_CHARS: usize = 120;
const WIKI_MEMORY_WRITER_MAX_CANDIDATES: usize = 6;

struct WikiMemoryFlushJob {
    store: WikiStore,
    llm_client: Option<Arc<LlmClient>>,
    writer_model: Option<ModelInfo>,
    writer_timeout_secs: u64,
    context_id: String,
    task_id: String,
    task: String,
    final_response: String,
    transcript: Vec<WikiMemoryTranscriptMessage>,
    drafts: Vec<ToolDerivedMemoryDraft>,
}

#[derive(Debug, Clone)]
struct WikiMemoryTranscriptMessage {
    role: &'static str,
    content: String,
}

#[derive(Debug, Deserialize)]
struct WikiMemoryWriterExtraction {
    #[serde(default)]
    candidates: Vec<WikiMemoryWriterCandidate>,
}

#[derive(Debug, Deserialize)]
struct WikiMemoryWriterCandidate {
    kind: String,
    title: String,
    content: String,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    importance: Option<f32>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    evidence: Vec<String>,
}

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

    /// Reject a pending SSH approval request.
    pub async fn reject_ssh_approval(
        &mut self,
        request_id: &str,
    ) -> Option<SshApprovalRequestView> {
        let topic_infra = self.topic_infra.as_ref()?;
        topic_infra.approvals.reject(request_id).await
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

        let mut prepared = self
            .prepare_execution(&task, progress_tx.as_ref(), options)
            .await;
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
        );

        Ok(
            run_with_timeout(&mut self.runner, &mut ctx, timeout_duration)
                .await
                .into(),
        )
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
            ExecutionTransition::WaitingForApproval => {
                self.session.complete();
                Ok(AgentExecutionOutcome::WaitingForApproval)
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
            self.session.reset_memory_behavior_runtime();
            self.session.clear_todos();
        }
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        if let Some(user_input) = user_input {
            self.session.remember_task(task);
            let user_message = AgentMessage::user_task(user_input.text_projection())
                .with_user_attachments(user_input.attachments.clone());
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
        let task_for_wiki_update = request.task.clone();
        let timeout_error_message = self.agent_timeout_error_message(request.options);
        let transition = self.run_execution(request, progress_tx).await?;
        let outcome =
            self.apply_execution_transition(transition, timeout_error_message.as_str())?;
        if let AgentExecutionOutcome::Completed(final_response) = &outcome {
            self.spawn_wiki_memory_update_after_successful_run(
                task_for_wiki_update,
                final_response.clone(),
            );
        }
        Ok(outcome)
    }

    pub(super) async fn prepare_execution(
        &mut self,
        task: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        options: AgentExecutionOptions,
    ) -> PreparedExecution {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let model_routes = self
            .model_routes_override
            .clone()
            .unwrap_or_else(|| self.settings.get_configured_agent_model_routes());
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_agent_model());
        let tool_runtime_registry =
            Arc::new(self.build_tool_runtime_registry(Arc::clone(&todos_arc), progress_tx));
        let tools = tool_runtime_registry.specs();
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let wiki_context = self.render_wiki_context_for_task(task).await;
        let prompt_instructions =
            effort_prompt_instructions(self.execution_profile.prompt_instructions(), options);
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            structured_output,
            &mut self.session,
            prompt_instructions.as_deref(),
            wiki_context.as_deref(),
        )
        .await;
        let messages = AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());
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
        let timeout_secs = options.min_timeout_secs().map_or_else(
            || self.settings.get_agent_timeout_secs(),
            |minimum| self.settings.get_agent_timeout_secs().max(minimum),
        );
        let search_limit = options
            .min_search_limit()
            .map_or_else(get_agent_search_limit, |minimum| {
                get_agent_search_limit().max(minimum)
            });
        PreparedExecution {
            todos_arc,
            tool_runtime_registry,
            tools,
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
        }
    }

    async fn render_wiki_context_for_task(&self, task: &str) -> Option<String> {
        let Some(store) = self.wiki_memory_store.clone() else {
            debug!("wiki memory store is not configured; skipping durable wiki context");
            return None;
        };
        let scope = self.session.memory_scope();
        let cache = Arc::new(WikiSessionCache::new(store));
        let assembler = WikiContextAssembler::new(cache, WikiContextAssemblerConfig::default());

        match assembler
            .assemble_for_context(scope.user_id, &scope.context_key, task)
            .await
        {
            Ok(rendered) if !rendered.is_empty => Some(rendered.text),
            Ok(_) => None,
            Err(error) => {
                warn!(%error, "wiki memory context assembly failed; continuing without durable wiki context");
                None
            }
        }
    }

    fn spawn_wiki_memory_update_after_successful_run(&self, task: String, final_response: String) {
        let Some(store) = self.wiki_memory_store.clone() else {
            warn!("wiki memory store is not configured; skipping durable wiki update");
            return;
        };

        let scope = self.session.memory_scope();
        let context_id = wiki_context_id(scope.user_id, &scope.context_key);
        let task_id = self
            .session
            .current_task_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let drafts = self.session.memory_behavior_runtime().snapshot();
        let writer_model = self.settings.get_configured_wiki_memory_writer_model();
        let writer_enabled = self.settings.is_wiki_memory_writer_enabled();
        if self.settings.wiki_memory_writer_enabled.unwrap_or(false) && writer_model.is_none() {
            warn!("wiki memory writer is enabled but no writer model/provider is configured");
        }
        let llm_client = writer_enabled.then(|| self.runner.llm_client());
        let transcript = if writer_enabled {
            build_wiki_memory_writer_transcript(self.session.memory.get_messages())
        } else {
            Vec::new()
        };

        let job = WikiMemoryFlushJob {
            store,
            llm_client,
            writer_model,
            writer_timeout_secs: self.settings.get_wiki_memory_writer_timeout_secs(),
            context_id,
            task_id,
            task,
            final_response,
            transcript,
            drafts,
        };

        tokio::spawn(async move {
            Self::flush_wiki_memory_job(job).await;
        });
    }

    async fn flush_wiki_memory_job(job: WikiMemoryFlushJob) {
        let Ok(_permit) = WIKI_MEMORY_BACKGROUND_WRITER_SEMAPHORE.acquire().await else {
            warn!("wiki memory background writer semaphore is closed; skipping durable update");
            return;
        };

        let llm_drafts = Self::extract_wiki_memory_background_drafts(&job).await;
        let drafts = merge_wiki_memory_drafts(job.drafts.clone(), llm_drafts);
        let now = chrono::Utc::now();
        let Some(patch) = WikiPatchPlanner::default().plan_run_patch(
            &job.context_id,
            &job.task_id,
            &job.task,
            &drafts,
            now,
        ) else {
            debug!(
                task_id = job.task_id,
                context_id = job.context_id,
                draft_count = drafts.len(),
                "wiki memory background writer produced no durable patch after successful run"
            );
            return;
        };
        let patch = Self::merge_canonical_wiki_upserts(&job.store, &job.context_id, patch).await;
        if patch.operations.is_empty() {
            debug!(
                task_id = job.task_id,
                context_id = job.context_id,
                "wiki memory background update matched existing canonical memory; skipping durable update"
            );
            return;
        }

        let validator = WikiPatchValidator::new(WikiPatchValidatorConfig::default());
        let validated = match validator.validate(&job.store, &job.context_id, patch) {
            Ok(validated) => validated,
            Err(error) => {
                warn!(%error, "wiki memory background patch validation failed; skipping durable update");
                return;
            }
        };

        let cache = WikiSessionCache::new(job.store);
        let applied = match cache.apply_validated_patch(&validated).await {
            Ok(applied) => applied,
            Err(error) => {
                warn!(%error, "wiki memory background patch apply failed; skipping durable update");
                return;
            }
        };
        let metadata_pages = match cache
            .reconcile_context_patch_metadata(&job.context_id, &validated, now)
            .await
        {
            Ok(metadata_pages) => metadata_pages,
            Err(error) => {
                warn!(%error, "wiki memory background metadata reconciliation failed; flushing validated pages only");
                0
            }
        };

        match cache.flush_dirty_pages().await {
            Ok(result) if result.written_pages > 0 => {
                info!(
                    applied_ops = applied,
                    metadata_pages,
                    written_pages = result.written_pages,
                    skipped_unchanged_pages = result.skipped_unchanged_pages,
                    "wiki memory background update flushed after successful run"
                );
            }
            Ok(result) => {
                info!(
                    applied_ops = applied,
                    considered_pages = result.considered_pages,
                    skipped_unchanged_pages = result.skipped_unchanged_pages,
                    "wiki memory background update produced no changed pages"
                );
            }
            Err(error) => {
                warn!(%error, "wiki memory background flush failed; user task result preserved");
            }
        }
    }

    async fn merge_canonical_wiki_upserts(
        store: &WikiStore,
        context_id: &str,
        mut patch: WikiPatchSet,
    ) -> WikiPatchSet {
        let mut operations = Vec::with_capacity(patch.operations.len());
        for operation in patch.operations {
            let WikiPatchOperation::UpsertPage {
                path,
                expected_hash: _,
                content,
            } = operation
            else {
                operations.push(operation);
                continue;
            };

            let Some(file) = canonical_context_file(context_id, &path) else {
                operations.push(WikiPatchOperation::UpsertPage {
                    path,
                    expected_hash: None,
                    content,
                });
                continue;
            };

            match store.read_context_file(context_id, file).await {
                Ok(Some(existing)) => {
                    if let Some(merged) =
                        merge_existing_canonical_wiki_page(&existing.content, &content)
                    {
                        operations.push(WikiPatchOperation::UpsertPage {
                            path,
                            expected_hash: Some(existing.content_hash),
                            content: merged,
                        });
                    }
                }
                Ok(None) => {
                    operations.push(WikiPatchOperation::UpsertPage {
                        path,
                        expected_hash: None,
                        content,
                    });
                }
                Err(error) => {
                    warn!(
                        %error,
                        path,
                        "wiki memory failed to load canonical page for merge; writing fresh page"
                    );
                    operations.push(WikiPatchOperation::UpsertPage {
                        path,
                        expected_hash: None,
                        content,
                    });
                }
            }
        }
        patch.operations = operations;
        patch
    }

    async fn extract_wiki_memory_background_drafts(
        job: &WikiMemoryFlushJob,
    ) -> Vec<ToolDerivedMemoryDraft> {
        let (Some(llm_client), Some(model)) = (&job.llm_client, &job.writer_model) else {
            return Vec::new();
        };
        if job.transcript.is_empty() {
            return Vec::new();
        }

        let system_prompt = wiki_memory_writer_system_prompt();
        let user_prompt = build_wiki_memory_writer_user_prompt(job);
        let request = llm_client.complete_internal_text(
            InternalTextPurpose::WikiMemoryWriter,
            system_prompt,
            &user_prompt,
            model,
        );

        let response = match tokio::time::timeout(
            std::time::Duration::from_secs(job.writer_timeout_secs),
            request,
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                warn!(%error, "wiki memory background LLM extraction failed");
                return Vec::new();
            }
            Err(_) => {
                warn!(
                    timeout_secs = job.writer_timeout_secs,
                    "wiki memory background LLM extraction timed out"
                );
                return Vec::new();
            }
        };

        match parse_wiki_memory_writer_response(&response) {
            Ok(drafts) => drafts,
            Err(error) => {
                warn!(%error, "wiki memory background LLM extraction returned invalid JSON");
                Vec::new()
            }
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

    /// Deterministically resume a paused SSH tool call after operator approval.
    pub async fn resume_ssh_approval(
        &mut self,
        request_id: &str,
        _progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        Err(anyhow!(
            "SSH approval resume is disabled in typed tool runtime v1; request_id={request_id}"
        ))
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
        AgentExecutionEffort::Heavy => Some(
            concat!(
                "[EFFORT: Heavy]\n",
                "For current factual, comparative, market, technical, legal, scientific, product, API, benchmark, or best/latest/top/current research tasks:\n",
                "- Create a source plan before final synthesis.\n",
                "- If `spawn_sub_agents` is available, start by delegating 2-4 independent research branches before final synthesis unless the task is clearly simple or strictly sequential.\n",
                "- Recommended branches: primary/official sources; recent independent secondary sources; contradictory evidence, criticism, and limitations; technical docs, benchmarks, repos, or changelogs when relevant.\n",
                "- Give each sub-agent a narrow task and an explicit tools whitelist using only available tools, for example `web_search`, `web_extract`, `searxng_search`, `crawl4ai_markdown`, and `web_markdown`.\n",
                "- For web-research sub-agents, include `crawl4ai_markdown` when available for browser-rendered or JS-heavy pages; keep `web_markdown` as the lightweight fallback.\n",
                "- Use `wait_sub_agents` before relying on delegated findings. Treat sub-agent output as leads, not final truth; cross-check important claims in the parent synthesis.\n",
                "- Use search plus extraction rather than snippets only, prioritize primary sources, and continue until evidence is sufficient or blockers are explicit.\n",
                "Before final answer, verify internally: current sources were used; selected URLs were read; primary sources and contradictions were checked; independent branches were delegated when useful and available; if not delegated, the task was simple/sequential or delegation was unavailable."
            ),
        ),
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

fn build_wiki_memory_writer_transcript(
    messages: &[AgentMessage],
) -> Vec<WikiMemoryTranscriptMessage> {
    let mut transcript = messages
        .iter()
        .rev()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::System | MessageRole::Tool => return None,
            };
            let content = truncate_wiki_writer_text(
                message.content.trim(),
                WIKI_MEMORY_WRITER_MAX_MESSAGE_CHARS,
            );
            (!content.is_empty()).then_some(WikiMemoryTranscriptMessage { role, content })
        })
        .take(WIKI_MEMORY_WRITER_MAX_TRANSCRIPT_MESSAGES)
        .collect::<Vec<_>>();
    transcript.reverse();
    transcript
}

fn wiki_memory_writer_system_prompt() -> &'static str {
    "You are a background memory curator for LLM Wiki. Extract only durable facts, preferences, or reusable procedures that the user explicitly asked to remember, clearly stated as an ongoing preference, or proved as a reusable procedure in the completed conversation. If the latest user message only says to save/remember something without a payload, resolve the payload from the immediately preceding user/assistant context. Do not save command failures, transient diagnostics, guesses, generic task outcomes, or facts that are already represented by deterministic drafts. Do not invent facts. Do not include secrets, credentials, API keys, tokens, passwords, or private keys. Return only JSON with this shape: {\"candidates\":[{\"kind\":\"fact|preference|procedure\",\"title\":\"short title\",\"content\":\"durable memory text\",\"confidence\":0.0,\"importance\":0.0,\"tags\":[\"tag\"],\"evidence\":[\"short evidence\"],\"reason\":\"why save\"}]}. Use confidence >= 0.85 only when the memory is explicit and durable. Return {\"candidates\":[]} when nothing should be saved."
}

fn build_wiki_memory_writer_user_prompt(job: &WikiMemoryFlushJob) -> String {
    let mut prompt = String::new();
    prompt.push_str("Completed task:\n");
    prompt.push_str(job.task.trim());
    prompt.push_str("\n\nFinal answer:\n");
    prompt.push_str(job.final_response.trim());
    prompt.push_str("\n\nRecent transcript:\n");
    for message in &job.transcript {
        prompt.push_str("- ");
        prompt.push_str(message.role);
        prompt.push_str(": ");
        prompt.push_str(&message.content.replace('\n', " "));
        prompt.push('\n');
    }
    if !job.drafts.is_empty() {
        prompt.push_str("\nDeterministic drafts already captured:\n");
        for draft in &job.drafts {
            prompt.push_str("- ");
            prompt.push_str(&draft.title.replace('\n', " "));
            prompt.push_str(": ");
            prompt.push_str(&draft.content.replace('\n', " "));
            prompt.push('\n');
        }
    }
    prompt
}

fn parse_wiki_memory_writer_response(response: &str) -> Result<Vec<ToolDerivedMemoryDraft>> {
    let extraction: WikiMemoryWriterExtraction = serde_json::from_str(response).or_else(|_| {
        extract_json_object(response)
            .map_or_else(|| serde_json::from_str(response), serde_json::from_str)
    })?;
    let now = chrono::Utc::now();
    Ok(extraction
        .candidates
        .into_iter()
        .take(WIKI_MEMORY_WRITER_MAX_CANDIDATES)
        .filter_map(|candidate| wiki_memory_candidate_to_draft(candidate, now))
        .collect())
}

fn extract_json_object(response: &str) -> Option<&str> {
    let start = response.find('{')?;
    let end = response.rfind('}')?;
    (start <= end).then_some(&response[start..=end])
}

fn wiki_memory_candidate_to_draft(
    candidate: WikiMemoryWriterCandidate,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<ToolDerivedMemoryDraft> {
    let kind = match candidate.kind.trim().to_ascii_lowercase().as_str() {
        "fact" | "note" | "observation" => ToolDerivedMemoryKind::Fact,
        "preference" => ToolDerivedMemoryKind::Preference,
        "procedure" | "workflow" => ToolDerivedMemoryKind::Procedure,
        _ => return None,
    };
    let title =
        truncate_wiki_writer_text(candidate.title.trim(), WIKI_MEMORY_WRITER_MAX_TITLE_CHARS);
    let content =
        truncate_wiki_writer_text(candidate.content.trim(), WIKI_MEMORY_WRITER_MAX_DRAFT_CHARS);
    if title.is_empty() || content.is_empty() || contains_secret_like_text(&content) {
        return None;
    }

    let short_description = candidate
        .short_description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .unwrap_or(&content);

    let mut tags = candidate
        .tags
        .into_iter()
        .map(|tag| truncate_wiki_writer_text(tag.trim(), 48))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    push_unique(&mut tags, "background-llm-writer".to_string());

    Some(ToolDerivedMemoryDraft {
        kind,
        title,
        content: content.clone(),
        short_description: truncate_wiki_writer_text(short_description, 180),
        importance: clamp_unit(candidate.importance.unwrap_or(0.78)),
        confidence: clamp_unit(candidate.confidence.unwrap_or(0.74)),
        source: "background_llm_writer".to_string(),
        reason: candidate
            .reason
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or_else(|| "background LLM extracted durable memory after run".to_string()),
        tags,
        evidence: candidate
            .evidence
            .into_iter()
            .map(|evidence| truncate_wiki_writer_text(evidence.trim(), 240))
            .filter(|evidence| !evidence.is_empty())
            .take(4)
            .collect(),
        captured_at: now,
    })
}

fn merge_wiki_memory_drafts(
    mut deterministic: Vec<ToolDerivedMemoryDraft>,
    llm_drafts: Vec<ToolDerivedMemoryDraft>,
) -> Vec<ToolDerivedMemoryDraft> {
    if llm_drafts.is_empty() {
        return deterministic;
    }

    deterministic.retain(|draft| !is_vacuous_explicit_remember_draft(draft));
    for draft in llm_drafts {
        if let Some(existing) = deterministic
            .iter_mut()
            .find(|existing| memory_drafts_are_similar(existing, &draft))
        {
            merge_wiki_memory_draft(existing, draft);
        } else if deterministic.len() < WIKI_MEMORY_WRITER_MAX_CANDIDATES * 2 {
            deterministic.push(draft);
        }
    }
    deterministic
}

fn merge_wiki_memory_draft(existing: &mut ToolDerivedMemoryDraft, draft: ToolDerivedMemoryDraft) {
    if existing.content.len() < draft.content.len()
        && is_vacuous_explicit_remember_draft(existing)
        && !is_vacuous_explicit_remember_draft(&draft)
    {
        existing.title = draft.title.clone();
        existing.content = draft.content.clone();
        existing.short_description = draft.short_description.clone();
    }
    existing.confidence = existing.confidence.max(draft.confidence);
    existing.importance = existing.importance.max(draft.importance);
    if !existing.source.contains(&draft.source) {
        existing.source = format!("{},{}", existing.source, draft.source);
    }
    for tag in draft.tags {
        push_unique(&mut existing.tags, tag);
    }
    for evidence in draft.evidence {
        push_unique(&mut existing.evidence, evidence);
    }
}

fn is_vacuous_explicit_remember_draft(draft: &ToolDerivedMemoryDraft) -> bool {
    draft.source.contains("explicit_remember_capture")
        && has_explicit_remember_intent(&draft.content)
        && extract_explicit_remember_payload(&draft.content).is_none()
}

fn memory_drafts_are_similar(
    left: &ToolDerivedMemoryDraft,
    right: &ToolDerivedMemoryDraft,
) -> bool {
    if left.kind != right.kind {
        return false;
    }
    let left_title = normalized_memory_text(&left.title);
    let right_title = normalized_memory_text(&right.title);
    let left_content = normalized_memory_text(&left.content);
    let right_content = normalized_memory_text(&right.content);

    (!left_title.is_empty() && left_title == right_title)
        || (!left_content.is_empty() && left_content == right_content)
        || (left_content.len() > 24 && right_content.contains(&left_content))
        || (right_content.len() > 24 && left_content.contains(&right_content))
}

fn normalized_memory_text(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn truncate_wiki_writer_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect::<String>()
}

fn clamp_unit(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

fn canonical_context_file<'a>(context_id: &str, path: &'a str) -> Option<&'a str> {
    let relative = path.strip_prefix(&format!("contexts/{context_id}/"))?;
    match relative {
        "constraints.md" | "procedures.md" => Some(relative),
        _ => None,
    }
}

fn merge_existing_canonical_wiki_page(existing: &str, candidate: &str) -> Option<String> {
    let candidate_body = extract_wiki_page_memory_body(candidate)?;
    let normalized_candidate = normalized_memory_text(candidate_body);
    if normalized_candidate.len() > 24
        && normalized_memory_text(existing).contains(&normalized_candidate)
    {
        return None;
    }

    let title = extract_wiki_page_title(candidate).unwrap_or("Wiki memory update");
    let mut merged = existing.trim_end().to_string();
    merged.push_str("\n\n## Entry - ");
    merged.push_str(title.trim());
    merged.push_str("\n\n");
    merged.push_str(candidate_body.trim());
    merged.push('\n');
    Some(merged)
}

fn extract_wiki_page_title(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("# ")
            .map(str::trim)
            .filter(|title| !title.is_empty())
    })
}

fn extract_wiki_page_memory_body(content: &str) -> Option<&str> {
    let after_title = content
        .split_once("\n# ")
        .and_then(|(_, rest)| rest.split_once('\n').map(|(_, body)| body))
        .unwrap_or(content);
    let body = after_title
        .split("\n\n## Capture")
        .next()
        .unwrap_or(after_title)
        .trim();
    (!body.is_empty()).then_some(body)
}

fn contains_secret_like_text(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "token=",
        "password",
        "secret",
        "private key",
        "-----begin",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

#[cfg(test)]
mod wiki_memory_merge_tests {
    use super::merge_existing_canonical_wiki_page;

    fn page(title: &str, body: &str) -> String {
        format!(
            "---\ntitle: {title}\ntype: procedure\nupdated_at: 2026-05-25T00:00:00Z\nconfidence: high\ntags:\n  - procedure\nsources:\n  - run:test\n---\n\n# {title}\n\n{body}\n\n## Capture\n\n- Kind: procedure\n"
        )
    }

    #[test]
    fn canonical_merge_appends_new_entry() {
        let existing = page("Procedures", "Run smoke tests before deploy.");
        let candidate = page(
            "Rollback workflow",
            "Keep rollback notes next to release notes.",
        );

        let merged = merge_existing_canonical_wiki_page(&existing, &candidate)
            .expect("new candidate should append");

        assert!(merged.contains("Run smoke tests before deploy."));
        assert!(merged.contains("## Entry - Rollback workflow"));
        assert!(merged.contains("Keep rollback notes next to release notes."));
    }

    #[test]
    fn canonical_merge_skips_duplicate_entry() {
        let existing = page("Procedures", "Run smoke tests before deploy.");
        let candidate = page("Deploy workflow", "Run smoke tests before deploy.");

        assert!(merge_existing_canonical_wiki_page(&existing, &candidate).is_none());
    }
}
