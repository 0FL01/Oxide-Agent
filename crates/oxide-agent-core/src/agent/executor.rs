//! Agent executor module
//!
//! Handles orchestration around the core agent runner, including
//! session lifecycle, skill prompts, and tool registry setup.

use super::compaction::{
    CompactionRequest, CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
    CompactionTrigger,
};
use super::hooks::{
    CompletionCheckHook, DelegationGuardHook, Hook, HookContext, HookEvent, HookResult,
    SearchBudgetHook, TimeoutReportHook, ToolAccessPolicyHook, WorkloadDistributorHook,
};
use super::memory::AgentMessage;
use super::profile::{AgentExecutionProfile, HookAccessPolicy, ToolAccessPolicy};
use super::prompt::create_agent_system_prompt;
use super::providers::{
    inject_approval_credentials, DelegationProvider, FileHosterProvider,
    ManagerControlPlaneProvider, ManagerTopicLifecycle, ReminderContext, ReminderProvider,
    SandboxProvider, SshApprovalGrant, SshApprovalRegistry, SshApprovalRequestView, SshMcpProvider,
    TodosProvider, TopicInfraPreflightReport, YtdlpProvider,
};
use super::registry::ToolRegistry;
use super::runner::{AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use super::session::{AgentSession, RuntimeContextInbox, RuntimeContextInjection};
use super::skills::SkillRegistry;
use super::tool_bridge::{execute_single_tool_call, ToolExecutionContext, ToolExecutionResult};
use crate::agent::progress::AgentEvent;
use crate::config::{get_agent_search_limit, AGENT_TIMEOUT_SECS};
use crate::llm::{LlmClient, ToolCall, ToolCallFunction};
use crate::storage::{StorageProvider, TopicInfraConfigRecord};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::info;

#[cfg(feature = "crawl4ai")]
use super::providers::Crawl4aiProvider;
#[cfg(feature = "tavily")]
use super::providers::TavilyProvider;

// Re-export sanitize_xml_tags for backward compatibility
pub use super::recovery::sanitize_xml_tags as public_sanitize_xml_tags;

/// Agent executor that runs tasks iteratively
pub struct AgentExecutor {
    runner: AgentRunner,
    session: AgentSession,
    skill_registry: Option<SkillRegistry>,
    settings: Arc<crate::config::AgentSettings>,
    manager_control_plane: Option<ManagerControlPlaneContext>,
    topic_infra: Option<TopicInfraContext>,
    reminder_context: Option<ReminderContext>,
    execution_profile: AgentExecutionProfile,
    tool_policy_state: Arc<RwLock<ToolAccessPolicy>>,
    hook_policy_state: Arc<RwLock<HookAccessPolicy>>,
    compaction_service: CompactionService,
    last_topic_infra_preflight_summary: Option<String>,
}

/// Terminal outcome of an agent execution request.
pub enum AgentExecutionOutcome {
    /// Agent finished and produced a final response.
    Completed(String),
    /// Agent paused because it is waiting for an external approval.
    WaitingForApproval,
}

#[derive(Clone)]
struct ManagerControlPlaneContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_lifecycle: Option<Arc<dyn ManagerTopicLifecycle>>,
}

#[derive(Clone)]
struct TopicInfraContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
    config: TopicInfraConfigRecord,
    approvals: SshApprovalRegistry,
}

struct PolicyControlledHook {
    name: &'static str,
    inner: Box<dyn Hook>,
    policy: Arc<RwLock<HookAccessPolicy>>,
}

impl PolicyControlledHook {
    fn new(
        name: &'static str,
        inner: Box<dyn Hook>,
        policy: Arc<RwLock<HookAccessPolicy>>,
    ) -> Self {
        Self {
            name,
            inner,
            policy,
        }
    }
}

impl Hook for PolicyControlledHook {
    fn name(&self) -> &'static str {
        self.name
    }

    fn handle(&self, event: &HookEvent, context: &HookContext) -> HookResult {
        if let Ok(policy) = self.policy.read() {
            if !policy.allows(self.name) {
                return HookResult::Continue;
            }
        }

        self.inner.handle(event, context)
    }
}

impl AgentExecutor {
    /// Create a new agent executor
    #[must_use]
    pub fn new(
        llm_client: Arc<LlmClient>,
        session: AgentSession,
        settings: Arc<crate::config::AgentSettings>,
    ) -> Self {
        let tool_policy_state = Arc::new(RwLock::new(ToolAccessPolicy::default()));
        let hook_policy_state = Arc::new(RwLock::new(HookAccessPolicy::default()));
        let mut runner = AgentRunner::new(llm_client.clone());
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        Self::register_policy_controlled_hook(
            &mut runner,
            WorkloadDistributorHook::new(),
            Arc::clone(&hook_policy_state),
        );
        Self::register_policy_controlled_hook(
            &mut runner,
            DelegationGuardHook::new(),
            Arc::clone(&hook_policy_state),
        );
        Self::register_policy_controlled_hook(
            &mut runner,
            SearchBudgetHook::new(get_agent_search_limit()),
            Arc::clone(&hook_policy_state),
        );
        runner.register_hook(Box::new(ToolAccessPolicyHook::new(Arc::clone(
            &tool_policy_state,
        ))));
        Self::register_policy_controlled_hook(
            &mut runner,
            TimeoutReportHook::new(),
            Arc::clone(&hook_policy_state),
        );

        let skill_registry = None;

        let compaction_service = {
            let (model_name, provider_name, _, timeout_secs) =
                settings.get_configured_compaction_model();
            CompactionService::default().with_summarizer(CompactionSummarizer::new(
                llm_client,
                CompactionSummarizerConfig {
                    model_name,
                    provider_name,
                    timeout_secs,
                },
            ))
        };

        Self {
            runner,
            session,
            skill_registry,
            settings,
            manager_control_plane: None,
            topic_infra: None,
            reminder_context: None,
            execution_profile: AgentExecutionProfile::default(),
            tool_policy_state,
            hook_policy_state,
            compaction_service,
            last_topic_infra_preflight_summary: None,
        }
    }

    fn register_policy_controlled_hook<H>(
        runner: &mut AgentRunner,
        hook: H,
        policy: Arc<RwLock<HookAccessPolicy>>,
    ) where
        H: Hook + 'static,
    {
        let name = hook.name();
        runner.register_hook(Box::new(PolicyControlledHook::new(
            name,
            Box::new(hook),
            policy,
        )));
    }

    /// Apply the latest execution profile for the next task run.
    pub fn set_execution_profile(&mut self, execution_profile: AgentExecutionProfile) {
        if let Ok(mut policy) = self.tool_policy_state.write() {
            *policy = execution_profile.tool_policy().clone();
        }
        if let Ok(mut policy) = self.hook_policy_state.write() {
            *policy = execution_profile.hook_policy().clone();
        }
        self.execution_profile = execution_profile;
    }

    /// Attach or clear topic-scoped infrastructure tooling.
    pub fn set_topic_infra(
        &mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
        config: Option<TopicInfraConfigRecord>,
    ) {
        self.topic_infra = config.map(|config| TopicInfraContext {
            storage,
            user_id,
            topic_id,
            config,
            approvals: self
                .topic_infra
                .as_ref()
                .map_or_else(SshApprovalRegistry::new, |ctx| ctx.approvals.clone()),
        });
    }

    /// Attach or clear reminder scheduling context for this executor.
    pub fn set_reminder_context(&mut self, context: ReminderContext) {
        self.reminder_context = Some(context);
    }

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
            .add_message(AgentMessage::system_context(content.clone()));
    }

    /// Attach user-scoped storage for manager control-plane tools.
    #[must_use]
    pub fn with_manager_control_plane(
        mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
    ) -> Self {
        self.manager_control_plane = Some(ManagerControlPlaneContext {
            storage,
            user_id,
            topic_lifecycle: None,
        });
        self
    }

    /// Attach transport forum topic lifecycle for manager tools.
    #[must_use]
    pub fn with_manager_topic_lifecycle(
        mut self,
        topic_lifecycle: Arc<dyn ManagerTopicLifecycle>,
    ) -> Self {
        if let Some(control_plane) = self.manager_control_plane.as_mut() {
            control_plane.topic_lifecycle = Some(topic_lifecycle);
        }
        self
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

    fn build_tool_registry(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));

        let sandbox_scope = self.session.sandbox_scope().clone();
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(sandbox_provider));
        registry.register(Box::new(FileHosterProvider::new(sandbox_scope.clone())));

        let ytdlp_provider = if let Some(tx) = progress_tx {
            YtdlpProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(ytdlp_provider));

        registry.register(Box::new(DelegationProvider::new(
            self.runner.llm_client(),
            sandbox_scope,
            self.settings.clone(),
        )));

        if let Some(control_plane) = &self.manager_control_plane {
            let mut manager_provider = ManagerControlPlaneProvider::new(
                Arc::clone(&control_plane.storage),
                control_plane.user_id,
            );
            if let Some(topic_lifecycle) = &control_plane.topic_lifecycle {
                manager_provider =
                    manager_provider.with_topic_lifecycle(Arc::clone(topic_lifecycle));
            }
            registry.register(Box::new(manager_provider));
        }

        if let Some(topic_infra) = &self.topic_infra {
            registry.register(Box::new(SshMcpProvider::new(
                Arc::clone(&topic_infra.storage),
                topic_infra.user_id,
                topic_infra.topic_id.clone(),
                topic_infra.config.clone(),
                topic_infra.approvals.clone(),
            )));
        }

        if let Some(reminder_context) = &self.reminder_context {
            registry.register(Box::new(ReminderProvider::new(reminder_context.clone())));
        }

        // Register web search provider based on configuration
        let search_provider = crate::config::get_search_provider();
        match search_provider.as_str() {
            "tavily" => {
                #[cfg(feature = "tavily")]
                if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
                    if !tavily_key.is_empty() {
                        if let Ok(p) = TavilyProvider::new(&tavily_key) {
                            registry.register(Box::new(p));
                        }
                    }
                }
                #[cfg(not(feature = "tavily"))]
                tracing::warn!("Tavily requested but feature not enabled");
            }
            "crawl4ai" => {
                #[cfg(feature = "crawl4ai")]
                if let Ok(url) = std::env::var("CRAWL4AI_URL") {
                    if !url.is_empty() {
                        registry.register(Box::new(Crawl4aiProvider::new(&url)));
                    }
                }
                #[cfg(not(feature = "crawl4ai"))]
                tracing::warn!("Crawl4AI requested but feature not enabled");
            }
            _ => unreachable!(), // get_search_provider() guarantees valid value
        }

        registry
    }

    async fn run_execution(
        &mut self,
        task: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
        append_user_message: bool,
        initial_tool_call: Option<ToolCall>,
        clear_pending_request_id: Option<&str>,
    ) -> Result<AgentExecutionOutcome> {
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

        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));

        let registry = self.build_tool_registry(Arc::clone(&todos_arc), progress_tx.as_ref());

        let tools = self
            .execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools());
        let (model_id, provider, model_max_output_tokens) =
            self.settings.get_configured_agent_model();
        let structured_output = !provider.eq_ignore_ascii_case("zai");
        let system_prompt = create_agent_system_prompt(
            task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
        )
        .await;
        let compaction_request = CompactionRequest::new(
            CompactionTrigger::PreRun,
            task,
            &system_prompt,
            &tools,
            &model_id,
            model_max_output_tokens,
            false,
        );
        let _ = self
            .compaction_service
            .prepare_for_run(&compaction_request, &mut self.session)
            .await?;
        let mut messages =
            AgentRunner::convert_memory_to_messages(self.session.memory.get_messages());

        if let Some(tool_call) = initial_tool_call {
            let cancellation_token = self.session.cancellation_token.clone();
            let tool_result = {
                let mut tool_ctx = ToolExecutionContext {
                    registry: &registry,
                    progress_tx: progress_tx.as_ref(),
                    todos_arc: &todos_arc,
                    messages: &mut messages,
                    agent: &mut self.session,
                    cancellation_token,
                };
                execute_single_tool_call(tool_call, &mut tool_ctx).await?
            };

            if let Some(request_id) = clear_pending_request_id {
                let _ = self.session.take_pending_ssh_replay(request_id);
            }

            if matches!(tool_result, ToolExecutionResult::WaitingForApproval { .. }) {
                self.session.complete();
                return Ok(AgentExecutionOutcome::WaitingForApproval);
            }
        }

        let mut ctx = AgentRunnerContext {
            task,
            system_prompt: &system_prompt,
            tools: &tools,
            registry: &registry,
            progress_tx: progress_tx.as_ref(),
            todos_arc: &todos_arc,
            task_id: &task_id,
            messages: &mut messages,
            agent: &mut self.session,
            skill_registry: self.skill_registry.as_mut(),
            config: {
                AgentRunnerConfig::new(
                    model_id,
                    crate::config::AGENT_MAX_ITERATIONS,
                    crate::config::AGENT_CONTINUATION_LIMIT,
                    self.settings.get_agent_timeout_secs(),
                )
            },
        };

        let timeout_duration = Duration::from_secs(AGENT_TIMEOUT_SECS);
        match timeout(timeout_duration, self.runner.run(&mut ctx)).await {
            Ok(inner) => match inner {
                Ok(super::runner::AgentRunResult::Final(res)) => {
                    self.session.complete();
                    Ok(AgentExecutionOutcome::Completed(res))
                }
                Ok(super::runner::AgentRunResult::WaitingForApproval) => {
                    self.session.complete();
                    Ok(AgentExecutionOutcome::WaitingForApproval)
                }
                Err(e) => {
                    self.session.fail(e.to_string());
                    Err(e)
                }
            },
            Err(_) => {
                self.session.timeout();
                let limit_mins = self.settings.get_agent_timeout_secs() / 60;
                Err(anyhow!(
                    "Task exceeded timeout limit ({} minutes)",
                    limit_mins
                ))
            }
        }
    }

    /// Execute a task with iterative tool calling (agentic loop)
    ///
    /// # Errors
    ///
    /// Returns an error if the LLM call fails, tool execution fails, or the iteration/timeout limits are exceeded.
    #[tracing::instrument(skip(self, progress_tx), fields(session_id = %self.session.session_id))]
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
        let tool_call = ToolCall {
            id: replay.tool_call_id,
            function: ToolCallFunction {
                name: replay.tool_name,
                arguments,
            },
            is_recovered: false,
        };

        self.run_execution(&task, progress_tx, false, Some(tool_call), Some(request_id))
            .await
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.runner.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.elapsed_secs() >= self.settings.get_agent_timeout_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentExecutor, PolicyControlledHook};
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::profile::HookAccessPolicy;
    use crate::agent::providers::TodoList;
    use crate::agent::providers::{
        ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult,
        ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
        ManagerTopicLifecycle,
    };
    use crate::agent::session::AgentSession;
    use crate::llm::LlmClient;
    use crate::storage::{
        AppendAuditEventOptions, AuditEventRecord, MockStorageProvider, TopicBindingKind,
        TopicBindingRecord,
    };
    use anyhow::{bail, Result};
    use mockall::predicate::eq;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use tokio::sync::Mutex;

    struct RecordingTopicLifecycle {
        create_calls: StdMutex<Vec<ForumTopicCreateRequest>>,
    }

    impl RecordingTopicLifecycle {
        fn new() -> Self {
            Self {
                create_calls: StdMutex::new(Vec::new()),
            }
        }

        fn create_calls(&self) -> Vec<ForumTopicCreateRequest> {
            match self.create_calls.lock() {
                Ok(calls) => calls.clone(),
                Err(_) => Vec::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ManagerTopicLifecycle for RecordingTopicLifecycle {
        async fn forum_topic_create(
            &self,
            request: ForumTopicCreateRequest,
        ) -> Result<ForumTopicCreateResult> {
            if let Ok(mut calls) = self.create_calls.lock() {
                calls.push(request.clone());
            }
            Ok(ForumTopicCreateResult {
                chat_id: request.chat_id.unwrap_or(-100_555),
                thread_id: 313,
                name: request.name,
                icon_color: request.icon_color.unwrap_or(9_367_192),
                icon_custom_emoji_id: request.icon_custom_emoji_id,
            })
        }

        async fn forum_topic_edit(
            &self,
            _request: ForumTopicEditRequest,
        ) -> Result<ForumTopicEditResult> {
            bail!("forum_topic_edit is not used by this test lifecycle")
        }

        async fn forum_topic_close(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_close is not used by this test lifecycle")
        }

        async fn forum_topic_reopen(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_reopen is not used by this test lifecycle")
        }

        async fn forum_topic_delete(
            &self,
            _request: ForumTopicThreadRequest,
        ) -> Result<ForumTopicActionResult> {
            bail!("forum_topic_delete is not used by this test lifecycle")
        }
    }

    fn build_executor() -> AgentExecutor {
        let settings = Arc::new(crate::config::AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let session = AgentSession::new(9_i64.into());
        AgentExecutor::new(llm, session, settings)
    }

    fn build_audit_record(options: AppendAuditEventOptions) -> AuditEventRecord {
        AuditEventRecord {
            schema_version: 1,
            version: 1,
            event_id: "evt-1".to_string(),
            user_id: options.user_id,
            topic_id: options.topic_id,
            agent_id: options.agent_id,
            action: options.action,
            payload: options.payload,
            created_at: 100,
        }
    }

    struct BlockingTestHook;

    impl Hook for BlockingTestHook {
        fn name(&self) -> &'static str {
            "workload_distributor"
        }

        fn handle(&self, _event: &HookEvent, _context: &HookContext) -> HookResult {
            HookResult::Block {
                reason: "test hook blocked".to_string(),
            }
        }
    }

    #[test]
    fn policy_controlled_hook_skips_disabled_manageable_hook() {
        let policy = Arc::new(std::sync::RwLock::new(HookAccessPolicy::new(
            None,
            std::collections::HashSet::from(["workload_distributor".to_string()]),
        )));
        let hook =
            PolicyControlledHook::new("workload_distributor", Box::new(BlockingTestHook), policy);
        let todos = TodoList::new();
        let memory = crate::agent::memory::AgentMemory::new(1024);

        let result = hook.handle(
            &HookEvent::BeforeAgent {
                prompt: "test".to_string(),
            },
            &HookContext::new(&todos, &memory, 0, 0, 4),
        );

        assert!(matches!(result, HookResult::Continue));
    }

    #[tokio::test]
    async fn manager_enabled_registry_executes_manager_tool() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 3,
                    user_id,
                    topic_id,
                    agent_id: "agent-a".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
            .await
            .expect("manager-enabled registry must route manager tool");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("manager tool response must be valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["binding"]["agent_id"], "agent-a");
    }

    #[tokio::test]
    async fn manager_disabled_registry_rejects_manager_tool() {
        let executor = build_executor();
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let err = registry
            .execute("topic_binding_get", r#"{"topic_id":"topic-a"}"#, None, None)
            .await
            .expect_err("manager-disabled registry must reject manager tools");

        assert!(err.to_string().contains("Unknown tool"));
    }

    #[tokio::test]
    async fn manager_dry_run_mutation_does_not_persist_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.user_id == 77
                    && options.action == "topic_binding_set"
                    && options.payload.get("outcome") == Some(&json!("dry_run"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_set",
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run manager mutation must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["preview"]["topic_id"], "topic-a");
    }

    #[tokio::test]
    async fn manager_dry_run_mutation_reports_audit_write_failure_non_fatally() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(None));
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_set",
                r#"{"topic_id":"topic-a","agent_id":"agent-a","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run manager mutation must remain non-fatal when audit write fails");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "write_failed");
        assert_eq!(parsed["preview"]["topic_id"], "topic-a");
    }

    #[tokio::test]
    async fn manager_executor_forum_topic_create_uses_lifecycle_with_non_fatal_audit() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_user_config()
            .returning(|_| Ok(crate::storage::UserConfig::default()));
        mock.expect_update_user_config().returning(|_, _| Ok(()));
        mock.expect_append_audit_event().returning(|_| {
            Err(crate::storage::StorageError::Config(
                "audit unavailable".to_string(),
            ))
        });

        let lifecycle = Arc::new(RecordingTopicLifecycle::new());
        let executor = build_executor()
            .with_manager_control_plane(Arc::new(mock), 77)
            .with_manager_topic_lifecycle(lifecycle.clone());
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "forum_topic_create",
                r#"{"chat_id":-100777,"name":"runtime-topic"}"#,
                None,
                None,
            )
            .await
            .expect("forum_topic_create must succeed when lifecycle succeeds");

        let parsed: serde_json::Value = serde_json::from_str(&response)
            .expect("forum topic create response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["topic"]["thread_id"], 313);
        assert_eq!(parsed["topic"]["name"], "runtime-topic");
        assert_eq!(parsed["audit_status"], "write_failed");

        let calls = lifecycle.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].chat_id, Some(-100_777));
        assert_eq!(calls[0].name, "runtime-topic");
    }

    #[tokio::test]
    async fn manager_executor_forum_topic_create_dry_run_skips_lifecycle() {
        let mut mock = MockStorageProvider::new();
        mock.expect_append_audit_event()
            .returning(|options| Ok(build_audit_record(options)));

        let lifecycle = Arc::new(RecordingTopicLifecycle::new());
        let executor = build_executor()
            .with_manager_control_plane(Arc::new(mock), 77)
            .with_manager_topic_lifecycle(lifecycle.clone());
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "forum_topic_create",
                r#"{"chat_id":-100777,"name":"dry-run","dry_run":true}"#,
                None,
                None,
            )
            .await
            .expect("dry-run forum_topic_create must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("dry-run response must be valid json");
        assert_eq!(parsed["ok"], true);
        assert_eq!(parsed["dry_run"], true);
        assert_eq!(parsed["audit_status"], "written");
        assert!(lifecycle.create_calls().is_empty());
    }

    #[tokio::test]
    async fn manager_rollback_restores_snapshot_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    topic_id,
                    agent_id: "agent-current".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(200_usize))
            .returning(|_, _, _| {
                Ok(vec![AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-previous".to_string()),
                    action: "topic_binding_set".to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": {
                            "schema_version": 1,
                            "version": 2,
                            "user_id": 77,
                            "topic_id": "topic-a",
                            "agent_id": "agent-previous",
                            "created_at": 1,
                            "updated_at": 2
                        },
                        "outcome": "applied"
                    }),
                    created_at: 30,
                }])
            });
        mock.expect_upsert_topic_binding()
            .withf(|options| {
                options.user_id == 77
                    && options.topic_id == "topic-a"
                    && options.agent_id == "agent-previous"
            })
            .returning(|options| {
                Ok(TopicBindingRecord {
                    schema_version: 1,
                    version: 6,
                    user_id: options.user_id,
                    topic_id: options.topic_id,
                    agent_id: options.agent_id,
                    binding_kind: options.binding_kind.unwrap_or(TopicBindingKind::Manual),
                    chat_id: options.chat_id.for_new_record(),
                    thread_id: options.thread_id.for_new_record(),
                    expires_at: options.expires_at.for_new_record(),
                    last_activity_at: options.last_activity_at,
                    created_at: 40,
                    updated_at: 50,
                })
            });
        mock.expect_delete_topic_binding().times(0);
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.action == "topic_binding_rollback"
                    && options.payload.get("operation") == Some(&json!("restore"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_rollback",
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback restore path must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("rollback response must be valid json");
        assert_eq!(parsed["operation"], "restore");
        assert_eq!(parsed["binding"]["agent_id"], "agent-previous");
    }

    #[tokio::test]
    async fn manager_rollback_deletes_when_snapshot_is_empty_via_executor_registry() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(TopicBindingRecord {
                    schema_version: 1,
                    version: 5,
                    user_id,
                    topic_id,
                    agent_id: "agent-current".to_string(),
                    binding_kind: TopicBindingKind::Manual,
                    chat_id: None,
                    thread_id: None,
                    expires_at: None,
                    last_activity_at: Some(20),
                    created_at: 10,
                    updated_at: 20,
                }))
            });
        mock.expect_list_audit_events_page()
            .with(eq(77_i64), eq(None), eq(200_usize))
            .returning(|_, _, _| {
                Ok(vec![AuditEventRecord {
                    schema_version: 1,
                    version: 4,
                    event_id: "evt-4".to_string(),
                    user_id: 77,
                    topic_id: Some("topic-a".to_string()),
                    agent_id: Some("agent-current".to_string()),
                    action: "topic_binding_delete".to_string(),
                    payload: json!({
                        "topic_id": "topic-a",
                        "previous": null,
                        "outcome": "applied"
                    }),
                    created_at: 30,
                }])
            });
        mock.expect_upsert_topic_binding().times(0);
        mock.expect_delete_topic_binding()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|_, _| Ok(()));
        mock.expect_append_audit_event()
            .withf(|options: &AppendAuditEventOptions| {
                options.action == "topic_binding_rollback"
                    && options.payload.get("operation") == Some(&json!("delete"))
            })
            .returning(|options| Ok(build_audit_record(options)));

        let executor = build_executor().with_manager_control_plane(Arc::new(mock), 77);
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute(
                "topic_binding_rollback",
                r#"{"topic_id":"topic-a"}"#,
                None,
                None,
            )
            .await
            .expect("rollback delete path must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("rollback response must be valid json");
        assert_eq!(parsed["operation"], "delete");
        assert!(parsed["binding"].is_null());
    }
}
