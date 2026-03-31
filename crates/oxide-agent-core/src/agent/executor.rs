//! Agent executor module
//!
//! Handles orchestration around the core agent runner, including
//! session lifecycle, skill prompts, and tool registry setup.

use super::compaction::{
    CompactionOutcome, CompactionRequest, CompactionService, CompactionSummarizer,
    CompactionSummarizerConfig, CompactionTrigger,
};
use super::hooks::{
    CompletionCheckHook, DelegationGuardHook, Hook, HookContext, HookEvent, HookResult,
    SearchBudgetHook, TimeoutReportHook, ToolAccessPolicyHook, WorkloadDistributorHook,
};
use super::memory::AgentMessage;
use super::profile::{AgentExecutionProfile, HookAccessPolicy, ToolAccessPolicy};
use super::prompt::create_agent_system_prompt;
use super::providers::{
    inject_approval_credentials, AgentsMdProvider, DelegationProvider, FileHosterProvider,
    KokoroTtsProvider, ManagerControlPlaneProvider, ManagerTopicLifecycle, MediaFileProvider,
    ReminderContext, ReminderProvider, SandboxProvider, SshApprovalGrant, SshApprovalRegistry,
    SshApprovalRequestView, SshMcpProvider, TodoList, TodosProvider, TopicInfraPreflightReport,
    YtdlpProvider,
};
use super::registry::ToolRegistry;
use super::runner::{AgentRunResult, AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use super::session::{
    AgentSession, PendingUserInput, RuntimeContextInbox, RuntimeContextInjection,
};
use super::skills::SkillRegistry;
use super::tool_bridge::{execute_single_tool_call, ToolExecutionContext, ToolExecutionResult};
use crate::agent::progress::AgentEvent;
use crate::config::{get_agent_max_iterations, get_agent_search_limit};
use crate::llm::{LlmClient, Message, ToolCall, ToolCallFunction, ToolDefinition};
use crate::storage::{StorageProvider, TopicInfraConfigRecord};
use anyhow::{anyhow, Result};
use std::future::Future;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};

#[cfg(feature = "crawl4ai")]
use super::providers::Crawl4aiProvider;
#[cfg(feature = "searxng")]
use super::providers::SearxngProvider;
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
    agents_md: Option<AgentsMdContext>,
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
    /// Agent paused because it is waiting for additional user input.
    WaitingForUserInput(PendingUserInput),
}

#[derive(Clone)]
struct AgentsMdContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
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

struct PreparedExecution {
    todos_arc: Arc<Mutex<TodoList>>,
    registry: ToolRegistry,
    tools: Vec<ToolDefinition>,
    system_prompt: String,
    messages: Vec<Message>,
    runner_config: AgentRunnerConfig,
}

enum TimedRunResult {
    Final(String),
    WaitingForApproval,
    WaitingForUserInput(PendingUserInput),
    Failed(anyhow::Error),
    TimedOut,
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
        mut session: AgentSession,
        settings: Arc<crate::config::AgentSettings>,
    ) -> Self {
        session.set_context_window_tokens(settings.get_agent_internal_context_budget_tokens());
        let tool_policy_state = Arc::new(RwLock::new(ToolAccessPolicy::default()));
        let hook_policy_state = Arc::new(RwLock::new(HookAccessPolicy::default()));
        let mut runner = AgentRunner::new(Arc::clone(&llm_client));
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
            let (_, _, _, timeout_secs) = settings.get_configured_compaction_model();
            CompactionService::default().with_summarizer(CompactionSummarizer::new(
                llm_client,
                CompactionSummarizerConfig {
                    model_routes: settings.get_configured_compaction_model_routes(false),
                    timeout_secs,
                    ..CompactionSummarizerConfig::default()
                },
            ))
        };

        Self {
            runner,
            session,
            skill_registry,
            settings,
            agents_md: None,
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

    /// Attach topic-scoped AGENTS.md tooling.
    pub fn set_agents_md_context(
        &mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: String,
    ) {
        self.agents_md = Some(AgentsMdContext {
            storage,
            user_id,
            topic_id,
        });
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
            .add_message(AgentMessage::system_context(content));
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

    /// Build the currently exposed tool definitions for this executor state.
    #[must_use]
    pub fn current_tool_definitions(&self) -> Vec<ToolDefinition> {
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let registry = self.build_tool_registry(todos_arc, None);
        self.execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools())
    }

    fn build_tool_registry(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();

        // Core providers: todos, sandbox, filehoster, media file analysis, ytdlp, delegation
        self.register_core_providers(&mut registry, todos_arc, progress_tx);

        // Topic-scoped providers: agents_md, manager, ssh, reminders
        self.register_topic_providers(&mut registry);

        // Feature-gated MCP and search providers
        self.register_mcp_providers(&mut registry);
        self.register_search_providers(&mut registry);

        // Optional TTS providers.
        self.register_kokoro_tts_provider(&mut registry, progress_tx);
        self.register_silero_tts_provider(&mut registry, progress_tx);

        registry
    }

    fn register_core_providers(
        &self,
        registry: &mut ToolRegistry,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        registry.register(Box::new(TodosProvider::new(Arc::clone(&todos_arc))));

        let sandbox_scope = self.session.sandbox_scope().clone();
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(sandbox_provider));
        registry.register(Box::new(FileHosterProvider::new(sandbox_scope.clone())));
        registry.register(Box::new(MediaFileProvider::new(
            self.runner.llm_client(),
            sandbox_scope.clone(),
        )));

        let ytdlp_provider = if let Some(tx) = progress_tx {
            YtdlpProvider::new(sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(sandbox_scope.clone())
        };
        registry.register(Box::new(ytdlp_provider));

        registry.register(Box::new(DelegationProvider::new(
            self.runner.llm_client(),
            sandbox_scope,
            Arc::clone(&self.settings),
        )));
    }

    fn register_topic_providers(&self, registry: &mut ToolRegistry) {
        if let Some(agents_md) = &self.agents_md {
            registry.register(Box::new(AgentsMdProvider::new(
                Arc::clone(&agents_md.storage),
                agents_md.user_id,
                agents_md.topic_id.clone(),
            )));
        }

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
    }

    #[cfg(feature = "jira")]
    fn register_jira_mcp_provider(registry: &mut ToolRegistry) {
        if let Some(config) = crate::agent::providers::JiraMcpConfig::from_env() {
            let binary_path = config.binary_path.clone();
            tracing::info!(
                binary_path = %binary_path,
                jira_url_present = !config.jira_url.is_empty(),
                jira_email_present = !config.jira_email.is_empty(),
                jira_token_present = !config.jira_token.is_empty(),
                "Registering Jira MCP provider"
            );
            registry.register(Box::new(crate::agent::providers::JiraMcpProvider::new(
                config,
            )));
            tracing::info!(binary_path = %binary_path, "Jira MCP provider registered");
        } else {
            tracing::warn!(
                "jira feature is enabled but JIRA_URL, JIRA_EMAIL, or JIRA_API_TOKEN is not set; \
                 Jira MCP provider will not be available. Set these env vars to enable it."
            );
        }
    }

    #[cfg(feature = "mattermost")]
    fn register_mattermost_mcp_provider(registry: &mut ToolRegistry) {
        if let Some(config) = crate::agent::providers::MattermostMcpConfig::from_env() {
            let binary_path = config.binary_path.clone();
            tracing::info!(
                binary_path = %binary_path,
                mattermost_url_present = !config.mattermost_url.is_empty(),
                mattermost_token_present = !config.mattermost_token.is_empty(),
                timeout_secs = config.timeout_secs,
                max_retries = config.max_retries,
                verify_ssl = config.verify_ssl,
                "Registering Mattermost MCP provider"
            );
            registry.register(Box::new(
                crate::agent::providers::MattermostMcpProvider::new(config),
            ));
            tracing::info!(binary_path = %binary_path, "Mattermost MCP provider registered");
        } else {
            tracing::warn!(
                "mattermost feature is enabled but MATTERMOST_URL or MATTERMOST_TOKEN is not set; \
                 Mattermost MCP provider will not be available. Set these env vars to enable it."
            );
        }
    }

    fn register_mcp_providers(&self, _registry: &mut ToolRegistry) {
        #[cfg(feature = "jira")]
        Self::register_jira_mcp_provider(_registry);

        #[cfg(feature = "mattermost")]
        Self::register_mattermost_mcp_provider(_registry);
    }

    fn register_search_providers(&self, registry: &mut ToolRegistry) {
        #[cfg(feature = "tavily")]
        if crate::config::is_tavily_enabled() {
            if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
                if !tavily_key.trim().is_empty() {
                    if let Ok(provider) = TavilyProvider::new(&tavily_key) {
                        registry.register(Box::new(provider));
                    }
                } else {
                    warn!("Tavily enabled but TAVILY_API_KEY is empty; provider not registered");
                }
            } else {
                warn!("Tavily enabled but TAVILY_API_KEY is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "tavily"))]
        if crate::config::is_tavily_enabled() {
            tracing::warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(feature = "searxng")]
        if crate::config::is_searxng_enabled() {
            if let Some(url) = crate::config::get_searxng_url() {
                if !url.trim().is_empty() {
                    match SearxngProvider::new(&url) {
                        Ok(provider) => registry.register(Box::new(provider)),
                        Err(error) => {
                            warn!(error = %error, "SearXNG provider initialization failed")
                        }
                    }
                } else {
                    warn!("SearXNG enabled but SEARXNG_URL is empty; provider not registered");
                }
            } else {
                warn!("SearXNG enabled but SEARXNG_URL is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "searxng"))]
        if crate::config::is_searxng_enabled() {
            tracing::warn!("SearXNG enabled but feature not compiled in");
        }

        #[cfg(feature = "crawl4ai")]
        if crate::config::is_crawl4ai_enabled() {
            if let Some(url) = crate::config::get_crawl4ai_url() {
                if !url.trim().is_empty() {
                    registry.register(Box::new(Crawl4aiProvider::new(&url)));
                } else {
                    warn!("Crawl4AI enabled but CRAWL4AI_URL is empty; provider not registered");
                }
            } else {
                warn!("Crawl4AI enabled but CRAWL4AI_URL is not set; provider not registered");
            }
        }
        #[cfg(not(feature = "crawl4ai"))]
        if crate::config::is_crawl4ai_enabled() {
            tracing::warn!("Crawl4AI enabled but feature not compiled in");
        }
    }

    fn register_kokoro_tts_provider(
        &self,
        registry: &mut ToolRegistry,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        // Get config with default values (uses DEFAULT_KOKORO_URL if env var not set)
        let config = crate::agent::providers::tts::TtsConfig::from_env();

        // Skip registration only if explicitly disabled (empty string in env var)
        if let Ok(url) = std::env::var("KOKORO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "TTS provider disabled: KOKORO_TTS_URL is explicitly set to empty string"
                );
                return;
            }
        }

        tracing::debug!(url = %config.base_url, "Registering TTS provider");
        let sandbox_scope = self.session.sandbox_scope().clone();

        // Register the TTS provider (health check is done lazily on first use)
        let provider = if let Some(tx) = progress_tx {
            KokoroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
                .with_progress_tx(tx.clone())
        } else {
            KokoroTtsProvider::from_config(config).with_sandbox_scope(sandbox_scope)
        };

        let base_url = provider.base_url().to_string();
        registry.register(Box::new(provider));
        tracing::info!(url = %base_url, "Kokoro TTS provider registered");
    }

    fn register_silero_tts_provider(
        &self,
        registry: &mut ToolRegistry,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        let config = crate::agent::providers::silero_tts::SileroTtsConfig::from_env();

        if let Ok(url) = std::env::var("SILERO_TTS_URL") {
            if url.trim().is_empty() {
                tracing::debug!(
                    "Silero TTS provider disabled: SILERO_TTS_URL is explicitly set to empty string"
                );
                return;
            }
        }

        tracing::debug!(url = %config.base_url, "Registering Silero TTS provider");
        let sandbox_scope = self.session.sandbox_scope().clone();

        let provider = if let Some(tx) = progress_tx {
            crate::agent::providers::silero_tts::SileroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
                .with_progress_tx(tx.clone())
        } else {
            crate::agent::providers::silero_tts::SileroTtsProvider::from_config(config)
                .with_sandbox_scope(sandbox_scope)
        };

        let base_url = provider.base_url().to_string();
        registry.register(Box::new(provider));
        tracing::info!(url = %base_url, "Silero TTS provider registered");
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
            &self.compaction_service,
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
                self.session.fail(error.to_string());
                Err(error)
            }
            TimedRunResult::TimedOut => {
                self.session.timeout();
                Err(anyhow!(timeout_error_message))
            }
        }
    }

    async fn prepare_execution(
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

        PreparedExecution {
            todos_arc,
            registry,
            tools,
            system_prompt,
            messages: AgentRunner::convert_memory_to_messages(self.session.memory.get_messages()),
            runner_config: AgentRunnerConfig::new(
                model.id.clone(),
                get_agent_max_iterations(),
                crate::config::AGENT_CONTINUATION_LIMIT,
                self.settings.get_agent_timeout_secs(),
                model.max_output_tokens,
            )
            .with_model_provider(model.provider.clone())
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
        let tool_result = {
            let mut tool_ctx = ToolExecutionContext {
                registry: &prepared.registry,
                progress_tx,
                todos_arc: &prepared.todos_arc,
                messages: &mut prepared.messages,
                agent: &mut self.session,
                cancellation_token,
            };
            execute_single_tool_call(tool_call, &mut tool_ctx).await?
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
        compaction_service: &'a CompactionService,
    ) -> AgentRunnerContext<'a> {
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
            compaction_service: Some(compaction_service),
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

    async fn await_until_cancelled<T, F>(
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

    /// Manually compact the current Agent Mode hot context without running a task iteration.
    pub async fn compact_current_context(
        &mut self,
        progress_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Result<CompactionOutcome> {
        let task = self
            .last_task()
            .map(str::to_string)
            .unwrap_or_else(|| "Continue the current Agent Mode session".to_string());
        let todos_arc = Arc::new(Mutex::new(self.session.memory.todos.clone()));
        let registry = self.build_tool_registry(Arc::clone(&todos_arc), progress_tx.as_ref());
        let tools = self
            .execution_profile
            .tool_policy()
            .filter_definitions(registry.all_tools());
        let model = self.settings.get_configured_agent_model();
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let system_prompt = create_agent_system_prompt(
            &task,
            &tools,
            structured_output,
            self.skill_registry.as_mut(),
            &mut self.session,
            self.execution_profile.prompt_instructions(),
        )
        .await;
        let request = CompactionRequest::new(
            CompactionTrigger::Manual,
            &task,
            &system_prompt,
            &tools,
            &model.id,
            model.max_output_tokens,
            false,
        );

        warn!(
            model = %model.id,
            tool_count = tools.len(),
            task_len = task.len(),
            system_prompt_len = system_prompt.len(),
            "Manual compaction requested"
        );
        Self::emit_manual_compaction_started(progress_tx.as_ref()).await;
        let cancellation_token = self.session.cancellation_token.clone();
        let outcome = match Self::await_until_cancelled(
            cancellation_token,
            self.compaction_service
                .prepare_for_run(&request, &mut self.session),
        )
        .await
        {
            Some(Ok(outcome)) => outcome,
            Some(Err(error)) => {
                warn!(error = %error, "Manual compaction failed");
                Self::emit_manual_compaction_failed(progress_tx.as_ref(), error.to_string()).await;
                return Err(error);
            }
            None => {
                if let Some(tx) = progress_tx.as_ref() {
                    let _ = tx.send(AgentEvent::Cancelled).await;
                }
                return Err(anyhow!("Task cancelled by user"));
            }
        };
        warn!(
            applied = outcome.applied,
            budget_state = ?outcome.budget.state,
            hot_memory_tokens_before = outcome.token_count_before,
            hot_memory_tokens_after = outcome.token_count_after,
            collapsed_retry_attempts = outcome.error_retry_collapse.collapsed_attempt_count,
            collapsed_retry_messages = outcome.error_retry_collapse.dropped_message_count,
            externalized_count = outcome.externalization.externalized_count,
            pruned_count = outcome.pruning.pruned_count,
            reclaimed_tokens = outcome.reclaimed_hot_memory_tokens(),
            cleanup_reclaimed_tokens = outcome.reclaimed_cleanup_tokens(),
            summary_attempted = outcome.summary_generation.attempted,
            summary_used_fallback = outcome.summary_generation.used_fallback,
            archived_chunk_count = outcome.archive_persistence.archived_chunk_count,
            summary_updated = outcome.rebuild.inserted_summary,
            "Manual compaction completed"
        );
        if outcome.pruning.applied {
            Self::emit_manual_pruning_applied(progress_tx.as_ref(), &outcome).await;
        }
        Self::emit_manual_compaction_completed(progress_tx.as_ref(), &outcome).await;
        Ok(outcome)
    }

    /// Check if the task has been cancelled
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.session.cancellation_token.is_cancelled()
    }

    fn agent_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.settings.get_agent_timeout_secs())
    }

    fn agent_timeout_error_message(&self) -> String {
        let limit_mins = self.settings.get_agent_timeout_secs() / 60;
        format!("Task exceeded timeout limit ({limit_mins} minutes)")
    }

    /// Reset the executor and session
    pub fn reset(&mut self) {
        self.session.reset();
        self.runner.reset();
    }

    /// Check if the session is timed out
    #[must_use]
    pub fn is_timed_out(&self) -> bool {
        self.session.is_processing()
            && self.session.elapsed_secs() >= self.settings.get_agent_timeout_secs()
    }

    async fn emit_manual_compaction_started(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionStarted {
                    trigger: CompactionTrigger::Manual,
                })
                .await;
        }
    }

    async fn emit_manual_pruning_applied(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::PruningApplied {
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.pruning.reclaimed_tokens,
                })
                .await;
        }
    }

    async fn emit_manual_compaction_completed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        outcome: &CompactionOutcome,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionCompleted {
                    trigger: CompactionTrigger::Manual,
                    applied: outcome.applied,
                    externalized_count: outcome.externalization.externalized_count,
                    pruned_count: outcome.pruning.pruned_count,
                    reclaimed_tokens: outcome.reclaimed_hot_memory_tokens(),
                    archived_chunk_count: outcome.archive_persistence.archived_chunk_count,
                    summary_updated: outcome.rebuild.inserted_summary,
                })
                .await;
        }
    }

    async fn emit_manual_compaction_failed(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        error: String,
    ) {
        if let Some(tx) = progress_tx {
            let _ = tx
                .send(AgentEvent::CompactionFailed {
                    trigger: CompactionTrigger::Manual,
                    error,
                })
                .await;
        }
    }

    /// Emit a milestone event for latency tracking.
    async fn emit_milestone(
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        name: &str,
    ) {
        if let Some(tx) = progress_tx {
            let timestamp_ms = chrono::Utc::now().timestamp_millis();
            let _ = tx
                .send(AgentEvent::Milestone {
                    name: name.to_string(),
                    timestamp_ms,
                })
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    // Allow clone_on_ref_ptr in tests due to trait object coercion requirements
    #![allow(clippy::clone_on_ref_ptr)]

    use super::{AgentExecutor, PolicyControlledHook};
    use crate::agent::hooks::{Hook, HookContext, HookEvent, HookResult};
    use crate::agent::profile::HookAccessPolicy;
    use crate::agent::providers::TodoList;
    use crate::agent::providers::{
        ForumTopicActionResult, ForumTopicCreateRequest, ForumTopicCreateResult,
        ForumTopicEditRequest, ForumTopicEditResult, ForumTopicThreadRequest,
        ManagerTopicLifecycle,
    };
    use crate::agent::session::{AgentSession, PendingUserInput, UserInputKind};
    use crate::config::AgentSettings;
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

    fn build_executor_with_timeout(agent_timeout_secs: u64) -> AgentExecutor {
        let settings = Arc::new(AgentSettings {
            agent_timeout_secs: Some(agent_timeout_secs),
            ..AgentSettings::default()
        });
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let session = AgentSession::new(9_i64.into());
        AgentExecutor::new(llm, session, settings)
    }

    fn build_executor_with_mock_response(response_text: &'static str) -> AgentExecutor {
        let settings = Arc::new(crate::config::AgentSettings {
            agent_model_id: Some("mock-model".to_string()),
            agent_model_provider: Some("mock".to_string()),
            ..crate::config::AgentSettings::default()
        });
        let mut provider = crate::llm::MockLlmProvider::new();
        provider.expect_chat_with_tools().return_once(move |_| {
            Ok(crate::llm::ChatResponse {
                content: Some(response_text.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
        provider
            .expect_chat_completion()
            .returning(|_, _, _, _, _| {
                Err(crate::llm::LlmError::Unknown("Not implemented".to_string()))
            });
        provider
            .expect_transcribe_audio()
            .returning(|_, _, _| Err(crate::llm::LlmError::Unknown("Not implemented".to_string())));
        provider.expect_analyze_image().returning(|_, _, _, _| {
            Err(crate::llm::LlmError::Unknown("Not implemented".to_string()))
        });
        let mut llm = LlmClient::new(settings.as_ref());
        llm.register_provider("mock".to_string(), Arc::new(provider));
        let session = AgentSession::new(9_i64.into());
        AgentExecutor::new(Arc::new(llm), session, settings)
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

    #[test]
    fn hard_timeout_uses_configured_duration_and_message() {
        let executor = build_executor_with_timeout(36_000);

        assert_eq!(
            executor.agent_timeout_duration(),
            std::time::Duration::from_secs(36_000)
        );
        assert_eq!(
            executor.agent_timeout_error_message(),
            "Task exceeded timeout limit (600 minutes)"
        );
    }

    #[test]
    fn executor_timeout_check_uses_configured_value_and_ignores_idle_sessions() {
        let mut executor = build_executor_with_timeout(0);

        executor.session_mut().start_task();
        assert!(executor.is_timed_out());

        executor.reset();
        assert!(!executor.is_timed_out());
    }

    #[test]
    fn resume_with_user_input_clears_pending_request_and_queues_context() {
        let mut executor = build_executor();
        executor
            .session_mut()
            .set_pending_user_input(PendingUserInput {
                kind: crate::agent::UserInputKind::Text,
                prompt: "Reply with details".to_string(),
            });

        assert!(executor.resume_with_user_input("Here are the details".to_string()));
        assert!(executor.session().pending_user_input().is_none());

        let pending = executor.session().drain_runtime_context();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content, "Here are the details");
    }

    #[test]
    fn resume_with_user_input_is_noop_without_pending_request() {
        let mut executor = build_executor();

        assert!(!executor.resume_with_user_input("ignored".to_string()));
        assert!(executor.session().drain_runtime_context().is_empty());
    }

    #[tokio::test]
    async fn resume_after_user_input_continues_saved_task_without_new_user_task() {
        let mut executor = build_executor_with_mock_response(
            r#"{"thought":"done","tool_call":null,"final_answer":"resumed ok","awaiting_user_input":null}"#,
        );
        executor.session_mut().remember_task("original task");
        executor
            .session_mut()
            .memory
            .add_message(crate::agent::memory::AgentMessage::user_task(
                "original task",
            ));
        executor
            .session_mut()
            .set_pending_user_input(PendingUserInput {
                kind: UserInputKind::Text,
                prompt: "Need more details".to_string(),
            });

        let result = executor
            .resume_after_user_input("extra details".to_string(), None)
            .await;

        assert!(matches!(
            result,
            Ok(super::AgentExecutionOutcome::Completed(ref answer)) if answer == "resumed ok"
        ));
        assert!(executor.session().pending_user_input().is_none());

        let user_task_count = executor
            .session()
            .memory
            .get_messages()
            .iter()
            .filter(|message| message.kind == crate::agent::compaction::AgentMessageKind::UserTask)
            .count();
        assert_eq!(user_task_count, 1);

        let runtime_context = executor.session().drain_runtime_context();
        assert!(runtime_context.is_empty());
    }

    #[tokio::test]
    async fn resume_after_user_input_rejects_sessions_without_pending_request() {
        let mut executor = build_executor();
        executor.session_mut().remember_task("original task");

        let error = match executor
            .resume_after_user_input("extra details".to_string(), None)
            .await
        {
            Ok(_) => panic!("resume should fail without pending request"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("not waiting for user input"));
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
    async fn main_agent_registry_includes_explicit_media_and_tts_file_tools() {
        let executor = build_executor();
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        for tool in [
            "transcribe_audio_file",
            "describe_image_file",
            "describe_video_file",
            "text_to_speech_en_file",
            "text_to_speech_ru_file",
        ] {
            assert!(registry.can_handle(tool), "missing registry tool: {tool}");
        }

        let tool_names = registry
            .all_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(tool_names.contains("transcribe_audio_file"));
        assert!(tool_names.contains("describe_video_file"));
        assert!(tool_names.contains("text_to_speech_en_file"));
        assert!(tool_names.contains("text_to_speech_ru_file"));
    }

    #[tokio::test]
    async fn agents_md_context_enables_self_editing_tools() {
        let mut mock = MockStorageProvider::new();
        mock.expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .returning(|user_id, topic_id| {
                Ok(Some(crate::storage::TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 4,
                    user_id,
                    topic_id,
                    agents_md: "# Topic AGENTS\nCurrent instructions".to_string(),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let mut executor = build_executor();
        executor.set_agents_md_context(Arc::new(mock), 77, "topic-a".to_string());
        let registry = executor.build_tool_registry(Arc::new(Mutex::new(TodoList::new())), None);

        let response = registry
            .execute("agents_md_get", "{}", None, None)
            .await
            .expect("agents_md_get must succeed when context is configured");

        let parsed: serde_json::Value =
            serde_json::from_str(&response).expect("tool response must be valid json");
        assert_eq!(parsed["found"], true);
        assert_eq!(parsed["topic_id"], "topic-a");
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
