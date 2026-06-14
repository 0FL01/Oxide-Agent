//! Delegation provider for sub-agent execution.
//!
//! Exposes async sub-agent tools that run isolated agent loops with a restricted toolset.

use crate::agent::compaction::CompactionController;
use crate::agent::context::{AgentContext, EphemeralSession};
use crate::agent::hooks::{
    CompletionCheckHook, HotContextHealthHook, SearchBudgetHook, SubAgentSafetyConfig,
    SubAgentSafetyHook, TimeoutReportHook,
};
use crate::agent::memory::{AgentMemory, AgentMessage, MessageRole};
use crate::agent::progress::AgentEvent;
use crate::agent::prompt::create_sub_agent_system_prompt;
use crate::agent::providers::{SandboxRuntime, TodoList};
use crate::agent::runner::{
    AgentRunner, AgentRunnerConfig, AgentRunnerContext, AgentRunnerContextBase, TimedRunResult,
    run_with_timeout,
};
use crate::agent::session::AgentMemoryScope;
use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolModuleContext, ToolModuleContextParts,
    ToolName, ToolOutput, ToolRegistry as RuntimeToolRegistry, ToolRuntimeConfig, ToolRuntimeError,
};
use crate::config::{
    AgentSettings, get_agent_continuation_limit, get_agent_search_limit,
    get_sub_agent_max_iterations,
};
use crate::llm::{Message, ToolDefinition};
use crate::sandbox::SandboxScope;
use crate::storage::StorageProvider;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tracing::{info, warn};
use uuid::Uuid;

#[cfg(feature = "tool-brave-search")]
use crate::agent::tool_runtime::BraveSearchToolModule;
#[cfg(feature = "tool-crawl4ai-markdown")]
use crate::agent::tool_runtime::Crawl4AiMarkdownToolModule;
#[cfg(feature = "tool-sandbox-exec")]
use crate::agent::tool_runtime::SandboxExecToolModule;
#[cfg(feature = "tool-sandbox-fileops")]
use crate::agent::tool_runtime::SandboxFileOpsToolModule;
#[cfg(feature = "tool-searxng")]
use crate::agent::tool_runtime::SearxngToolModule;
#[cfg(feature = "tool-tavily")]
use crate::agent::tool_runtime::TavilyToolModule;
#[cfg(feature = "tool-todos")]
use crate::agent::tool_runtime::TodosToolModule;
#[cfg(any(
    feature = "tool-sandbox-exec",
    feature = "tool-sandbox-fileops",
    feature = "tool-brave-search",
    feature = "tool-searxng",
    feature = "tool-tavily",
    feature = "tool-todos",
    feature = "tool-crawl4ai-markdown",
    feature = "tool-webfetch-md",
    feature = "tool-ytdlp"
))]
use crate::agent::tool_runtime::ToolModule;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::tool_runtime::WebCrawlerToolModule;
#[cfg(feature = "tool-webfetch-md")]
use crate::agent::tool_runtime::WebFetchMdToolModule;
#[cfg(feature = "tool-ytdlp")]
use crate::agent::tool_runtime::YtdlpToolModule;
use tokio::sync::Semaphore;

const TOOL_SPAWN_SUB_AGENTS: &str = "spawn_sub_agents";
const TOOL_WAIT_SUB_AGENTS: &str = "wait_sub_agents";
const TOOL_CANCEL_SUB_AGENTS: &str = "cancel_sub_agents";
const TOOL_WEB_CRAWLER: &str = "web_crawler";
const TOOL_WEB_MARKDOWN: &str = "web_markdown";
const TOOL_CRAWL4AI_MARKDOWN: &str = "crawl4ai_markdown";
const SUB_AGENT_MAX_CONCURRENT_JOBS: usize = 5;
const SUB_AGENT_DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
const SUB_AGENT_MAX_WAIT_TIMEOUT_MS: u64 = 3_600_000;
const SUB_AGENT_DISPLAY_NAMES: &[&str] = &[
    "investi-gator",
    "navig-gator",
    "debug-bug",
    "hash-hippo",
    "blob-blobfish",
    "commit-chameleon",
    "koala-ty-assurance",
    "data-basset",
    "se-quail",
    "byte-bull",
    "ape-i",
    "maca-queue",
    "wi-fly",
    "ram-ram",
    "term-ite",
    "algo-rhythm-ant",
    "cat-alyst",
    "log-frog",
    "c-horse",
    "octo-pusher",
    "spam-ster",
];

const BLOCKED_SUB_AGENT_TOOLS: &[&str] = &[
    TOOL_SPAWN_SUB_AGENTS,
    TOOL_WAIT_SUB_AGENTS,
    TOOL_CANCEL_SUB_AGENTS,
    "send_file_to_user",
    "upload_file",
    "ssh_send_file_to_user",
    "transcribe_audio_file",
    "describe_image_file",
    "describe_video_file",
    "text_to_speech_en",
    "text_to_speech_en_file",
    "text_to_speech_ru",
    "text_to_speech_ru_file",
    "recreate_sandbox",
    "stack_logs_list_sources",
    "stack_logs_fetch",
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
    "wiki_memory_delete",
    // Jira write operations blocked for sub-agents (read-only access allowed)
    "jira_write",
];
const SUB_AGENT_REPORT_MAX_MESSAGES: usize = 6;
const SUB_AGENT_REPORT_MAX_CHARS: usize = 800;
const TOPIC_AGENTS_MD_LOAD_TIMEOUT_SECS: u64 = 5;

/// Provider for sub-agent delegation tool.
pub struct DelegationProvider {
    llm_client: Arc<crate::llm::LlmClient>,
    #[cfg_attr(
        not(any(
            feature = "tool-sandbox-exec",
            feature = "tool-sandbox-fileops",
            feature = "tool-ytdlp",
        )),
        allow(dead_code)
    )]
    sandbox_scope: SandboxScope,
    settings: Arc<crate::config::AgentSettings>,
    topic_agents_md_context: Option<TopicAgentsMdContext>,
    jobs: Arc<SubAgentJobStore>,
}

#[derive(Clone)]
struct TopicAgentsMdContext {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    topic_id: String,
}

struct PreparedSubAgentExecution {
    task_id: String,
    name: String,
    task: String,
    tool_runtime_registry: Arc<RuntimeToolRegistry>,
    tools: Vec<ToolDefinition>,
    system_prompt: String,
    date_suffix: String,
    todos_arc: Arc<Mutex<TodoList>>,
    messages: Vec<Message>,
    sub_session: EphemeralSession,
    runner_config: AgentRunnerConfig,
    compaction_controller: CompactionController,
    progress_tx: Option<mpsc::Sender<AgentEvent>>,
    progress_relay_task: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SubAgentJobStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

impl SubAgentJobStatus {
    const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

struct CompletedSubAgentExecution {
    status: SubAgentJobStatus,
    output: String,
}

struct SubAgentJobRecord {
    id: String,
    name: String,
    task: String,
    status: SubAgentJobStatus,
    output: Option<String>,
    started_at: Instant,
    completed_at: Option<Instant>,
    cancellation_token: tokio_util::sync::CancellationToken,
    handle: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
struct SubAgentJobSnapshot {
    id: String,
    name: String,
    task: String,
    status: SubAgentJobStatus,
    output: Option<String>,
    elapsed_ms: u128,
    completed: bool,
}

impl SubAgentJobSnapshot {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "name": self.name,
            "task": self.task,
            "status": self.status.as_str(),
            "output": self.output,
            "elapsed_ms": self.elapsed_ms,
            "completed": self.completed,
        })
    }
}

#[derive(Debug, Clone)]
struct SubAgentMissingSnapshot {
    id: String,
    status: &'static str,
}

impl SubAgentMissingSnapshot {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "id": self.id,
            "status": self.status,
        })
    }
}

#[derive(Default)]
struct SubAgentJobLookup {
    found: Vec<SubAgentJobSnapshot>,
    missing: Vec<SubAgentMissingSnapshot>,
}

struct SubAgentJobStore {
    jobs: StdMutex<HashMap<String, SubAgentJobRecord>>,
    semaphore: Arc<Semaphore>,
    notify: Notify,
}

impl SubAgentJobStore {
    fn new(max_concurrent: usize) -> Self {
        Self {
            jobs: StdMutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            notify: Notify::new(),
        }
    }

    fn max_concurrent(&self) -> usize {
        SUB_AGENT_MAX_CONCURRENT_JOBS
    }

    fn active_count(&self) -> usize {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.values()
            .filter(|job| !job.status.is_terminal())
            .count()
    }

    fn try_acquire_slots(&self, count: usize) -> Result<Vec<OwnedSemaphorePermit>> {
        let mut permits = Vec::with_capacity(count);
        for _ in 0..count {
            match Arc::clone(&self.semaphore).try_acquire_owned() {
                Ok(permit) => permits.push(permit),
                Err(_) => {
                    drop(permits);
                    return Err(anyhow!(
                        "sub-agent concurrency limit reached: max {} active jobs",
                        SUB_AGENT_MAX_CONCURRENT_JOBS
                    ));
                }
            }
        }
        Ok(permits)
    }

    fn insert_running(
        &self,
        id: String,
        name: String,
        task: String,
        cancellation_token: tokio_util::sync::CancellationToken,
    ) {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.insert(
            id.clone(),
            SubAgentJobRecord {
                id,
                name,
                task,
                status: SubAgentJobStatus::Running,
                output: None,
                started_at: Instant::now(),
                completed_at: None,
                cancellation_token,
                handle: None,
            },
        );
        self.notify.notify_waiters();
    }

    fn set_handle(&self, id: &str, handle: JoinHandle<()>) {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(job) = jobs.get_mut(id) {
            job.handle = Some(handle);
        }
    }

    fn has_active_name(&self, name: &str) -> bool {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.values()
            .any(|job| !job.status.is_terminal() && job.name == name)
    }

    fn finish(&self, id: &str, status: SubAgentJobStatus, output: String) {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(job) = jobs.get_mut(id) {
            if job.status != SubAgentJobStatus::Cancelled {
                job.status = status;
                job.output = Some(output);
            } else if job.output.is_none() {
                job.output = Some(output);
            }
            job.completed_at = Some(Instant::now());
        }
        drop(jobs);
        self.notify.notify_waiters();
    }

    fn cancel_ids(&self, ids: &[String]) -> SubAgentJobLookup {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut lookup = SubAgentJobLookup::default();
        for id in ids {
            match jobs.get_mut(id) {
                Some(job) => {
                    job.cancellation_token.cancel();
                    if !job.status.is_terminal() {
                        job.status = SubAgentJobStatus::Cancelled;
                        job.output = Some("Sub-agent was cancelled by parent request.".to_string());
                        job.completed_at = Some(Instant::now());
                    }
                    lookup.found.push(snapshot_job(job));
                }
                None => lookup.missing.push(SubAgentMissingSnapshot {
                    id: id.clone(),
                    status: "not_found",
                }),
            }
        }
        drop(jobs);
        self.notify.notify_waiters();
        lookup
    }

    fn cancel_all(&self) -> SubAgentJobLookup {
        let ids = {
            let jobs = self
                .jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            jobs.keys().cloned().collect::<Vec<_>>()
        };
        self.cancel_ids(&ids)
    }

    fn lookup(&self, ids: &[String]) -> SubAgentJobLookup {
        let jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut lookup = SubAgentJobLookup::default();
        for id in ids {
            match jobs.get(id) {
                Some(job) => lookup.found.push(snapshot_job(job)),
                None => lookup.missing.push(SubAgentMissingSnapshot {
                    id: id.clone(),
                    status: "not_found",
                }),
            }
        }
        lookup
    }

    async fn wait_for_any_terminal(&self, ids: &[String], timeout_ms: u64) -> SubAgentJobLookup {
        let wait_duration = Duration::from_millis(timeout_ms.min(SUB_AGENT_MAX_WAIT_TIMEOUT_MS));
        let started = Instant::now();

        loop {
            let notified = self.notify.notified();
            let lookup = self.lookup(ids);
            if lookup
                .found
                .iter()
                .any(|snapshot| snapshot.status.is_terminal())
                || !lookup.missing.is_empty()
                || wait_duration.is_zero()
                || started.elapsed() >= wait_duration
            {
                return lookup;
            }

            let remaining = wait_duration.saturating_sub(started.elapsed());
            if timeout(remaining, notified).await.is_err() {
                return self.lookup(ids);
            }
        }
    }
}

impl Drop for SubAgentJobStore {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            for job in jobs.values_mut() {
                job.cancellation_token.cancel();
                if let Some(handle) = job.handle.take() {
                    handle.abort();
                }
            }
        }
    }
}

fn snapshot_job(job: &SubAgentJobRecord) -> SubAgentJobSnapshot {
    let completed = job.status.is_terminal();
    let elapsed = job
        .completed_at
        .unwrap_or_else(Instant::now)
        .duration_since(job.started_at);
    SubAgentJobSnapshot {
        id: job.id.clone(),
        name: job.name.clone(),
        task: job.task.clone(),
        status: job.status,
        output: job.output.clone(),
        elapsed_ms: elapsed.as_millis(),
        completed,
    }
}

fn normalize_job_ids(ids: Vec<String>) -> Result<Vec<String>> {
    let ids = ids
        .into_iter()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(anyhow!("ids must contain at least one sub-agent id"));
    }
    Ok(ids)
}

fn lookup_to_json(lookup: SubAgentJobLookup) -> Vec<serde_json::Value> {
    lookup
        .found
        .into_iter()
        .map(|snapshot| snapshot.to_json())
        .chain(lookup.missing.into_iter().map(|missing| missing.to_json()))
        .collect()
}

fn serialize_json(value: serde_json::Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|error| {
        json!({
            "ok": false,
            "error": format!("failed to serialize sub-agent response: {error}"),
        })
        .to_string()
    })
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
            topic_agents_md_context: None,
            jobs: Arc::new(SubAgentJobStore::new(SUB_AGENT_MAX_CONCURRENT_JOBS)),
        }
    }

    /// Inherit topic-scoped `AGENTS.md` context for sub-agent prompt composition.
    #[must_use]
    pub fn with_topic_agents_md_context(
        mut self,
        storage: Arc<dyn StorageProvider>,
        user_id: i64,
        topic_id: impl Into<String>,
    ) -> Self {
        let topic_id = topic_id.into();
        if !topic_id.trim().is_empty() {
            self.topic_agents_md_context = Some(TopicAgentsMdContext {
                storage,
                user_id,
                topic_id,
            });
        }
        self
    }

    /// Build native typed runtime executors for sub-agent delegation tools.
    #[must_use]
    pub fn tool_runtime_executors(
        self: &Arc<Self>,
        progress_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        Self::tool_definitions()
            .into_iter()
            .map(|spec| {
                Arc::new(DelegationToolExecutor {
                    provider: Arc::clone(self),
                    name: ToolName::from(spec.name.clone()),
                    spec,
                    progress_tx: progress_tx.clone(),
                }) as Arc<dyn ToolExecutor>
            })
            .collect()
    }

    fn tool_definitions() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: TOOL_SPAWN_SUB_AGENTS.to_string(),
                description: "Start one or more stateless sub-agents in the background. \
Use this for independent, atomic tasks. Returns sub-agent ids immediately; use wait_sub_agents only when results are needed. \
At most five sub-agents may run concurrently."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "tasks": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": SUB_AGENT_MAX_CONCURRENT_JOBS,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "task": {
                                        "type": "string",
                                        "description": "Short, clear task for one sub-agent"
                                    },
                                    "tools": {
                                        "type": "array",
                                        "description": "Whitelist of allowed tools for this sub-agent",
                                        "items": {"type": "string"}
                                    },
                                    "context": {
                                        "type": "string",
                                        "description": "Additional task context (optional)"
                                    }
                                },
                                "required": ["task", "tools"]
                            }
                        }
                    },
                    "required": ["tasks"]
                }),
            },
            ToolDefinition {
                name: TOOL_WAIT_SUB_AGENTS.to_string(),
                description: "Wait for one or more background sub-agents. \
Returns as soon as any requested sub-agent reaches a final status or the timeout expires."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Sub-agent ids returned by spawn_sub_agents"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Optional wait timeout in milliseconds. Defaults to 30000 and is capped at 3600000."
                        }
                    },
                    "required": ["ids"]
                }),
            },
            ToolDefinition {
                name: TOOL_CANCEL_SUB_AGENTS.to_string(),
                description: "Cancel selected background sub-agents, or all sub-agents for this parent run with all=true."
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Sub-agent ids to cancel"
                        },
                        "all": {
                            "type": "boolean",
                            "description": "Cancel all sub-agents owned by this parent run"
                        }
                    }
                }),
            },
        ]
    }

    fn blocked_tool_set() -> HashSet<String> {
        BLOCKED_SUB_AGENT_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect()
    }

    fn build_sub_agent_tool_runtime_executors(
        &self,
        todos_arc: Arc<Mutex<crate::agent::providers::TodoList>>,
        memory_scope: AgentMemoryScope,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> Vec<Arc<dyn ToolExecutor>> {
        let mut executors: Vec<Arc<dyn ToolExecutor>> = Vec::new();
        let module_ctx =
            self.build_sub_agent_tool_module_context(todos_arc, memory_scope, progress_tx);

        #[cfg(not(any(
            feature = "tool-sandbox-exec",
            feature = "tool-sandbox-fileops",
            feature = "tool-brave-search",
            feature = "tool-searxng",
            feature = "tool-tavily",
            feature = "tool-todos",
            feature = "tool-crawl4ai-markdown",
            feature = "tool-webfetch-md",
            feature = "tool-ytdlp"
        )))]
        let _ = (&module_ctx, &mut executors);

        #[cfg(feature = "tool-todos")]
        self.push_sub_agent_tool_module(&mut executors, &TodosToolModule, &module_ctx);

        #[cfg(feature = "tool-sandbox-exec")]
        self.push_sub_agent_tool_module(&mut executors, &SandboxExecToolModule, &module_ctx);

        #[cfg(feature = "tool-sandbox-fileops")]
        self.push_sub_agent_tool_module(&mut executors, &SandboxFileOpsToolModule, &module_ctx);

        #[cfg(feature = "tool-ytdlp")]
        self.push_sub_agent_tool_module(&mut executors, &YtdlpToolModule, &module_ctx);

        #[cfg(feature = "tool-webfetch-md")]
        self.push_sub_agent_tool_module(&mut executors, &WebCrawlerToolModule, &module_ctx);

        #[cfg(feature = "tool-webfetch-md")]
        self.push_sub_agent_tool_module(&mut executors, &WebFetchMdToolModule, &module_ctx);

        #[cfg(feature = "tool-crawl4ai-markdown")]
        self.push_sub_agent_tool_module(&mut executors, &Crawl4AiMarkdownToolModule, &module_ctx);

        #[cfg(feature = "tool-tavily")]
        self.push_sub_agent_tool_module(&mut executors, &TavilyToolModule, &module_ctx);

        #[cfg(feature = "tool-brave-search")]
        self.push_sub_agent_tool_module(&mut executors, &BraveSearchToolModule, &module_ctx);

        #[cfg(feature = "tool-searxng")]
        self.push_sub_agent_tool_module(&mut executors, &SearxngToolModule, &module_ctx);

        self.warn_for_uncompiled_sub_agent_tool_modules();

        executors
    }

    fn build_sub_agent_tool_module_context(
        &self,
        todos_arc: Arc<Mutex<TodoList>>,
        _memory_scope: AgentMemoryScope,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
    ) -> ToolModuleContext {
        let sandbox_runtime = if let Some(tx) = progress_tx {
            Arc::new(SandboxRuntime::new(self.sandbox_scope.clone()).with_progress_tx(tx.clone()))
        } else {
            Arc::new(SandboxRuntime::new(self.sandbox_scope.clone()))
        };

        ToolModuleContext::new(ToolModuleContextParts {
            todos: todos_arc,
            sandbox_scope: self.sandbox_scope.clone(),
            sandbox_runtime,
            llm_client: Arc::clone(&self.llm_client),
            settings: Arc::clone(&self.settings),
            #[cfg(feature = "tool-agents-md")]
            agents_md_context: None,
            #[cfg(feature = "manager-control-plane")]
            manager_control_plane_context: None,
            #[cfg(feature = "integration-ssh-mcp")]
            ssh_mcp_context: None,
            #[cfg(feature = "tool-reminder")]
            reminder_context: None,
            #[cfg(feature = "tool-wiki-memory")]
            wiki_memory_store: None,
            #[cfg(feature = "tool-wiki-memory")]
            memory_scope: _memory_scope,
            progress_tx: progress_tx.cloned(),
        })
    }

    #[cfg(any(
        feature = "tool-sandbox-exec",
        feature = "tool-sandbox-fileops",
        feature = "tool-brave-search",
        feature = "tool-searxng",
        feature = "tool-tavily",
        feature = "tool-todos",
        feature = "tool-crawl4ai-markdown",
        feature = "tool-webfetch-md",
        feature = "tool-ytdlp"
    ))]
    fn push_sub_agent_tool_module<M>(
        &self,
        executors: &mut Vec<Arc<dyn ToolExecutor>>,
        module: &M,
        ctx: &ToolModuleContext,
    ) where
        M: ToolModule,
    {
        let module_id = module.module_id();
        if !self.settings.is_module_enabled(module_id.as_str()) {
            return;
        }

        executors.extend(module.tool_runtime_executors(ctx));
    }

    fn warn_for_uncompiled_sub_agent_tool_modules(&self) {
        #[cfg(not(feature = "tool-tavily"))]
        if crate::config::is_tavily_enabled() {
            warn!("Tavily enabled but feature not compiled in");
        }

        #[cfg(not(feature = "tool-brave-search"))]
        if crate::config::is_brave_search_enabled() {
            warn!("Brave Search enabled but feature not compiled in");
        }

        #[cfg(not(feature = "tool-searxng"))]
        if crate::config::is_searxng_enabled() {
            warn!("SearXNG enabled but feature not compiled in");
        }
    }

    fn build_sub_agent_tool_runtime_registry(
        &self,
        allowed_tools: &HashSet<String>,
        executors: Vec<Arc<dyn ToolExecutor>>,
    ) -> RuntimeToolRegistry {
        let mut registry = RuntimeToolRegistry::new();
        for executor in executors {
            let tool_name = executor.name();
            if !allowed_tools.contains(tool_name.as_str()) {
                continue;
            }
            if let Err(error) = registry.register(executor) {
                warn!(
                    tool_name = %tool_name,
                    error = %error,
                    "Skipping duplicate sub-agent typed runtime executor"
                );
            }
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
        let mut allowed: HashSet<String> = requested
            .iter()
            .filter(|name| !blocked.contains(*name))
            .filter(|name| available_tools.contains(*name))
            .cloned()
            .collect();

        if requested.contains(TOOL_WEB_MARKDOWN)
            && !blocked.contains(TOOL_WEB_CRAWLER)
            && available_tools.contains(TOOL_WEB_CRAWLER)
        {
            allowed.insert(TOOL_WEB_CRAWLER.to_string());
        }

        if requested.contains(TOOL_WEB_MARKDOWN)
            && !blocked.contains(TOOL_CRAWL4AI_MARKDOWN)
            && available_tools.contains(TOOL_CRAWL4AI_MARKDOWN)
        {
            allowed.insert(TOOL_CRAWL4AI_MARKDOWN.to_string());
        }

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

    fn create_sub_agent_runner_with_client(
        llm_client: Arc<crate::llm::LlmClient>,
        blocked: HashSet<String>,
        max_tokens: usize,
    ) -> AgentRunner {
        let max_iterations = get_sub_agent_max_iterations();
        let mut runner = AgentRunner::new(llm_client);
        runner.register_hook(Box::new(CompletionCheckHook::new()));
        runner.register_hook(Box::new(HotContextHealthHook::new()));
        runner.register_hook(Box::new(SubAgentSafetyHook::new(SubAgentSafetyConfig {
            max_iterations,
            max_tokens,
            blocked_tools: blocked,
        })));
        runner.register_hook(Box::new(SearchBudgetHook::new(get_agent_search_limit())));
        runner.register_hook(Box::new(TimeoutReportHook::new()));
        runner
    }

    fn create_sub_agent_compaction_controller(&self) -> CompactionController {
        CompactionController::local_llm(
            Arc::clone(&self.llm_client),
            self.settings.get_sub_agent_timeout_secs(),
        )
    }

    fn parse_sub_agent_task_args(arguments: &str) -> Result<SubAgentTaskArgs> {
        let args: SubAgentTaskArgs = serde_json::from_str(arguments)?;
        if args.task.trim().is_empty() {
            return Err(anyhow!("Sub-agent task cannot be empty"));
        }
        if args.tools.is_empty() {
            return Err(anyhow!("Sub-agent tools whitelist cannot be empty"));
        }
        Ok(args)
    }

    fn validate_sub_agent_task_args(args: SubAgentTaskArgs) -> Result<SubAgentTaskArgs> {
        if args.task.trim().is_empty() {
            return Err(anyhow!("Sub-agent task cannot be empty"));
        }
        if args.tools.is_empty() {
            return Err(anyhow!("Sub-agent tools whitelist cannot be empty"));
        }
        Ok(args)
    }

    fn unique_sub_agent_display_name(
        &self,
        seed: Uuid,
        reserved_names: &HashSet<String>,
    ) -> String {
        let bytes = seed.as_bytes();
        let start = usize::from(bytes[0]) % SUB_AGENT_DISPLAY_NAMES.len();
        for offset in 0..SUB_AGENT_DISPLAY_NAMES.len() {
            let name = SUB_AGENT_DISPLAY_NAMES[(start + offset) % SUB_AGENT_DISPLAY_NAMES.len()];
            if !reserved_names.contains(name) && !self.jobs.has_active_name(name) {
                return name.to_string();
            }
        }

        SUB_AGENT_DISPLAY_NAMES[start].to_string()
    }

    fn build_sub_agent_session(
        task: &str,
        max_tokens: usize,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
        topic_agents_md: Option<&str>,
    ) -> EphemeralSession {
        let mut sub_session = match cancellation_token {
            Some(parent_token) => EphemeralSession::with_parent_token(max_tokens, parent_token),
            None => EphemeralSession::new(max_tokens),
        };
        if let Some(topic_agents_md) = topic_agents_md
            .map(str::trim)
            .filter(|content| !content.is_empty())
        {
            sub_session
                .memory_mut()
                .add_message(AgentMessage::topic_agents_md(topic_agents_md));
        }
        sub_session
            .memory_mut()
            .add_message(AgentMessage::user_task(task));
        sub_session
    }

    async fn load_topic_agents_md(&self) -> Result<Option<String>> {
        let Some(context) = self.topic_agents_md_context.as_ref() else {
            return Ok(None);
        };
        let record = timeout(
            Duration::from_secs(TOPIC_AGENTS_MD_LOAD_TIMEOUT_SECS),
            context
                .storage
                .get_topic_agents_md(context.user_id, context.topic_id.clone()),
        )
        .await
        .map_err(|_| {
            anyhow!(
                "Timed out after {TOPIC_AGENTS_MD_LOAD_TIMEOUT_SECS}s while loading topic AGENTS.md for sub-agent bootstrap"
            )
        })?
        .map_err(|error| {
            anyhow!("Failed to load topic AGENTS.md for sub-agent bootstrap: {error}")
        })?;

        match record {
            Some(record) => {
                let agents_md = record.agents_md.trim().to_string();
                if agents_md.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(agents_md))
                }
            }
            None => Ok(None),
        }
    }

    fn build_sub_agent_runner_config(&self, model: &crate::config::ModelInfo) -> AgentRunnerConfig {
        AgentRunnerConfig::new(
            model.id.clone(),
            get_sub_agent_max_iterations(),
            get_agent_continuation_limit(),
            self.settings.get_sub_agent_timeout_secs(),
            model.max_output_tokens,
        )
        .with_model_provider(model.provider.clone())
        .with_model_routes(self.settings.get_configured_sub_agent_model_routes())
        .with_sub_agent(true)
    }

    async fn prepare_sub_agent_execution(
        &self,
        arguments: &str,
        reserved_names: &HashSet<String>,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<PreparedSubAgentExecution> {
        let SubAgentTaskArgs {
            task,
            tools: requested_tools,
            context,
        } = Self::parse_sub_agent_task_args(arguments)?;

        let task_uuid = Uuid::new_v4();
        let task_id = format!("sub-{task_uuid}");
        let name = self.unique_sub_agent_display_name(task_uuid, reserved_names);
        let model_routes = self.settings.get_configured_sub_agent_model_routes();
        let model = model_routes
            .first()
            .cloned()
            .unwrap_or_else(|| self.settings.get_configured_sub_agent_model());
        let sub_agent_context_budget = self.settings.get_sub_agent_internal_context_budget_tokens();
        let topic_agents_md = self.load_topic_agents_md().await?;
        let sub_session = Self::build_sub_agent_session(
            task.as_str(),
            sub_agent_context_budget,
            cancellation_token,
            topic_agents_md.as_deref(),
        );
        let todos_arc = Arc::new(Mutex::new(sub_session.memory().todos.clone()));
        let (sub_agent_progress_tx, progress_relay_task) =
            spawn_sub_agent_progress_relay(progress_tx, task_id.clone(), name.clone());
        let executors = self.build_sub_agent_tool_runtime_executors(
            Arc::clone(&todos_arc),
            AgentMemoryScope::new(0, "sub-agent", task_id.clone()),
            sub_agent_progress_tx.as_ref(),
        );
        let available_tools: HashSet<String> = executors
            .iter()
            .map(|executor| executor.name().into_inner())
            .collect();
        let allowed = self.filter_allowed_tools(requested_tools, &available_tools, &task_id)?;
        let tool_runtime_registry =
            Arc::new(self.build_sub_agent_tool_runtime_registry(&allowed, executors));
        let tools = tool_runtime_registry.specs();
        let structured_output = crate::llm::LlmClient::supports_structured_output_for_model(&model);
        let system_prompt = create_sub_agent_system_prompt(
            task.as_str(),
            &tools,
            structured_output,
            context.as_deref(),
        );

        Ok(PreparedSubAgentExecution {
            task_id,
            name,
            task,
            tool_runtime_registry,
            tools,
            system_prompt: system_prompt.base,
            date_suffix: system_prompt.date_suffix,
            todos_arc,
            messages: AgentRunner::convert_memory_to_messages(sub_session.memory().get_messages()),
            sub_session,
            runner_config: self.build_sub_agent_runner_config(&model),
            compaction_controller: self.create_sub_agent_compaction_controller(),
            progress_tx: sub_agent_progress_tx,
            progress_relay_task,
        })
    }

    fn build_sub_agent_runner_context<'a>(
        prepared: &'a mut PreparedSubAgentExecution,
    ) -> AgentRunnerContext<'a> {
        let mut ctx = AgentRunnerContext::new_base(
            AgentRunnerContextBase {
                task: prepared.task.as_str(),
                system_prompt: &prepared.system_prompt,
                date_suffix: &prepared.date_suffix,
                tools: &prepared.tools,
                progress_tx: prepared.progress_tx.as_ref(),
                todos_arc: &prepared.todos_arc,
                task_id: &prepared.task_id,
                messages: &mut prepared.messages,
                agent: &mut prepared.sub_session,
            },
            Some(&prepared.compaction_controller),
            prepared.runner_config.clone(),
        );
        ctx.tool_runtime_registry = Some(Arc::clone(&prepared.tool_runtime_registry));
        ctx
    }

    fn sub_agent_timeout_duration_for_settings(settings: &AgentSettings) -> Duration {
        Duration::from_secs(settings.get_sub_agent_timeout_secs() + 30)
    }

    fn build_sub_agent_error_report_for_settings(
        settings: &AgentSettings,
        task_id: &str,
        memory: &AgentMemory,
        error: String,
    ) -> String {
        Self::build_sub_agent_report_with_status_for_settings(
            settings,
            task_id,
            memory,
            SubAgentReportStatus::Error,
            error,
        )
    }

    fn build_sub_agent_timeout_report_for_settings(
        settings: &AgentSettings,
        task_id: &str,
        memory: &AgentMemory,
    ) -> String {
        let limit = settings.get_sub_agent_timeout_secs();
        Self::build_sub_agent_report_with_status_for_settings(
            settings,
            task_id,
            memory,
            SubAgentReportStatus::Timeout,
            format!("Sub-agent hard timed out after {} seconds", limit + 30),
        )
    }

    fn build_sub_agent_report_with_status_for_settings(
        settings: &AgentSettings,
        task_id: &str,
        memory: &AgentMemory,
        status: SubAgentReportStatus,
        error: String,
    ) -> String {
        build_sub_agent_report(SubAgentReportContext {
            task_id,
            status,
            error: Some(error),
            memory,
            timeout_secs: settings.get_sub_agent_timeout_secs(),
        })
    }

    fn shape_sub_agent_terminal_output_for_settings(
        settings: &AgentSettings,
        outcome: TimedRunResult,
        task_id: &str,
        memory: &AgentMemory,
    ) -> String {
        match outcome {
            TimedRunResult::Final(result) => result,
            TimedRunResult::WaitingForUserInput(_) => {
                warn!(task_id = %task_id, "Sub-agent paused waiting for unsupported user input");
                Self::build_sub_agent_error_report_for_settings(
                    settings,
                    task_id,
                    memory,
                    "sub-agent paused waiting for unsupported user input".to_string(),
                )
            }
            TimedRunResult::Failed(err) => {
                warn!(task_id = %task_id, error = %err, "Sub-agent failed");
                Self::build_sub_agent_error_report_for_settings(
                    settings,
                    task_id,
                    memory,
                    err.to_string(),
                )
            }
            TimedRunResult::TimedOut => {
                warn!(task_id = %task_id, "Sub-agent hard timed out");
                Self::build_sub_agent_timeout_report_for_settings(settings, task_id, memory)
            }
        }
    }

    const fn status_for_timed_run_result(outcome: &TimedRunResult) -> SubAgentJobStatus {
        match outcome {
            TimedRunResult::Final(_) => SubAgentJobStatus::Completed,
            TimedRunResult::WaitingForUserInput(_) => SubAgentJobStatus::Failed,
            TimedRunResult::Failed(_) => SubAgentJobStatus::Failed,
            TimedRunResult::TimedOut => SubAgentJobStatus::TimedOut,
        }
    }

    async fn finish_sub_agent_progress_relay(prepared: &mut PreparedSubAgentExecution) {
        // Drop the restricted providers before awaiting the relay task.
        // Some providers retain cloned progress senders internally; if the
        // typed runtime registry stays alive while we await the relay, the
        // sub-agent channel never closes and the relay task cannot finish.
        prepared.tools.clear();
        prepared.tool_runtime_registry = Arc::new(RuntimeToolRegistry::new());
        drop(prepared.progress_tx.take());

        if let Some(task) = prepared.progress_relay_task.take() {
            let _ = task.await;
        }
    }

    async fn spawn_sub_agents(
        &self,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        let args: SpawnSubAgentsArgs = serde_json::from_str(arguments)?;
        if args.tasks.is_empty() {
            return Err(anyhow!("tasks must contain at least one sub-agent task"));
        }
        if args.tasks.len() > SUB_AGENT_MAX_CONCURRENT_JOBS {
            return Err(anyhow!(
                "cannot spawn more than {SUB_AGENT_MAX_CONCURRENT_JOBS} sub-agents at once"
            ));
        }

        let tasks = args
            .tasks
            .into_iter()
            .map(Self::validate_sub_agent_task_args)
            .collect::<Result<Vec<_>>>()?;
        let permits = self.jobs.try_acquire_slots(tasks.len())?;
        let mut prepared_jobs = Vec::with_capacity(tasks.len());
        let mut reserved_names = HashSet::with_capacity(tasks.len());
        for task in tasks {
            let arguments = serde_json::to_string(&task)?;
            let prepared = self
                .prepare_sub_agent_execution(
                    &arguments,
                    &reserved_names,
                    progress_tx,
                    cancellation_token,
                )
                .await?;
            reserved_names.insert(prepared.name.clone());
            prepared_jobs.push(prepared);
        }

        let mut started = Vec::with_capacity(prepared_jobs.len());
        for (prepared, permit) in prepared_jobs.into_iter().zip(permits) {
            let job_id = prepared.task_id.clone();
            let job_name = prepared.name.clone();
            let task = prepared.task.clone();
            let job_token = prepared.sub_session.cancellation_token().clone();
            self.jobs
                .insert_running(job_id.clone(), job_name.clone(), task.clone(), job_token);

            let jobs = Arc::clone(&self.jobs);
            let llm_client = Arc::clone(&self.llm_client);
            let settings = Arc::clone(&self.settings);
            let task_id_for_run = job_id.clone();
            let handle = tokio::spawn(async move {
                let _slot = permit;
                let completed =
                    Self::run_prepared_sub_agent_execution(llm_client, settings, prepared).await;
                jobs.finish(&task_id_for_run, completed.status, completed.output);
            });
            self.jobs.set_handle(&job_id, handle);

            started.push(json!({
                "id": job_id,
                "name": job_name,
                "task": task,
                "status": SubAgentJobStatus::Running.as_str(),
            }));
        }

        Ok(serialize_json(json!({
            "ok": true,
            "started": started,
            "active_count": self.jobs.active_count(),
            "max_active": self.jobs.max_concurrent(),
        })))
    }

    async fn wait_sub_agents(&self, arguments: &str) -> Result<String> {
        let args: WaitSubAgentsArgs = serde_json::from_str(arguments)?;
        let ids = normalize_job_ids(args.ids)?;
        let timeout_ms = args
            .timeout_ms
            .unwrap_or(SUB_AGENT_DEFAULT_WAIT_TIMEOUT_MS)
            .min(SUB_AGENT_MAX_WAIT_TIMEOUT_MS);
        let lookup = self.jobs.wait_for_any_terminal(&ids, timeout_ms).await;
        let timed_out = !lookup
            .found
            .iter()
            .any(|snapshot| snapshot.status.is_terminal())
            && lookup.missing.is_empty();

        Ok(serialize_json(json!({
            "ok": true,
            "timed_out": timed_out,
            "statuses": lookup_to_json(lookup),
            "active_count": self.jobs.active_count(),
            "max_active": self.jobs.max_concurrent(),
        })))
    }

    fn cancel_sub_agents(&self, arguments: &str) -> Result<String> {
        let args: CancelSubAgentsArgs = serde_json::from_str(arguments)?;
        let lookup = if args.all {
            self.jobs.cancel_all()
        } else {
            let ids = normalize_job_ids(args.ids)?;
            self.jobs.cancel_ids(&ids)
        };

        Ok(serialize_json(json!({
            "ok": true,
            "statuses": lookup_to_json(lookup),
            "active_count": self.jobs.active_count(),
            "max_active": self.jobs.max_concurrent(),
        })))
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &str,
        progress_tx: Option<&tokio::sync::mpsc::Sender<AgentEvent>>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<String> {
        match tool_name {
            TOOL_SPAWN_SUB_AGENTS => {
                self.spawn_sub_agents(arguments, progress_tx, cancellation_token)
                    .await
            }
            TOOL_WAIT_SUB_AGENTS => self.wait_sub_agents(arguments).await,
            TOOL_CANCEL_SUB_AGENTS => self.cancel_sub_agents(arguments),
            _ => Err(anyhow!("Unknown delegation tool: {tool_name}")),
        }
    }

    async fn run_prepared_sub_agent_execution(
        llm_client: Arc<crate::llm::LlmClient>,
        settings: Arc<AgentSettings>,
        mut prepared: PreparedSubAgentExecution,
    ) -> CompletedSubAgentExecution {
        let mut runner = Self::create_sub_agent_runner_with_client(
            llm_client,
            Self::blocked_tool_set(),
            prepared.sub_session.memory().max_tokens(),
        );
        info!(task_id = %prepared.task_id, "Running sub-agent delegation");

        let outcome = {
            let mut ctx = Self::build_sub_agent_runner_context(&mut prepared);
            run_with_timeout(
                &mut runner,
                &mut ctx,
                Self::sub_agent_timeout_duration_for_settings(&settings),
            )
            .await
        };
        let status = Self::status_for_timed_run_result(&outcome);
        Self::finish_sub_agent_progress_relay(&mut prepared).await;

        let output = Self::shape_sub_agent_terminal_output_for_settings(
            &settings,
            outcome,
            &prepared.task_id,
            prepared.sub_session.memory(),
        );

        CompletedSubAgentExecution { status, output }
    }
}

struct DelegationToolExecutor {
    provider: Arc<DelegationProvider>,
    name: ToolName,
    spec: ToolDefinition,
    progress_tx: Option<mpsc::Sender<AgentEvent>>,
}

#[async_trait]
impl ToolExecutor for DelegationToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });
        self.provider
            .execute_tool(
                self.name.as_str(),
                &invocation.raw_arguments,
                self.progress_tx.as_ref(),
                Some(&invocation.cancellation_token),
            )
            .await
            .map(|output| normalizer.success(&invocation, &output, ""))
            .map_err(delegation_runtime_error)
    }
}

fn delegation_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if error.downcast_ref::<serde_json::Error>().is_some()
        || message.contains("tasks must contain")
        || message.contains("cannot spawn more than")
        || message.contains("Sub-agent task cannot be empty")
        || message.contains("Sub-agent tools whitelist cannot be empty")
        || message.contains("ids must contain")
        || message.contains("No allowed tools left")
    {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

fn spawn_sub_agent_progress_relay(
    parent_tx: Option<&mpsc::Sender<AgentEvent>>,
    sub_agent_id: String,
    sub_agent_name: String,
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

            if parent_tx
                .send(event.with_sub_agent_source(sub_agent_id.clone(), sub_agent_name.clone()))
                .await
                .is_err()
            {
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
            | AgentEvent::Finished
            | AgentEvent::Cancelling { .. }
            | AgentEvent::Cancelled
            | AgentEvent::Error(_)
            | AgentEvent::LoopDetected { .. }
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct SubAgentTaskArgs {
    task: String,
    tools: Vec<String>,
    #[serde(default)]
    context: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpawnSubAgentsArgs {
    tasks: Vec<SubAgentTaskArgs>,
}

#[derive(Debug, Deserialize)]
struct WaitSubAgentsArgs {
    ids: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct CancelSubAgentsArgs {
    #[serde(default)]
    ids: Vec<String>,
    #[serde(default)]
    all: bool,
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
        DelegationProvider, SUB_AGENT_DISPLAY_NAMES, SUB_AGENT_MAX_CONCURRENT_JOBS,
        SubAgentJobStatus, SubAgentJobStore, TOOL_CANCEL_SUB_AGENTS, TOOL_SPAWN_SUB_AGENTS,
        TOOL_WAIT_SUB_AGENTS, should_forward_sub_agent_progress_event,
        spawn_sub_agent_progress_relay,
    };
    use crate::agent::compaction::BudgetState;
    use crate::agent::context::{AgentContext, EphemeralSession};
    use crate::agent::identity::SessionId;
    use crate::agent::memory::AgentMessage;
    use crate::agent::progress::{AgentEvent, AgentEventSource, FileDeliveryKind, TokenSnapshot};
    use crate::agent::providers::TodoList;
    use crate::agent::runner::TimedRunResult;
    use crate::agent::session::{AgentMemoryScope, PendingUserInput, UserInputKind};
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolInvocation, ToolName, ToolOutputStatus, ToolRuntimeError, ToolTimeoutConfig, TurnId,
    };
    use crate::config::AgentSettings;
    use crate::llm::{InvocationId, LlmClient};
    use crate::storage::MockStorageProvider;
    use chrono::Utc;
    use mockall::predicate::eq;
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn runtime_invocation(tool_name: &str, raw_arguments: &str) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(77),
            turn_id: TurnId::from("turn-delegation"),
            batch_id: ToolBatchId::from("batch-delegation"),
            batch_index: 0,
            invocation_id: InvocationId::from(format!("invoke-{tool_name}")),
            tool_call_id: ToolCallId::from(format!("call-{tool_name}")),
            provider_tool_call_id: None,
            tool_name: ToolName::from(tool_name),
            raw_provider_payload: json!({}),
            raw_arguments: raw_arguments.to_string(),
            normalized_arguments: serde_json::Value::Null,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    fn test_memory_scope() -> AgentMemoryScope {
        AgentMemoryScope::new(77, "test-topic", "sub-agent-test")
    }

    #[test]
    fn sub_agent_blocklist_includes_sensitive_tools() {
        let blocked = DelegationProvider::blocked_tool_set();

        for tool in [
            "spawn_sub_agents",
            "wait_sub_agents",
            "cancel_sub_agents",
            "transcribe_audio_file",
            "describe_image_file",
            "describe_video_file",
            "text_to_speech_en",
            "text_to_speech_en_file",
            "text_to_speech_ru",
            "text_to_speech_ru_file",
            "upload_file",
            "recreate_sandbox",
            "stack_logs_list_sources",
            "stack_logs_fetch",
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
    fn sub_agent_job_store_enforces_five_active_slots() {
        let store = SubAgentJobStore::new(SUB_AGENT_MAX_CONCURRENT_JOBS);
        let permits = store
            .try_acquire_slots(SUB_AGENT_MAX_CONCURRENT_JOBS)
            .expect("five slots should be available");

        let error = match store.try_acquire_slots(1) {
            Ok(_) => panic!("sixth concurrent sub-agent must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("concurrency limit"));

        drop(permits);
        assert!(store.try_acquire_slots(1).is_ok());
    }

    #[test]
    fn sub_agent_job_store_cancel_marks_jobs_terminal() {
        let store = SubAgentJobStore::new(SUB_AGENT_MAX_CONCURRENT_JOBS);
        store.insert_running(
            "sub-1".to_string(),
            "debug-bug".to_string(),
            "Inspect workspace".to_string(),
            tokio_util::sync::CancellationToken::new(),
        );

        let lookup = store.cancel_ids(&["sub-1".to_string()]);

        assert_eq!(lookup.found.len(), 1);
        assert_eq!(lookup.found[0].status, SubAgentJobStatus::Cancelled);
        assert_eq!(lookup.found[0].name, "debug-bug");
        assert!(lookup.found[0].completed);
        assert_eq!(store.active_count(), 0);
    }

    #[test]
    fn sub_agent_display_names_are_drawn_from_pun_pool() {
        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);

        let name = provider.unique_sub_agent_display_name(Uuid::nil(), &HashSet::new());

        assert!(SUB_AGENT_DISPLAY_NAMES.contains(&name.as_str()));
    }

    #[test]
    fn sub_agent_display_names_avoid_active_collisions() {
        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);
        provider.jobs.insert_running(
            "sub-1".to_string(),
            "investi-gator".to_string(),
            "Inspect workspace".to_string(),
            tokio_util::sync::CancellationToken::new(),
        );

        let name = provider.unique_sub_agent_display_name(Uuid::nil(), &HashSet::new());

        assert_ne!(name, "investi-gator");
        assert!(SUB_AGENT_DISPLAY_NAMES.contains(&name.as_str()));
    }

    #[test]
    fn typed_runtime_executors_register_delegation_tools() {
        let settings = Arc::new(AgentSettings::default());
        let provider = Arc::new(DelegationProvider::new(
            Arc::new(LlmClient::new(&settings)),
            1_i64,
            settings,
        ));
        let executors = provider.tool_runtime_executors(None);
        let tool_names = executors
            .iter()
            .map(|executor| executor.name().into_inner())
            .collect::<Vec<_>>();

        for tool_name in [
            TOOL_SPAWN_SUB_AGENTS,
            TOOL_WAIT_SUB_AGENTS,
            TOOL_CANCEL_SUB_AGENTS,
        ] {
            assert!(
                tool_names.iter().any(|name| name == tool_name),
                "missing typed delegation executor: {tool_name}"
            );
            assert_eq!(
                tool_names
                    .iter()
                    .filter(|name| name.as_str() == tool_name)
                    .count(),
                1,
                "expected one typed delegation executor for {tool_name}"
            );
        }
    }

    #[tokio::test]
    async fn typed_runtime_executor_rejects_empty_spawn_tasks_before_runner() {
        let settings = Arc::new(AgentSettings::default());
        let provider = Arc::new(DelegationProvider::new(
            Arc::new(LlmClient::new(&settings)),
            1_i64,
            settings,
        ));
        let executor = provider
            .tool_runtime_executors(None)
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_SPAWN_SUB_AGENTS)
            .expect("spawn_sub_agents typed executor registered");

        let error = executor
            .execute(runtime_invocation(TOOL_SPAWN_SUB_AGENTS, r#"{"tasks":[]}"#))
            .await
            .expect_err("empty spawn tasks must be invalid arguments");

        assert!(matches!(error, ToolRuntimeError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn typed_runtime_executor_cancels_all_without_jobs() {
        let settings = Arc::new(AgentSettings::default());
        let provider = Arc::new(DelegationProvider::new(
            Arc::new(LlmClient::new(&settings)),
            1_i64,
            settings,
        ));
        let executor = provider
            .tool_runtime_executors(None)
            .into_iter()
            .find(|executor| executor.name().as_str() == TOOL_CANCEL_SUB_AGENTS)
            .expect("cancel_sub_agents typed executor registered");

        let output = executor
            .execute(runtime_invocation(
                TOOL_CANCEL_SUB_AGENTS,
                r#"{"all":true}"#,
            ))
            .await
            .expect("cancel all succeeds without active jobs");

        assert_eq!(output.status, ToolOutputStatus::Success);
        let stdout = output.stdout.text.as_deref().expect("stdout text");
        assert!(stdout.contains(r#""ok": true"#));
        assert!(stdout.contains(r#""active_count": 0"#));
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
            "stack_logs_fetch".to_string(),
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
                    "stack_logs_fetch".to_string(),
                ],
                &available_tools,
                "test-task",
            )
            .expect("non-manager tool should survive filtering");

        assert_eq!(allowed, HashSet::from(["write_todos".to_string()]));
    }

    #[test]
    fn sub_agent_progress_filter_drops_agent_scoped_events() {
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
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::LoopDetected {
                loop_type: crate::agent::loop_detection::LoopType::ToolCallLoop,
                iteration: 3,
            }
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::Cancelling {
                tool_name: "execute_command".to_string(),
            }
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::Cancelled
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::Error("LLM call failed".to_string())
        ));
        assert!(!should_forward_sub_agent_progress_event(
            &AgentEvent::Finished
        ));
        assert!(should_forward_sub_agent_progress_event(
            &AgentEvent::ToolCall {
                id: "tool-1".to_string(),
                source: AgentEventSource::Root,
                name: "execute_command".to_string(),
                input: "{\"command\":\"pwd\"}".to_string(),
                command_preview: Some("pwd".to_string()),
            }
        ));
    }

    #[tokio::test]
    async fn sub_agent_progress_relay_filters_snapshot_events() {
        let (parent_tx, mut parent_rx) = mpsc::channel(8);
        let (sub_tx, relay_task) = spawn_sub_agent_progress_relay(
            Some(&parent_tx),
            "sub-1".to_string(),
            "debug-bug".to_string(),
        );
        let sub_tx = sub_tx.expect("relay tx");

        sub_tx
            .send(AgentEvent::Thinking {
                snapshot: sample_snapshot(64_000),
            })
            .await
            .expect("thinking send");
        sub_tx
            .send(AgentEvent::ToolCall {
                id: "tool-1".to_string(),
                source: AgentEventSource::Root,
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
        assert!(matches!(
            forwarded,
            AgentEvent::SubAgent {
                sub_agent_id,
                sub_agent_name,
                event,
            } if sub_agent_id == "sub-1"
                && sub_agent_name == "debug-bug"
                && matches!(*event, AgentEvent::ToolCall {
                    source: AgentEventSource::SubAgent,
                    ..
                })
        ));
        assert!(parent_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn sub_agent_progress_relay_drops_loop_notifications() {
        let (parent_tx, mut parent_rx) = mpsc::channel(8);
        let (sub_tx, relay_task) = spawn_sub_agent_progress_relay(
            Some(&parent_tx),
            "sub-1".to_string(),
            "debug-bug".to_string(),
        );
        let sub_tx = sub_tx.expect("relay tx");

        sub_tx
            .send(AgentEvent::LoopDetected {
                loop_type: crate::agent::loop_detection::LoopType::ToolCallLoop,
                iteration: 7,
            })
            .await
            .expect("loop send");

        drop(sub_tx);
        if let Some(task) = relay_task {
            task.await.expect("relay task join");
        }
        drop(parent_tx);

        assert!(parent_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn sub_agent_progress_relay_drops_terminal_status_events() {
        let (parent_tx, mut parent_rx) = mpsc::channel(8);
        let (sub_tx, relay_task) = spawn_sub_agent_progress_relay(
            Some(&parent_tx),
            "sub-1".to_string(),
            "debug-bug".to_string(),
        );
        let sub_tx = sub_tx.expect("relay tx");

        sub_tx
            .send(AgentEvent::Cancelling {
                tool_name: "execute_command".to_string(),
            })
            .await
            .expect("cancelling send");
        sub_tx
            .send(AgentEvent::Cancelled)
            .await
            .expect("cancelled send");
        sub_tx
            .send(AgentEvent::Error("LLM call failed".to_string()))
            .await
            .expect("error send");
        sub_tx
            .send(AgentEvent::Finished)
            .await
            .expect("finished send");

        drop(sub_tx);
        if let Some(task) = relay_task {
            task.await.expect("relay task join");
        }
        drop(parent_tx);

        assert!(parent_rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn prepare_sub_agent_execution_applies_sub_agent_budget_policy() {
        let settings = Arc::new(AgentSettings {
            sub_agent_model_id: Some("sub-model".to_string()),
            sub_agent_model_provider: Some("mock".to_string()),
            sub_agent_max_output_tokens: Some(12_345),
            sub_agent_context_window_tokens: Some(96_000),
            ..AgentSettings::default()
        });
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);

        let mut prepared = provider
            .prepare_sub_agent_execution(
                &json!({
                    "task": "Inspect the workspace.",
                    "tools": ["write_todos"],
                    "context": "Keep notes concise."
                })
                .to_string(),
                &HashSet::new(),
                None,
                None,
            )
            .await
            .expect("sub-agent preparation succeeds");

        assert_eq!(prepared.runner_config.model_name, "sub-model");
        assert_eq!(prepared.runner_config.model_max_output_tokens, 12_345);
        assert_eq!(prepared.sub_session.memory().max_tokens(), 96_000);
        assert_eq!(
            prepared.tool_runtime_registry.specs().len(),
            prepared.tools.len()
        );
        assert!(
            prepared
                .tool_runtime_registry
                .get(&ToolName::from("write_todos"))
                .is_some()
        );

        let ctx = DelegationProvider::build_sub_agent_runner_context(&mut prepared);
        assert!(ctx.tool_runtime_registry.is_some());
    }

    #[tokio::test]
    async fn prepare_sub_agent_execution_inherits_topic_agents_md() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .return_once(|user_id, topic_id| {
                Ok(Some(crate::storage::TopicAgentsMdRecord {
                    schema_version: 1,
                    version: 2,
                    user_id,
                    topic_id,
                    agents_md: "# Topic AGENTS\nStay grounded in official docs.".to_string(),
                    created_at: 10,
                    updated_at: 20,
                }))
            });

        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings)
                .with_topic_agents_md_context(Arc::new(storage), 77, "topic-a");

        let prepared = provider
            .prepare_sub_agent_execution(
                &json!({
                    "task": "Inspect the workspace.",
                    "tools": ["write_todos"]
                })
                .to_string(),
                &HashSet::new(),
                None,
                None,
            )
            .await
            .expect("sub-agent preparation succeeds");

        let messages = prepared.sub_session.memory().get_messages();
        assert_eq!(messages.len(), 2);
        assert!(messages[0].content.contains("[TOPIC_AGENTS_MD]"));
        assert!(messages[0].content.contains("# Topic AGENTS"));
        assert!(messages[1].content.contains("Inspect the workspace."));
    }

    #[tokio::test]
    async fn prepare_sub_agent_execution_fails_when_topic_agents_md_load_fails() {
        let mut storage = MockStorageProvider::new();
        storage
            .expect_get_topic_agents_md()
            .with(eq(77_i64), eq("topic-a".to_string()))
            .return_once(|_, _| {
                Err(crate::storage::StorageError::Config(
                    "storage unavailable".to_string(),
                ))
            });

        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings)
                .with_topic_agents_md_context(Arc::new(storage), 77, "topic-a");

        let error = match provider
            .prepare_sub_agent_execution(
                &json!({
                    "task": "Inspect the workspace.",
                    "tools": ["write_todos"]
                })
                .to_string(),
                &HashSet::new(),
                None,
                None,
            )
            .await
        {
            Ok(_) => panic!("sub-agent preparation should fail closed on AGENTS load errors"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("Failed to load topic AGENTS.md for sub-agent bootstrap")
        );
    }

    #[test]
    fn build_sub_agent_tool_runtime_executors_do_not_expose_compress() {
        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);
        let todos = Arc::new(tokio::sync::Mutex::new(TodoList::new()));
        let executors =
            provider.build_sub_agent_tool_runtime_executors(todos, test_memory_scope(), None);
        let tools: HashSet<String> = executors
            .iter()
            .map(|executor| executor.name().into_inner())
            .collect();

        assert!(!tools.contains("compress"));
    }

    #[test]
    fn build_sub_agent_tool_runtime_executors_use_narrow_sandbox_surface() {
        let settings = Arc::new(AgentSettings::default());
        let provider =
            DelegationProvider::new(Arc::new(LlmClient::new(&settings)), 1_i64, settings);
        let todos = Arc::new(tokio::sync::Mutex::new(TodoList::new()));
        let executors =
            provider.build_sub_agent_tool_runtime_executors(todos, test_memory_scope(), None);
        let tools: HashSet<String> = executors
            .iter()
            .map(|executor| executor.name().into_inner())
            .collect();

        assert!(!tools.contains("send_file_to_user"));
        assert!(!tools.contains("upload_file"));
        assert!(!tools.contains("recreate_sandbox"));

        #[cfg(feature = "tool-sandbox-exec")]
        assert!(tools.contains("execute_command"));
        #[cfg(feature = "tool-sandbox-fileops")]
        for tool in ["write_file", "read_file", "apply_file_edit", "list_files"] {
            assert!(tools.contains(tool), "missing sandbox fileops tool: {tool}");
        }
    }

    #[test]
    fn build_sub_agent_wiki_memory_policy_keeps_read_only_tools_available() {
        let blocked: HashSet<&str> = super::BLOCKED_SUB_AGENT_TOOLS.iter().copied().collect();

        assert!(!blocked.contains("wiki_memory_list"));
        assert!(!blocked.contains("wiki_memory_read"));
        assert!(blocked.contains("wiki_memory_delete"));
    }

    #[test]
    fn shape_sub_agent_terminal_output_maps_user_input_pause_to_error_report() {
        let settings = Arc::new(AgentSettings::default());
        let mut session = EphemeralSession::new(512);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect the workspace."));

        let report = DelegationProvider::shape_sub_agent_terminal_output_for_settings(
            settings.as_ref(),
            TimedRunResult::WaitingForUserInput(PendingUserInput {
                kind: UserInputKind::Text,
                prompt: "Need confirmation".to_string(),
            }),
            "sub-task-1",
            session.memory(),
        );

        assert!(report.contains(r#""status": "error""#));
        assert!(report.contains("unsupported external approval"));
        assert!(report.contains("sub-task-1"));
    }

    #[test]
    fn shape_sub_agent_terminal_output_maps_timeout_to_timeout_report() {
        let settings = Arc::new(AgentSettings {
            sub_agent_timeout_secs: Some(45),
            ..AgentSettings::default()
        });
        let mut session = EphemeralSession::new(512);
        session
            .memory_mut()
            .add_message(AgentMessage::user_task("Inspect the workspace."));

        let report = DelegationProvider::shape_sub_agent_terminal_output_for_settings(
            settings.as_ref(),
            TimedRunResult::TimedOut,
            "sub-task-2",
            session.memory(),
        );

        assert!(report.contains(r#""status": "timeout""#));
        assert!(report.contains("Sub-agent hard timed out after 75 seconds"));
        assert!(report.contains(r#""timeout_secs": 45"#));
    }

    fn sample_snapshot(context_window_tokens: usize) -> TokenSnapshot {
        TokenSnapshot {
            hot_memory_tokens: 777,
            system_prompt_tokens: 500,
            tool_schema_tokens: 600,
            total_input_tokens: 2_900,
            reserved_output_tokens: 0,
            hard_reserve_tokens: 8_192,
            projected_total_tokens: 11_092,
            context_window_tokens,
            headroom_tokens: context_window_tokens.saturating_sub(11_092),
            budget_state: BudgetState::Warning,
            last_api_usage: None,
        }
    }
}
