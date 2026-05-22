use super::types::{
    current_model_route, ExecutionRequest, ExecutionTransition, PreparedExecution,
    ResolvedExecutionRequest, RunnerContextServices,
};
use super::{AgentExecutionOutcome, AgentExecutor};
use crate::agent::memory::{AgentMessage, MessageRole};
use crate::agent::memory_behavior::{ToolDerivedMemoryDraft, ToolDerivedMemoryKind};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_agent_system_prompt;
use crate::agent::providers::{
    inject_approval_credentials, SshApprovalGrant, SshApprovalRequestView,
    TopicInfraPreflightReport,
};
use crate::agent::runner::{run_with_timeout, AgentRunner, AgentRunnerConfig};
use crate::agent::session::{AgentSession, RuntimeContextInbox, RuntimeContextInjection};
use crate::agent::tool_bridge::{
    execute_single_tool_call, ToolExecutionContext, ToolExecutionResult,
};
use crate::agent::tool_runtime::scope_tool_model_route;
use crate::agent::wiki_memory::planner::{
    extract_explicit_remember_payload, has_explicit_remember_intent,
};
use crate::agent::wiki_memory::{
    wiki_context_id, WikiContextAssembler, WikiContextAssemblerConfig, WikiPatchPlanner,
    WikiPatchValidator, WikiPatchValidatorConfig, WikiSessionCache, WikiStore,
};
use crate::config::{get_agent_max_iterations, ModelInfo};
use crate::llm::{LlmClient, ToolCall, ToolCallFunction};
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
        request: ResolvedExecutionRequest,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<ExecutionTransition> {
        let ResolvedExecutionRequest {
            task,
            append_user_message,
            initial_tool_call,
            clear_pending_request_id,
        } = request;
        let task_id = self.prime_session_for_execution(&task, append_user_message);
        info!(
            task = %task,
            task_id = %task_id,
            memory_messages = self.session.memory.get_messages().len(),
            memory_tokens = self.session.memory.token_count(),
            "Starting agent task"
        );

        let mut prepared = self.prepare_execution(&task, progress_tx.as_ref()).await;
        Self::emit_milestone(progress_tx.as_ref(), "prepare_execution_done").await;

        let timeout_duration = self.agent_timeout_duration();

        if self
            .replay_initial_tool_call(
                initial_tool_call,
                clear_pending_request_id.as_deref(),
                &mut prepared,
                progress_tx.as_ref(),
            )
            .await?
        {
            return Ok(ExecutionTransition::WaitingForApproval);
        }

        let mut ctx = prepared.build_runner_context(
            &task,
            &task_id,
            progress_tx.as_ref(),
            &mut self.session,
            self.skill_registry.as_mut(),
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

    fn prime_session_for_execution(&mut self, task: &str, append_user_message: bool) -> String {
        if append_user_message {
            self.session.reset_memory_behavior_runtime();
            self.session.clear_todos();
        }
        self.session.start_task();
        let task_id = self.session.current_task_id.clone().unwrap_or_default();
        if append_user_message {
            self.session.remember_task(task);
            self.session
                .memory
                .add_message(AgentMessage::user_task(task));
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
            ExecutionRequest::NewTask { task } => Ok(ResolvedExecutionRequest {
                task,
                append_user_message: true,
                initial_tool_call: None,
                clear_pending_request_id: None,
            }),
            ExecutionRequest::ResumeApproval { request_id } => {
                let task = self.saved_task("no saved task to resume")?;
                let grant = self
                    .grant_ssh_approval(&request_id)
                    .await
                    .ok_or_else(|| anyhow!("SSH approval request not found or already handled"))?;
                let replay = self
                    .session
                    .pending_ssh_replay(&request_id)
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

                Ok(ResolvedExecutionRequest {
                    task,
                    append_user_message: false,
                    initial_tool_call: Some(tool_call),
                    clear_pending_request_id: Some(request_id),
                })
            }
            ExecutionRequest::ResumeUserInput { content } => {
                let task = self.saved_task("no saved task to resume")?;
                if !self.resume_with_user_input(content) {
                    return Err(anyhow!("session is not waiting for user input"));
                }

                Ok(ResolvedExecutionRequest {
                    task,
                    append_user_message: false,
                    initial_tool_call: None,
                    clear_pending_request_id: None,
                })
            }
            ExecutionRequest::ContinueRuntimeContext => {
                let task = self.saved_task("no saved task to continue")?;
                if !self.session.has_pending_runtime_context() {
                    return Err(anyhow!("session has no queued runtime context"));
                }

                Ok(ResolvedExecutionRequest {
                    task,
                    append_user_message: false,
                    initial_tool_call: None,
                    clear_pending_request_id: None,
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
        let timeout_error_message = self.agent_timeout_error_message();
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
        let wiki_context = self.render_wiki_context_for_task(task).await;
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
            wiki_context.as_deref(),
        )
        .await;
        let messages = AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());
        PreparedExecution {
            todos_arc,
            registry,
            tools,
            system_prompt,
            messages,
            runner_config: AgentRunnerConfig::new(
                model.id.clone(),
                get_agent_max_iterations(),
                crate::config::AGENT_CONTINUATION_LIMIT,
                self.settings.get_agent_timeout_secs(),
                model.max_output_tokens,
            )
            .with_model_provider(model.provider.clone())
            .with_temperature(self.settings.get_configured_agent_temperature())
            .with_model_routes(model_routes)
            .with_codex_style_compaction(self.settings.codex_style_compaction_enabled()),
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
        let request =
            llm_client.chat_completion_for_model_info(system_prompt, &[], &user_prompt, model);

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
        self.run_execution_request(
            ExecutionRequest::NewTask {
                task: task.to_string(),
            },
            progress_tx,
        )
        .await
    }

    /// Deterministically resume a paused SSH tool call after operator approval.
    pub async fn resume_ssh_approval(
        &mut self,
        request_id: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution_request(
            ExecutionRequest::ResumeApproval {
                request_id: request_id.to_string(),
            },
            progress_tx,
        )
        .await
    }

    /// Resume a paused task after receiving the user input it requested.
    pub async fn resume_after_user_input(
        &mut self,
        content: String,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<AgentExecutionOutcome> {
        self.run_execution_request(ExecutionRequest::ResumeUserInput { content }, progress_tx)
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
    "You are a background memory curator for LLM Wiki. Extract only durable facts, preferences, or reusable procedures that are explicitly requested to be remembered or strongly implied by the completed conversation. If the latest user message only says to save/remember something without a payload, resolve the payload from the immediately preceding user/assistant context. Do not invent facts. Do not include secrets, credentials, API keys, tokens, passwords, or private keys. Return only JSON with this shape: {\"candidates\":[{\"kind\":\"fact|preference|procedure\",\"title\":\"short title\",\"content\":\"durable memory text\",\"confidence\":0.0,\"importance\":0.0,\"tags\":[\"tag\"],\"evidence\":[\"short evidence\"],\"reason\":\"why save\"}]}. Return {\"candidates\":[]} when nothing should be saved."
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
        confidence: clamp_unit(candidate.confidence.unwrap_or(0.86)),
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
