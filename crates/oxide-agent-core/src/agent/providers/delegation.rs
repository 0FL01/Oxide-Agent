//! Delegation provider for sub-agent execution.
//!
//! Exposes `delegate_to_sub_agent` tool that runs an isolated agent loop
//! with a lightweight model and restricted toolset.

use crate::agent::compaction::{
    CompactionService, CompactionSummarizer, CompactionSummarizerConfig,
};
use crate::agent::context::{AgentContext, EphemeralSession};
use crate::agent::hooks::{
    CompletionCheckHook, SearchBudgetHook, SubAgentSafetyConfig, SubAgentSafetyHook,
    TimeoutReportHook,
};
use crate::agent::memory::{AgentMemory, AgentMessage, MessageRole};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_sub_agent_system_prompt;
use crate::agent::provider::ToolProvider;
use crate::agent::providers::{
    FileHosterProvider, SandboxProvider, TodoList, TodosProvider, YtdlpProvider,
};
use crate::agent::registry::ToolRegistry;
use crate::agent::runner::{AgentRunResult, AgentRunner, AgentRunnerConfig, AgentRunnerContext};
use crate::config::{
    get_agent_search_limit, get_sub_agent_max_iterations, AGENT_CONTINUATION_LIMIT,
};
use crate::llm::{Message, ToolDefinition};
use crate::sandbox::SandboxScope;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};
use tracing::{info, warn};
use uuid::Uuid;

#[cfg(feature = "browser_use")]
use crate::agent::providers::BrowserUseProvider;
#[cfg(feature = "crawl4ai")]
use crate::agent::providers::Crawl4aiProvider;
#[cfg(feature = "searxng")]
use crate::agent::providers::SearxngProvider;
#[cfg(feature = "tavily")]
use crate::agent::providers::TavilyProvider;
use tokio::sync::Semaphore;

const BLOCKED_SUB_AGENT_TOOLS: &[&str] = &[
    "delegate_to_sub_agent",
    "send_file_to_user",
    "ssh_send_file_to_user",
    "transcribe_audio_file",
    "describe_image_file",
    "describe_video_file",
    "text_to_speech_en",
    "text_to_speech_en_file",
    "text_to_speech_ru",
    "text_to_speech_ru_file",
    "recreate_sandbox",
    "topic_binding_set",
    "topic_binding_get",
    "topic_binding_delete",
    "topic_binding_rollback",
    "topic_context_upsert",
    "topic_context_get",
    "topic_context_delete",
    "topic_context_rollback",
    "topic_agents_md_upsert",
    "topic_agents_md_get",
    "topic_agents_md_delete",
    "topic_agents_md_rollback",
    "agents_md_get",
    "agents_md_update",
    "private_secret_probe",
    "topic_infra_upsert",
    "topic_infra_get",
    "topic_infra_delete",
    "topic_infra_rollback",
    "agent_profile_upsert",
    "agent_profile_get",
    "agent_profile_delete",
    "agent_profile_rollback",
    "forum_topic_provision_ssh_agent",
    "forum_topic_create",
    "forum_topic_edit",
    "forum_topic_close",
    "forum_topic_reopen",
    "forum_topic_delete",
    "forum_topic_list",
    "reminder_schedule",
    "reminder_list",
    "reminder_cancel",
    "reminder_pause",
    "reminder_resume",
    "reminder_retry",
    // Jira write operations blocked for sub-agents (read-only access allowed)
    "jira_write",
];
const SUB_AGENT_REPORT_MAX_MESSAGES: usize = 6;
const SUB_AGENT_REPORT_MAX_CHARS: usize = 800;

/// Provider for sub-agent delegation tool.
pub struct DelegationProvider {
    llm_client: Arc<crate::llm::LlmClient>,
    sandbox_scope: SandboxScope,
    settings: Arc<crate::config::AgentSettings>,
    browser_use_profile_scope: Option<String>,
    /// Semaphore to limit concurrent crawl4ai requests per sub-agent.
    /// Used via Arc::clone() in build_sub_agent_providers().
    #[allow(dead_code)]
    crawl4ai_semaphore: Arc<Semaphore>,
    /// Semaphore to limit concurrent Browser Use requests per sub-agent.
    /// Used via Arc::clone() in build_sub_agent_providers().
    #[allow(dead_code)]
    browser_use_semaphore: Arc<Semaphore>,
}

struct PreparedSubAgentExecution {
    task_id: String,
    task: String,
    registry: ToolRegistry,
    tools: Vec<ToolDefinition>,
    system_prompt: String,
    todos_arc: Arc<Mutex<TodoList>>,
    messages: Vec<Message>,
    sub_session: EphemeralSession,
    runner_config: AgentRunnerConfig,
    compaction_service: CompactionService,
    progress_tx: Option<mpsc::Sender<AgentEvent>>,
    progress_relay_task: Option<JoinHandle<()>>,
}

enum SubAgentRunOutcome {
    Final(String),
    WaitingForApproval,
    Failed(anyhow::Error),
    TimedOut,
}

impl DelegationProvider {
    /// Create a new delegation provider.
    #[must_use]
    pub fn new(
        llm_client: Arc<crate::llm::LlmClient>,
        sandbox_scope: impl Into<SandboxScope>,
        settings: Arc<crate::config::AgentSettings>,
    ) -> Self {
        Self {
            llm_client,
            sandbox_scope: sandbox_scope.into(),
            settings,
            browser_use_profile_scope: None,
            crawl4ai_semaphore: Arc::new(Semaphore::new(
                crate::config::get_crawl4ai_max_concurrent(),
            )),
            browser_use_semaphore: Arc::new(Semaphore::new(
                crate::config::get_browser_use_max_concurrent(),
            )),
        }
    }

    /// Inherit a topic/context profile scope for Browser Use sub-agent reuse.
    #[must_use]
    pub fn with_browser_use_profile_scope(mut self, profile_scope: impl Into<String>) -> Self {
        let profile_scope = profile_scope.into();
        if !profile_scope.trim().is_empty() {
            self.browser_use_profile_scope = Some(profile_scope);
        }
        self
    }

    fn blocked_tool_set() -> HashSet<String> {
        BLOCKED_SUB_AGENT_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    }

    fn build_sub_agent_providers(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Vec<Box<dyn ToolProvider>> {
        let sandbox_provider = if let Some(tx) = progress_tx {
            SandboxProvider::new(self.sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            SandboxProvider::new(self.sandbox_scope.clone())
        };
        let ytdlp_provider = if let Some(tx) = progress_tx {
            YtdlpProvider::new(self.sandbox_scope.clone()).with_progress_tx(tx.clone())
        } else {
            YtdlpProvider::new(self.sandbox_scope.clone())
        };

        let mut providers: Vec<Box<dyn ToolProvider>> = vec![
            Box::new(TodosProvider::new(todos_arc)),
            Box::new(sandbox_provider),
            Box::new(FileHosterProvider::new(self.sandbox_scope.clone())),
            Box::new(ytdlp_provider),
        ];

        #[cfg(feature = "tavily")]
        if crate::config::is_tavily_enabled() {
            if let Ok(tavily_key) = std::env::var("TAVILY_API_KEY") {
                if !tavily_key.trim().is_empty() {
                    if let Ok(provider) = TavilyProvider::new(&tavily_key) {
                        providers.push(Box::new(provider));
                    }
                } else {
                    warn!("Tavily enabled but TAVILY_API_KEY is empty; sub-agent provider not registered");
                }
            } else {
                warn!("Tavily enabled but TAVILY_API_KEY is not set; sub-agent provider not registered");
            }
        }
        #[cfg(not(feature = "tavily"))]
        if crate::config::is_tavily_enabled() {
            warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(feature = "searxng")]
        if crate::config::is_searxng_enabled() {
            if let Some(url) = crate::config::get_searxng_url() {
                if !url.trim().is_empty() {
                    match SearxngProvider::new(&url) {
                        Ok(provider) => providers.push(Box::new(provider)),
                        Err(error) => {
                            warn!(error = %error, "SearXNG sub-agent provider initialization failed")
                        }
                    }
                } else {
                    warn!("SearXNG enabled but SEARXNG_URL is empty; sub-agent provider not registered");
                }
            } else {
                warn!(
                    "SearXNG enabled but SEARXNG_URL is not set; sub-agent provider not registered"
                );
            }
        }
        #[cfg(not(feature = "searxng"))]
        if crate::config::is_searxng_enabled() {
            warn!("SearXNG enabled but feature not compiled in");
        }

        #[cfg(feature = "crawl4ai")]
        if crate::config::is_crawl4ai_enabled() {
            if let Some(url) = crate::config::get_crawl4ai_url() {
                if !url.trim().is_empty() {
                    let sem = Arc::clone(&self.crawl4ai_semaphore);
                    providers.push(Box::new(Crawl4aiProvider::new_with_semaphore(&url, sem)));
                } else {
                    warn!("Crawl4AI enabled but CRAWL4AI_URL is empty; sub-agent provider not registered");
                }
            } else {
                warn!("Crawl4AI enabled but CRAWL4AI_URL is not set; sub-agent provider not registered");
            }
        }
        #[cfg(not(feature = "crawl4ai"))]
        if crate::config::is_crawl4ai_enabled() {
            warn!("Crawl4AI enabled but feature not compiled in");
        }

        #[cfg(feature = "browser_use")]
        self.maybe_push_browser_use_provider(&mut providers);
        #[cfg(not(feature = "browser_use"))]
        if crate::config::is_browser_use_enabled() {
            warn!("Browser Use enabled but feature not compiled in");
        }

        providers
    }

    #[cfg(feature = "browser_use")]
    fn maybe_push_browser_use_provider(&self, providers: &mut Vec<Box<dyn ToolProvider>>) {
        if !crate::config::is_browser_use_enabled() {
            return;
        }

        if let Some(url) = crate::config::get_browser_use_url() {
            if !url.trim().is_empty() {
                let sem = Arc::clone(&self.browser_use_semaphore);
                let mut provider =
                    BrowserUseProvider::new_with_semaphore(&url, Arc::clone(&self.settings), sem);
                if let Some(profile_scope) = &self.browser_use_profile_scope {
                    provider = provider.with_profile_scope(profile_scope.clone());
                }
                provider = provider.with_sandbox_scope(self.sandbox_scope.clone());
                providers.push(Box::new(provider));
            } else {
                warn!("Browser Use enabled but BROWSER_USE_URL is empty; sub-agent provider not registered");
            }
        } else {
            warn!("Browser Use enabled but BROWSER_USE_URL is not set; sub-agent provider not registered");
        }
    }

    fn build_registry(
        &self,
        allowed_tools: &HashSet<String>,
        providers: Vec<Box<dyn ToolProvider>>,
    ) -> ToolRegistry {
        let allowed = Arc::new(allowed_tools.clone());
        let mut registry = ToolRegistry::new();
        for provider in providers {
            registry.register(Box::new(RestrictedToolProvider::new(
                provider,
                Arc::clone(&allowed),
            )));
        }
        registry
    }

    fn filter_allowed_tools(
        &self,
        requested_tools: Vec<String>,
        available_tools: &HashSet<String>,
        task_id: &str,
    ) -> Result<HashSet<String>> {
        let blocked = Self::blocked_tool_set();
        let requested: HashSet<String> = requested_tools.into_iter().collect();
        let allowed: HashSet<String> = requested
            .iter()
            .filter(|name| !blocked.contains(*name))
            .filter(|name| available_tools.contains(*name))
            .cloned()
            .collect();

        if allowed.is_empty() {
            warn!(
                task_id = %task_id,
                requested = ?requested,
                available = ?available_tools,
                "No allowed tools left after filtering"
            );
            return Err(anyhow!(
                "No allowed tools left after filtering (blocked or unavailable). Requested: {:?}, Available: {:?}",
                requested,
                available_tools
            ));
        }
        Ok(allowed)
    }

    fn create_sub_agent_runner(&self, blocked: HashSet<String>, max_tokens: usize) -> AgentRunner {
        let max_iterations = get_sub_agent_max_iterations();
        let mut runner = AgentRunner::new(Arc::clone(&self.llm_client));
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(SubAgentSafetyHook::new(SubAgentSafetyConfig {
            max_iterations,
            max_tokens,
            blocked_tools: blocked,
        })));
        runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
        runner.register_hook(Box::new(TimeoutReportHook::new()));
        runner
    }

    fn create_sub_agent_compaction_service(&self) -> CompactionService {
        let (_, _, _, timeout_secs) = self.settings.get_configured_compaction_model();
        CompactionService::default().with_summarizer(CompactionSummarizer::new(
            Arc::clone(&self.llm_client),
            CompactionSummarizerConfig {
                model_routes: self.settings.get_configured_compaction_model_routes(true),
                timeout_secs,
                ..CompactionSummarizerConfig::default()
            },
        ))
    }

    fn parse_delegate_args(arguments: &str) -> Result<DelegateToSubAgentArgs> {
        let args: DelegateToSubAgentArgs = serde_json::from_str(arguments)?;
        if args.task.trim().is_empty() {
            return Err(anyhow!("Sub-agent task cannot be empty"));
        }
        if args.tools.is_empty() {
            return Err(anyhow!("Sub-agent tools whitelist cannot be empty"));
        }
        Ok(args)
    }

    fn build_sub_agent_session(
        task: &str,
        max_tokens: usize,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> EphemeralSession {
        let mut sub_session = match cancellation_token {
            Some(parent_token) => EphemeralSession::with_parent_token(max_tokens, parent_token),
            None => EphemeralSession::new(max_tokens),
        };
        sub_session
            .memory_mut()
            .add_message(AgentMessage::user_task(task));
        sub_session
    }

    fn build_sub_agent_runner_config(&self, model: &crate::config::ModelInfo) -> AgentRunnerConfig {
        AgentRunnerConfig::new(
            model.id.clone(),
            get_sub_agent_max_iterations(),
            AGENT_CONTINUATION_LIMIT,
            self.settings.get_sub_agent_timeout_secs(),
            model.max_output_tokens,
        )
        .with_model_provider(model.provider.clone())
        .with_model_routes(self.settings.get_configured_sub_agent_model_routes())
        .with_sub_agent(true)
    }

    fn prepare_sub_agent_execution(
        &self,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<PreparedSubAgentExecution> {
        let DelegateToSubAgentArgs {
            task,
            tools: requested_tools,
            context,
        } = Self::parse_delegate_args(arguments)?;

        let task_id = format!("sub-{}", Uuid::new_v4());
        let model_routes = self.settings.get_configured_sub_agent_model_routes();
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_sub_agent_model());
        let sub_agent_context_budget = self.settings.get_sub_agent_internal_context_budget_tokens();
        let sub_session = Self::build_sub_agent_session(
            task.as_str(),
            sub_agent_context_budget,
            cancellation_token,
        );
        let todos_arc = Arc::new(Mutex::new(sub_session.memory().todos.clone()));
        let (sub_agent_progress_tx, progress_relay_task) =
            spawn_sub_agent_progress_relay(progress_tx);
        let providers =
            self.build_sub_agent_providers(Arc::clone(&todos_arc), sub_agent_progress_tx.as_ref());
        let available_tools: HashSet<String> = providers
            .iter()
            .flat_map(|provider| provider.tools())
            .map(|tool| tool.name)
            .collect();
        let allowed = self.filter_allowed_tools(requested_tools, &available_tools, &task_id)?;
        let registry = self.build_registry(&allowed, providers);
        let tools = registry.all_tools();
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let system_prompt = create_sub_agent_system_prompt(
            task.as_str(),
            &tools,
            structured_output,
            context.as_deref(),
        );

        Ok(PreparedSubAgentExecution {
            task_id,
            task,
            registry,
            tools,
            system_prompt,
            todos_arc,
            messages: AgentRunner::convert_memory_to_messages(sub_session.memory().get_messages()),
            sub_session,
            runner_config: self.build_sub_agent_runner_config(&model),
            compaction_service: self.create_sub_agent_compaction_service(),
            progress_tx: sub_agent_progress_tx,
            progress_relay_task,
        })
    }

    fn build_sub_agent_runner_context<'a>(
        prepared: &'a mut PreparedSubAgentExecution,
    ) -> AgentRunnerContext<'a> {
        AgentRunnerContext {
            task: prepared.task.as_str(),
            system_prompt: &prepared.system_prompt,
            tools: &prepared.tools,
            registry: &prepared.registry,
            progress_tx: prepared.progress_tx.as_ref(),
            todos_arc: &prepared.todos_arc,
            task_id: &prepared.task_id,
            messages: &mut prepared.messages,
            agent: &mut prepared.sub_session,
            skill_registry: None,
            compaction_service: Some(&prepared.compaction_service),
            config: prepared.runner_config.clone(),
        }
    }

    async fn run_sub_agent_with_timeout(
        &self,
        runner: &mut AgentRunner,
        ctx: &mut AgentRunnerContext<'_>,
    ) -> SubAgentRunOutcome {
        match timeout(self.sub_agent_timeout_duration(), runner.run(ctx)).await {
            Ok(Ok(AgentRunResult::Final(result))) => SubAgentRunOutcome::Final(result),
            Ok(Ok(AgentRunResult::WaitingForApproval)) => SubAgentRunOutcome::WaitingForApproval,
            Ok(Ok(AgentRunResult::WaitingForUserInput(_))) => {
                SubAgentRunOutcome::WaitingForApproval
            }
            Ok(Err(error)) => SubAgentRunOutcome::Failed(error),
            Err(_) => SubAgentRunOutcome::TimedOut,
        }
    }

    fn sub_agent_timeout_duration(&self) -> Duration {
        Duration::from_secs(self.settings.get_sub_agent_timeout_secs() + 30)
    }

    fn build_sub_agent_error_report(
        &self,
        task_id: &str,
        memory: &AgentMemory,
        error: String,
    ) -> String {
        build_sub_agent_report(SubAgentReportContext {
            task_id,
            status: SubAgentReportStatus::Error,
            error: Some(error),
            memory,
            timeout_secs: self.settings.get_sub_agent_timeout_secs(),
        })
    }

    fn build_sub_agent_timeout_report(&self, task_id: &str, memory: &AgentMemory) -> String {
        let limit = self.settings.get_sub_agent_timeout_secs();
        build_sub_agent_report(SubAgentReportContext {
            task_id,
            status: SubAgentReportStatus::Timeout,
            error: Some(format!(
                "Sub-agent hard timed out after {} seconds",
                limit + 30
            )),
            memory,
            timeout_secs: limit,
        })
    }

    async fn finish_sub_agent_progress_relay(prepared: &mut PreparedSubAgentExecution) {
        // Drop the restricted providers before awaiting the relay task.
        // Some providers retain cloned progress senders internally; if the
        // registry stays alive while we await the relay, the sub-agent channel
        // never closes and the relay task cannot finish.
        prepared.tools.clear();
        prepared.registry = ToolRegistry::new();
        drop(prepared.progress_tx.take());

        if let Some(task) = prepared.progress_relay_task.take() {
            let _ = task.await;
        }
    }
}

#[async_trait]
impl ToolProvider for DelegationProvider {
    fn name(&self) -> &'static str {
        "delegation"
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "delegate_to_sub_agent".to_string(),
            description: "Delegate rough work to lightweight sub-agent. \
Pass a short, clear task and a list of allowed tools. \
You can add additional context (e.g., a quote from a skill). \
If the sub-agent doesn't finish, a partial report will be returned."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Task for sub-agent"
                    },
                    "tools": {
                        "type": "array",
                        "description": "Whitelist of allowed tools",
                        "items": {"type": "string"}
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional context (optional)"
                    }
                },
                "required": ["task", "tools"]
            }),
        }]
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        tool_name == "delegate_to_sub_agent"
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if tool_name != "delegate_to_sub_agent" {
            return Err(anyhow!("Unknown delegation tool: {tool_name}"));
        }

        let mut prepared =
            self.prepare_sub_agent_execution(arguments, progress_tx, cancellation_token)?;
        let mut runner = self.create_sub_agent_runner(
            Self::blocked_tool_set(),
            prepared.sub_session.memory().max_tokens(),
        );
        info!(task_id = %prepared.task_id, "Running sub-agent delegation");

        let outcome = {
            let mut ctx = Self::build_sub_agent_runner_context(&mut prepared);
            self.run_sub_agent_with_timeout(&mut runner, &mut ctx).await
        };
        Self::finish_sub_agent_progress_relay(&mut prepared).await;

        match outcome {
            SubAgentRunOutcome::Final(result) => Ok(result),
            SubAgentRunOutcome::WaitingForApproval => {
                warn!(task_id = %prepared.task_id, "Sub-agent paused waiting for unsupported approval");
                Ok(self.build_sub_agent_error_report(
                    &prepared.task_id,
                    prepared.sub_session.memory(),
                    "sub-agent paused waiting for unsupported external approval".to_string(),
                ))
            }
            SubAgentRunOutcome::Failed(err) => {
                warn!(task_id = %prepared.task_id, error = %err, "Sub-agent failed");
                Ok(self.build_sub_agent_error_report(
                    &prepared.task_id,
                    prepared.sub_session.memory(),
                    err.to_string(),
                ))
            }
            SubAgentRunOutcome::TimedOut => {
                warn!(task_id = %prepared.task_id, "Sub-agent hard timed out");
                Ok(self.build_sub_agent_timeout_report(
                    &prepared.task_id,
                    prepared.sub_session.memory(),
                ))
            }
        }
    }
}

fn spawn_sub_agent_progress_relay(
    parent_tx: Option<&mpsc::Sender<AgentEvent>>,
) -> (Option<mpsc::Sender<AgentEvent>>, Option<JoinHandle<()>>) {
    let Some(parent_tx) = parent_tx.cloned() else {
        return (None, None);
    };

    let (tx, mut rx) = mpsc::channel(100);
    let relay = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if !should_forward_sub_agent_progress_event(&event) {
                continue;
            }

            if parent_tx.send(event).await.is_err() {
                break;
            }
        }
    });

    (Some(tx), Some(relay))
}

fn should_forward_sub_agent_progress_event(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::Thinking { .. }
            | AgentEvent::TokenSnapshotUpdated { .. }
            | AgentEvent::FileToSend { .. }
            | AgentEvent::FileToSendWithConfirmation { .. }
    )
}

#[derive(Debug, Deserialize)]
struct DelegateToSubAgentArgs {
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    context: Option<String>,
}

struct RestrictedToolProvider {
    inner: Box<dyn ToolProvider>,
    allowed_tools: Arc<HashSet<String>>,
}

impl RestrictedToolProvider {
    fn new(inner: Box<dyn ToolProvider>, allowed_tools: Arc<HashSet<String>>) -> Self {
        Self {
            inner,
            allowed_tools,
        }
    }
}

#[async_trait]
impl ToolProvider for RestrictedToolProvider {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn tools(&self) -> Vec<ToolDefinition> {
        self.inner
            .tools()
            .into_iter()
            .filter(|tool| self.allowed_tools.contains(&tool.name))
            .collect()
    }

    fn can_handle(&self, tool_name: &str) -> bool {
        self.allowed_tools.contains(tool_name) && self.inner.can_handle(tool_name)
    }

    async fn execute(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        if !self.allowed_tools.contains(tool_name) {
            warn!(tool_name = %tool_name, "Tool blocked by delegation whitelist");
            return Err(anyhow!("Tool '{tool_name}' is not allowed for sub-agent"));
        }

        self.inner
            .execute(tool_name, arguments, progress_tx, cancellation_token)
            .await
    }
}

enum SubAgentReportStatus {
    Timeout,
    Error,
}

impl SubAgentReportStatus {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Error => "error",
        }
    }
}

struct SubAgentReportContext<'a> {
    task_id: &'a str,
    status: SubAgentReportStatus,
    error: Option<String>,
    memory: &'a AgentMemory,
    timeout_secs: u64,
}

fn build_sub_agent_report(ctx: SubAgentReportContext<'_>) -> String {
    let report = json!({
        "status": ctx.status.as_str(),
        "task_id": ctx.task_id,
        "error": ctx.error,
        "note": "Sub-agent did not finish the task. Use partial results below.",
        "timeout_secs": ctx.timeout_secs,
        "tokens": ctx.memory.token_count(),
        "todos": &ctx.memory.todos,
        "recent_messages": summarize_recent_messages(ctx.memory),
    });

    match serde_json::to_string_pretty(&report) {
        Ok(payload) => payload,
        Err(err) => format!(
            "Sub-agent {}. Failed to serialize report: {err}",
            ctx.status.as_str()
        ),
    }
}

fn summarize_recent_messages(memory: &AgentMemory) -> Vec<serde_json::Value> {
    let mut items = Vec::new();
    for message in memory
        .get_messages()
        .iter()
        .rev()
        .take(SUB_AGENT_REPORT_MAX_MESSAGES)
    {
        let content = crate::utils::truncate_str(&message.content, SUB_AGENT_REPORT_MAX_CHARS);
        let reasoning = message
            .reasoning
            .as_ref()
            .map(|text| crate::utils::truncate_str(text, SUB_AGENT_REPORT_MAX_CHARS));
        items.push(json!({
            "role": role_label(&message.role),
            "content": content,
            "reasoning": reasoning,
            "tool_name": message.tool_name.as_deref(),
            "tool_call_id": message.tool_call_id.as_deref(),
        }));
    }
    items.reverse();
    items
}

fn role_label(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        should_forward_sub_agent_progress_event, spawn_sub_agent_progress_relay, DelegationProvider,
    };
    use crate::agent::compaction::BudgetState;
    use crate::agent::context::AgentContext;
    use crate::agent::progress::{AgentEvent, FileDeliveryKind, TokenSnapshot};
    use crate::agent::providers::TodoList;
    use crate::config::AgentSettings;
    use crate::llm::LlmClient;
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[test]
    fn sub_agent_blocklist_includes_sensitive_tools() {
        let blocked = DelegationProvider::blocked_tool_set();

        for tool in [
            "transcribe_audio_file",
            "describe_image_file",
            "describe_video_file",
            "text_to_speech_en",
            "text_to_speech_en_file",
            "text_to_speech_ru",
            "text_to_speech_ru_file",
            "recreate_sandbox",
            "topic_binding_set",
            "topic_binding_get",
            "topic_binding_delete",
            "topic_binding_rollback",
            "topic_context_upsert",
            "topic_context_get",
            "topic_context_delete",
            "topic_context_rollback",
            "topic_agents_md_upsert",
            "topic_agents_md_get",
            "topic_agents_md_delete",
            "topic_agents_md_rollback",
            "agents_md_get",
            "agents_md_update",
            "private_secret_probe",
            "topic_infra_upsert",
            "topic_infra_get",
            "topic_infra_delete",
            "topic_infra_rollback",
            "agent_profile_upsert",
            "agent_profile_get",
            "agent_profile_delete",
            "agent_profile_rollback",
            "forum_topic_provision_ssh_agent",
            "forum_topic_create",
            "forum_topic_edit",
            "forum_topic_close",
            "forum_topic_reopen",
            "forum_topic_delete",
            "forum_topic_list",
        ] {
            assert!(blocked.contains(tool), "missing blocked tool: {tool}");
        }
    }

    #[test]
    fn filter_allowed_tools_rejects_manager_control_plane_requests() {
        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);
        let available_tools = HashSet::from([
            "write_todos".to_string(),
            "transcribe_audio_file".to_string(),
            "text_to_speech_en".to_string(),
            "text_to_speech_en_file".to_string(),
            "text_to_speech_ru".to_string(),
            "text_to_speech_ru_file".to_string(),
            "forum_topic_create".to_string(),
            "topic_binding_set".to_string(),
            "topic_infra_upsert".to_string(),
        ]);

        let allowed = provider
            .filter_allowed_tools(
                vec![
                    "write_todos".to_string(),
                    "transcribe_audio_file".to_string(),
                    "text_to_speech_en".to_string(),
                    "text_to_speech_en_file".to_string(),
                    "text_to_speech_ru".to_string(),
                    "text_to_speech_ru_file".to_string(),
                    "forum_topic_create".to_string(),
                    "topic_binding_set".to_string(),
                    "topic_infra_upsert".to_string(),
                ],
                &available_tools,
                "test-task",
            )
            .expect("non-manager tool should survive filtering");

        assert_eq!(allowed, HashSet::from(["write_todos".to_string()]));
    }

    #[test]
    fn sub_agent_progress_filter_drops_token_snapshots() {
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::Thinking {
                snapshot: sample_snapshot(64_000),
            }
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::TokenSnapshotUpdated {
                snapshot: sample_snapshot(64_000),
            }
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::FileToSend {
                kind: FileDeliveryKind::Auto,
                file_name: "note.ogg".to_string(),
                content: vec![1, 2, 3],
            }
        ));
        assert!(should_forward_sub_agent_progress_event(
            &AgentEvent::ToolCall {
                name: "execute_command".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
                command_preview: Some("pwd".to_string()),
            }
        ));
    }

    #[tokio::test]
    async fn sub_agent_progress_relay_filters_snapshot_events() {
        let (parent_tx, mut parent_rx) = mpsc::channel(8);
        let (sub_tx, relay_task) = spawn_sub_agent_progress_relay(Some(&parent_tx));
        let sub_tx = sub_tx.expect("relay tx");

        sub_tx
            .send(AgentEvent::Thinking {
                snapshot: sample_snapshot(64_000),
            })
            .await
            .expect("thinking send");
        sub_tx
            .send(AgentEvent::ToolCall {
                name: "execute_command".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
                command_preview: Some("pwd".to_string()),
            })
            .await
            .expect("tool call send");
        sub_tx
            .send(AgentEvent::FileToSend {
                kind: FileDeliveryKind::Auto,
                file_name: "note.ogg".to_string(),
                content: vec![1, 2, 3],
            })
            .await
            .expect("file send");

        drop(sub_tx);
        if let Some(task) = relay_task {
            task.await.expect("relay task join");
        }
        drop(parent_tx);

        let forwarded = parent_rx.recv().await.expect("forwarded event");
        assert!(matches!(forwarded, AgentEvent::ToolCall { .. }));
        assert!(parent_rx.recv().await.is_none());
    }

    #[test]
    fn prepare_sub_agent_execution_applies_sub_agent_budget_policy() {
        let settings = Arc::new(AgentSettings {
            sub_agent_model_id: Some("sub-model".to_string()),
            sub_agent_model_provider: Some("mock".to_string()),
            sub_agent_max_output_tokens: Some(12_345),
            sub_agent_context_window_tokens: Some(96_000),
            ..AgentSettings::default()
        });
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);

        let prepared = provider
            .prepare_sub_agent_execution(
                &json!({
                    "task": "Inspect the workspace.",
                    "tools": ["write_todos"],
                    "context": "Keep notes concise."
                })
                .to_string(),
                None,
                None,
            )
            .expect("sub-agent preparation succeeds");

        assert_eq!(prepared.runner_config.model_name, "sub-model");
        assert_eq!(prepared.runner_config.model_max_output_tokens, 12_345);
        assert_eq!(prepared.sub_session.memory().max_tokens(), 96_000);
    }

    #[cfg(feature = "browser_use")]
    #[test]
    fn build_sub_agent_providers_registers_browser_use_when_enabled() {
        let _guard = crate::config::test_env_mutex()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::env::set_var("BROWSER_USE_URL", "http://browser-use:8000");
        std::env::set_var("BROWSER_USE_ENABLED", "true");

        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);
        let todos = Arc::new(tokio::sync::Mutex::new(TodoList::new()));
        let providers = provider.build_sub_agent_providers(todos, None);
        let tools: HashSet<String> = providers
            .iter()
            .flat_map(|provider| provider.tools())
            .map(|tool| tool.name)
            .collect();

        assert!(tools.contains("browser_use_run_task"));
        assert!(tools.contains("browser_use_get_session"));
        assert!(tools.contains("browser_use_close_session"));
        assert!(tools.contains("browser_use_extract_content"));
        assert!(tools.contains("browser_use_screenshot"));

        std::env::remove_var("BROWSER_USE_ENABLED");
        std::env::remove_var("BROWSER_USE_URL");
    }

    fn sample_snapshot(context_window_tokens: usize) -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens: 777,
            system_prompt_tokens: 500,
            tool_schema_tokens: 600,
            loaded_skill_tokens: 0,
            total_input_tokens: 2_900,
            reserved_output_tokens: 64_000,
            hard_reserve_tokens: 8_192,
            projected_total_tokens: 75_092,
            context_window_tokens,
            headroom_tokens: context_window_tokens.saturating_sub(75_092),
            budget_state: BudgetState::Warning,
            last_api_usage: None,
        }
    }
}
