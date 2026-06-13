//! Session management for the web transport.
//!
//! Provides a high-level session service that:
//! - creates / retrieves / removes sessions via `SessionRegistry`
//! - tracks `TaskTimeline` for latency measurements
//! - owns `InMemoryStorage` and `WebAgentTransport` instances
//!
//! ## Durable memory persistence
//!
//! On each task completion the `StorageFlowCheckpoint` persists `AgentMemory`
//! to durable storage at key:
//!
//! ```text
//! users/{user_id}/topics/{context_key}/flows/{flow_id}/memory.json
//! ```
//!
//! When a web session is re-created after a backend restart, the
//! [`WebSessionManager::create_session_with_id`] method hydrates the
//! previously persisted `AgentMemory` from storage **before** injecting
//! topic `AGENTS.md` or installing the checkpoint. This ensures the agent
//! retains full conversation history across restarts.
//!
//! Identifiers (`context_key`, `agent_flow_id`) are stable across restarts
//! because they are persisted in the `WebSessionRecord` and reused by
//! `ensure_runtime_session` when the runtime session is re-created.

use crate::in_memory_storage::InMemoryStorage;
use crate::web_transport::TaskEventLog;
use chrono::{DateTime, Utc};
use oxide_agent_core::agent::memory::{AgentMessage, MessageRole};
use oxide_agent_core::agent::providers::ReminderContext;
use oxide_agent_core::agent::{
    AgentExecutionProfile, AgentExecutor, AgentMemory, AgentMemoryScope, AgentSession, SessionId,
    ToolAccessPolicy,
};
use oxide_agent_core::config::{
    AgentSettings, DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS,
    DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS, ModelInfo,
};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::{ReminderThreadKind, StorageProvider};
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_web_contracts::ModelSelection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use uuid::Uuid;

const WEB_LATENCY_TARGET: &str = "oxide_agent_transport_web::web_latency";

fn log_session_create_phase(
    user_id: i64,
    session_id: &str,
    context_key: &str,
    agent_flow_id: &str,
    phase: &str,
    started_at: Instant,
    phase_started_at: Instant,
) {
    tracing::debug!(
        target: WEB_LATENCY_TARGET,
        user_id,
        session_id = %session_id,
        context_key = %context_key,
        agent_flow_id = %agent_flow_id,
        phase,
        phase_ms = phase_started_at.elapsed().as_millis(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "web session runtime create latency"
    );
}

/// Session metadata returned via HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionMeta {
    pub session_id: String,
    pub user_id: i64,
    pub context_key: String,
    pub agent_flow_id: String,
    #[serde(default)]
    pub model_selection: Option<ModelSelection>,
    #[serde(default)]
    pub agent_profile_id: Option<String>,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Processing,
    Completed,
    TimedOut,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct WebSessionRuntimeOptions {
    pub model_selection: Option<ModelSelection>,
    pub agent_profile_id: Option<String>,
    pub execution_profile: Option<AgentExecutionProfile>,
    pub skip_fresh_durable_bootstrap: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchProbeRuntimeOptions {
    pub(crate) tool_allowlist: Vec<String>,
    pub(crate) prompt_instructions: Option<String>,
}

/// Task metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskMeta {
    pub task_id: String,
    pub session_id: String,
    pub task_text: String,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// Task timeline with latency milestones.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskTimeline {
    pub task_id: String,
    pub session_id: String,
    /// Milliseconds from HTTP request received to each milestone.
    pub milestones: TaskMilestones,
    /// Individual tool call timings.
    pub tool_calls: Vec<ToolCallTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskMilestones {
    /// When the HTTP request handler started processing.
    pub http_received_ms: i64,
    /// When the executor lock was acquired and session prepared.
    pub session_ready_ms: Option<i64>,
    /// When the first `AgentEvent::Thinking` was received.
    pub first_thinking_ms: Option<i64>,
    /// When the final response was produced.
    pub final_response_ms: Option<i64>,
    /// When memory was persisted to storage.
    pub memory_persisted_ms: Option<i64>,
}

impl TaskMilestones {
    pub fn new(received_at: DateTime<Utc>) -> Self {
        Self {
            http_received_ms: -(Utc::now()
                .signed_duration_since(received_at)
                .num_milliseconds()), // offset from now, corrected on read
            session_ready_ms: None,
            first_thinking_ms: None,
            final_response_ms: None,
            memory_persisted_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolCallTiming {
    pub name: String,
    /// Offset in milliseconds from `http_received`.
    pub started_at_ms: i64,
    /// Offset in milliseconds from `http_received`.
    pub finished_at_ms: Option<i64>,
}

/// Handle to a running task within a session.
#[derive(Clone)]
pub struct RunningTask {
    pub task_id: String,
    pub event_log: TaskEventLog,
    pub timeline: Arc<RwLock<TaskMilestones>>,
    pub status: Arc<RwLock<TaskStatus>>,
}

impl RunningTask {
    pub fn new(task_id: String) -> Self {
        Self {
            task_id,
            event_log: TaskEventLog::new(),
            timeline: Arc::new(RwLock::new(TaskMilestones::new(Utc::now()))),
            status: Arc::new(RwLock::new(TaskStatus::Running)),
        }
    }
}

/// Manages all sessions for the web transport.
pub struct WebSessionManager {
    /// Shared session registry from the runtime crate.
    registry: SessionRegistry,
    /// In-memory storage shared by all sessions.
    storage: Arc<dyn StorageProvider>,
    /// LLM client for agent execution.
    llm: Arc<LlmClient>,
    /// Agent settings (copied from config).
    agent_settings: Arc<AgentSettings>,
    /// Per-session metadata.
    sessions: Arc<RwLock<HashMap<String, SessionMeta>>>,
    /// Per-task metadata and timelines.
    tasks: Arc<RwLock<HashMap<String, TaskMeta>>>,
    /// Active running tasks keyed by task_id.
    running_tasks: Arc<RwLock<HashMap<String, RunningTask>>>,
}

#[must_use]
pub(crate) fn web_session_sandbox_scope(user_id: i64, context_key: &str) -> SandboxScope {
    SandboxScope::new(user_id, context_key.to_string())
}

fn parse_web_model_id(value: &str) -> Option<(String, String)> {
    let value = value.trim();
    if let Some(model_id) = value.strip_prefix("opencode-go/") {
        let model_id = model_id.trim();
        return (!model_id.is_empty() && !model_id.contains('/'))
            .then(|| ("opencode-go".to_string(), model_id.to_string()));
    }
    if let Some(model_id) = value.strip_prefix("opencode-zen/") {
        let model_id = model_id.trim();
        return (!model_id.is_empty() && !model_id.contains('/'))
            .then(|| ("opencode-zen".to_string(), model_id.to_string()));
    }
    if let Some(rest) = value.strip_prefix("openai-base:") {
        let (name, model_id) = rest.split_once('/')?;
        let name = normalized_openai_base_instance_name(name)?;
        let model_id = model_id.trim();
        return (!model_id.is_empty())
            .then(|| (format!("openai-base:{name}"), model_id.to_string()));
    }
    if value.is_empty() || value.contains('/') {
        return None;
    }
    Some(("opencode-go".to_string(), value.to_string()))
}

fn web_qualified_model_id_for_prefix(value: &str, model_prefix: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("opencode-go/")
        || value.starts_with("opencode-zen/")
        || value.starts_with("openai-base:")
    {
        let (prefix, model_id) = parse_web_model_id(value)?;
        return (prefix == model_prefix).then(|| format!("{prefix}/{model_id}"));
    }
    if value.is_empty() || (!is_openai_base_prefix(model_prefix) && value.contains('/')) {
        return None;
    }
    Some(format!("{model_prefix}/{value}"))
}

fn raw_model_id_for_prefix(value: &str, model_prefix: &str) -> Option<String> {
    let qualified = web_qualified_model_id_for_prefix(value, model_prefix)?;
    qualified
        .strip_prefix(&format!("{model_prefix}/"))
        .map(ToString::to_string)
}

fn selected_web_model_route(
    selected_qualified_id: &str,
    selected_prefix: &str,
    provider: &str,
    configured_routes: &[ModelInfo],
) -> ModelInfo {
    configured_routes
        .iter()
        .find(|route| {
            web_model_provider_prefix(&route.provider).as_deref() == Some(selected_prefix)
                && web_qualified_model_id_for_prefix(&route.id, selected_prefix).as_deref()
                    == Some(selected_qualified_id)
        })
        .cloned()
        .and_then(|route| normalize_model_route(route, provider, provider))
        .unwrap_or_else(|| ModelInfo {
            id: if is_openai_base_prefix(selected_prefix) {
                raw_model_id_for_prefix(selected_qualified_id, selected_prefix)
                    .unwrap_or_else(|| selected_qualified_id.to_string())
            } else {
                selected_qualified_id.to_string()
            },
            provider: provider.to_string(),
            max_output_tokens: DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS,
            context_window_tokens: DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS,
            weight: 1,
        })
}

fn normalize_model_route(
    mut route: ModelInfo,
    opencode_go_provider: &str,
    opencode_zen_provider: &str,
) -> Option<ModelInfo> {
    let id = route.id.trim();
    let provider = route.provider.trim();
    if id.is_empty() || provider.is_empty() {
        return None;
    }

    if let Some(model_prefix) = web_model_provider_prefix(provider) {
        route.id = if is_openai_base_prefix(&model_prefix) {
            raw_model_id_for_prefix(id, &model_prefix)?
        } else {
            web_qualified_model_id_for_prefix(id, &model_prefix)?
        };
        route.provider = if model_prefix == "opencode-zen" {
            opencode_zen_provider.to_string()
        } else if is_openai_base_prefix(&model_prefix) {
            preferred_provider_name(&model_prefix, opencode_go_provider)
        } else {
            opencode_go_provider.to_string()
        };
    } else {
        route.id = id.to_string();
        route.provider = provider.to_string();
    }
    if route.max_output_tokens == 0 {
        route.max_output_tokens = DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS;
    }
    if route.context_window_tokens == 0 {
        route.context_window_tokens = DEFAULT_AGENT_MODEL_CONTEXT_WINDOW_TOKENS;
    }
    route.weight = route.weight.max(1);
    Some(route)
}

fn normalized_provider_name(provider: &str) -> String {
    provider
        .trim()
        .strip_prefix("llm-provider/")
        .unwrap_or(provider.trim())
        .replace('_', "-")
        .to_ascii_lowercase()
}

fn web_model_provider_prefix(provider: &str) -> Option<String> {
    let normalized = normalized_provider_name(provider);
    match normalized.as_str() {
        "opencode-go" => Some("opencode-go".to_string()),
        "opencode-zen" => Some("opencode-zen".to_string()),
        _ => normalized
            .strip_prefix("openai-base:")
            .and_then(normalized_openai_base_instance_name)
            .map(|name| format!("openai-base:{name}")),
    }
}

fn preferred_provider_name(model_prefix: &str, fallback: &str) -> String {
    if is_openai_base_prefix(model_prefix) {
        model_prefix.to_string()
    } else {
        fallback.to_string()
    }
}

fn is_openai_base_prefix(prefix: &str) -> bool {
    prefix.starts_with("openai-base:")
}

fn normalized_openai_base_instance_name(name: &str) -> Option<String> {
    let name = name.trim().replace('_', "-").to_ascii_lowercase();
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return None;
    }
    Some(name)
}

impl WebSessionManager {
    /// Create a new session manager.
    ///
    /// Requires a pre-configured `LlmClient` and `AgentSettings`.
    /// The `SessionRegistry` is shared with other transports.
    pub fn new(
        registry: SessionRegistry,
        llm: Arc<LlmClient>,
        agent_settings: Arc<AgentSettings>,
    ) -> Self {
        Self::new_with_storage(
            registry,
            llm,
            agent_settings,
            Arc::new(InMemoryStorage::new()),
        )
    }

    /// Create a new session manager using an explicit storage provider.
    ///
    /// Production web console setup passes the durable SQLx/Postgres provider here so
    /// runtime memory, wiki memory, topic AGENTS.md and reminder context share
    /// the same storage backend as the rest of the application.
    pub fn new_with_storage(
        registry: SessionRegistry,
        llm: Arc<LlmClient>,
        agent_settings: Arc<AgentSettings>,
        storage: Arc<dyn StorageProvider>,
    ) -> Self {
        Self {
            registry,
            storage,
            llm,
            agent_settings,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            running_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Storage shared by all sessions managed here.
    #[must_use]
    pub fn storage(&self) -> Arc<dyn StorageProvider> {
        self.storage.clone()
    }

    /// Access the underlying session registry (for test use).
    #[must_use]
    pub fn session_registry(&self) -> &SessionRegistry {
        &self.registry
    }

    /// Shared LLM client for agent execution and auto-title generation.
    #[must_use]
    pub fn llm_client(&self) -> Arc<LlmClient> {
        self.llm.clone()
    }

    /// Agent settings including configured model routes.
    #[must_use]
    pub fn agent_settings(&self) -> Arc<AgentSettings> {
        self.agent_settings.clone()
    }

    async fn model_routes_override_for_selection(
        &self,
        selection: Option<&ModelSelection>,
    ) -> Option<Vec<ModelInfo>> {
        let selection = selection?;
        let (selected_prefix, _) = parse_web_model_id(&selection.qualified_id)?;
        let selected_model_id =
            web_qualified_model_id_for_prefix(&selection.qualified_id, &selected_prefix)?;
        let configured_routes = self.agent_settings.get_configured_agent_model_routes();
        let selected_provider = self.preferred_web_model_provider_name(&selected_prefix);
        let selected_route = selected_web_model_route(
            &selected_model_id,
            &selected_prefix,
            &selected_provider,
            &configured_routes,
        );

        Some(vec![selected_route])
    }

    fn preferred_web_model_provider_name(&self, model_prefix: &str) -> String {
        if is_openai_base_prefix(model_prefix) {
            return model_prefix.to_string();
        }
        let (dash, underscore) = match model_prefix {
            "opencode-zen" => ("opencode-zen", "opencode_zen"),
            _ => ("opencode-go", "opencode_go"),
        };
        if self.llm.is_provider_available(dash) {
            dash.to_string()
        } else if self.llm.is_provider_available(underscore) {
            underscore.to_string()
        } else {
            dash.to_string()
        }
    }

    pub(crate) async fn create_search_probe_executor(
        &self,
        parent_session_id: &str,
        options: SearchProbeRuntimeOptions,
    ) -> Option<AgentExecutor> {
        let parent_meta = self.get_session(parent_session_id).await?;
        let probe_sid = derive_search_probe_session_id(parent_session_id);
        let session = AgentSession::new(probe_sid);
        let mut executor =
            AgentExecutor::new(self.llm.clone(), session, self.agent_settings.clone());

        if let Some(model_routes) = self
            .model_routes_override_for_selection(parent_meta.model_selection.as_ref())
            .await
        {
            executor.set_model_routes_override(model_routes);
        }

        executor.set_execution_profile(search_probe_execution_profile(options));
        Some(executor)
    }

    pub(crate) async fn last_main_agent_final_message(
        &self,
        parent_session_id: &str,
    ) -> Option<String> {
        let sid = self.resolve_session_id(parent_session_id).await?;
        let executor_arc = self.registry.get(&sid).await?;
        let executor = executor_arc.read().await;
        executor
            .session()
            .memory
            .get_messages()
            .iter()
            .rev()
            .find(|message| {
                let content = message.content.trim();
                message.role == MessageRole::Assistant
                    && !content.is_empty()
                    && message
                        .tool_calls
                        .as_ref()
                        .is_none_or(std::vec::Vec::is_empty)
            })
            .map(|message| message.content.trim().to_string())
    }

    // --- Session CRUD ---

    /// Create a new session and register it in the `SessionRegistry`.
    ///
    /// Returns the session_id.
    pub async fn create_session(
        &self,
        user_id: i64,
        context_key: Option<String>,
        agent_flow_id: Option<String>,
    ) -> String {
        let session_id = Uuid::new_v4().to_string();
        let context_key = context_key.unwrap_or_else(|| "default".to_string());
        let agent_flow_id = agent_flow_id.unwrap_or_else(|| "default".to_string());
        self.create_session_with_id(user_id, session_id, context_key, agent_flow_id)
            .await
    }

    /// Create a new session with an externally selected public session id.
    ///
    /// Used by the production `/api/v1` web console API so the persisted web
    /// session record and runtime memory scope can share the same id-derived
    /// context key.
    ///
    /// ## Initialization order
    ///
    /// 1. Create an empty `AgentSession` with stable scopes.
    /// 2. Hydrate `AgentMemory` from durable storage (if available), except
    ///    for newly-created fresh web sessions where no durable key can exist yet.
    /// 3. Restore volatile execution metadata derived from persisted memory.
    /// 4. Inject topic `AGENTS.md` only if the restored memory does not already
    ///    contain a pinned copy.
    /// 5. Install the storage checkpoint so subsequent task execution persists
    ///    memory snapshots to the same durable key.
    /// 6. Create the `AgentExecutor` and register it.
    pub async fn create_session_with_id(
        &self,
        user_id: i64,
        session_id: String,
        context_key: String,
        agent_flow_id: String,
    ) -> String {
        self.create_session_with_model_selection(
            user_id,
            session_id,
            context_key,
            agent_flow_id,
            WebSessionRuntimeOptions::default(),
        )
        .await
    }

    pub async fn create_session_with_model_selection(
        &self,
        user_id: i64,
        session_id: String,
        context_key: String,
        agent_flow_id: String,
        options: WebSessionRuntimeOptions,
    ) -> String {
        let started_at = Instant::now();
        let mut phase_started_at = started_at;
        let WebSessionRuntimeOptions {
            model_selection,
            agent_profile_id,
            execution_profile,
            skip_fresh_durable_bootstrap,
        } = options;
        let has_execution_profile = execution_profile.is_some();
        let skip_fresh_durable_bootstrap =
            skip_fresh_durable_bootstrap && is_fresh_web_session_context(&context_key);

        let session_id_i64 = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            session_id.hash(&mut h);
            user_id.hash(&mut h);
            h.finish() as i64
        };

        let sid = SessionId::from(session_id_i64);
        let sandbox_scope = web_session_sandbox_scope(user_id, &context_key);
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "runtime_ids_prepared",
            started_at,
            phase_started_at,
        );
        phase_started_at = Instant::now();

        let mut session = AgentSession::new_with_scopes(
            sid,
            sandbox_scope,
            AgentMemoryScope::new(user_id, context_key.clone(), agent_flow_id.clone()),
        );
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "agent_session_created",
            started_at,
            phase_started_at,
        );
        phase_started_at = Instant::now();

        // 1. Hydrate persisted AgentMemory from durable storage.
        //    This restores conversation history across backend restarts. Fresh
        //    web sessions skip this read because their scoped memory key was
        //    just created and cannot have a persisted snapshot yet.
        if !skip_fresh_durable_bootstrap {
            self.hydrate_agent_memory(
                &mut session,
                user_id,
                &session_id,
                &context_key,
                &agent_flow_id,
            )
            .await;
        }
        let hydrated_message_count = session.memory.get_messages().len();
        tracing::debug!(
            target: WEB_LATENCY_TARGET,
            user_id,
            session_id = %session_id,
            context_key = %context_key,
            agent_flow_id = %agent_flow_id,
            message_count = hydrated_message_count,
            durable_load_skipped = skip_fresh_durable_bootstrap,
            phase = "agent_memory_hydrated",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = started_at.elapsed().as_millis(),
            "web session runtime create latency"
        );
        phase_started_at = Instant::now();

        // 2. Inject topic AGENTS.md only if the restored memory does not
        //    already contain a pinned copy from durable storage. Fresh web
        //    sessions skip this read because their unique context has no
        //    topic-scoped AGENTS.md yet.
        if !skip_fresh_durable_bootstrap {
            inject_topic_agents_md_for_session(
                self.storage(),
                user_id,
                context_key.clone(),
                &mut session,
            )
            .await;
        }
        tracing::debug!(
            target: WEB_LATENCY_TARGET,
            user_id,
            session_id = %session_id,
            context_key = %context_key,
            agent_flow_id = %agent_flow_id,
            durable_load_skipped = skip_fresh_durable_bootstrap,
            phase = "topic_agents_md_injected",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = started_at.elapsed().as_millis(),
            "web session runtime create latency"
        );
        phase_started_at = Instant::now();

        // 3. Attach a checkpoint backed by the manager storage so memory
        //    survives across tasks and follows the configured durable backend.
        //    Must be installed AFTER hydration so the first checkpoint write
        //    reflects the full hydrated state, not an empty snapshot.
        session.set_memory_checkpoint(Arc::new(StorageFlowCheckpoint {
            storage: self.storage(),
            user_id,
            context_key: context_key.clone(),
            agent_flow_id: agent_flow_id.clone(),
        }));
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "memory_checkpoint_installed",
            started_at,
            phase_started_at,
        );
        phase_started_at = Instant::now();

        // 4. Create executor only after the session is fully hydrated.
        let mut executor =
            AgentExecutor::new(self.llm.clone(), session, self.agent_settings.clone())
                .with_wiki_memory_store(oxide_agent_core::agent::WikiStore::from_storage_provider(
                    self.storage(),
                    "",
                ));
        executor.set_agents_md_context(self.storage(), user_id, context_key.clone());
        executor.set_reminder_context(ReminderContext {
            storage: self.storage(),
            user_id,
            context_key: context_key.clone(),
            flow_id: agent_flow_id.clone(),
            chat_id: user_id,
            thread_id: None,
            thread_kind: ReminderThreadKind::None,
            notifier: None,
        });
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "agent_executor_created",
            started_at,
            phase_started_at,
        );
        phase_started_at = Instant::now();

        if let Some(model_routes) = self
            .model_routes_override_for_selection(model_selection.as_ref())
            .await
        {
            executor.set_model_routes_override(model_routes);
        }
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "model_routes_resolved",
            started_at,
            phase_started_at,
        );
        phase_started_at = Instant::now();

        if let Some(execution_profile) = execution_profile {
            executor.set_execution_profile(execution_profile);
        }

        self.registry.insert(sid, executor).await;
        tracing::debug!(
            target: WEB_LATENCY_TARGET,
            user_id,
            session_id = %session_id,
            context_key = %context_key,
            agent_flow_id = %agent_flow_id,
            agent_profile_id = ?agent_profile_id,
            has_model_selection = model_selection.is_some(),
            has_execution_profile,
            phase = "runtime_registry_inserted",
            phase_ms = phase_started_at.elapsed().as_millis(),
            elapsed_ms = started_at.elapsed().as_millis(),
            "web session runtime create latency"
        );
        phase_started_at = Instant::now();

        let meta = SessionMeta {
            session_id: session_id.clone(),
            user_id,
            context_key: context_key.clone(),
            agent_flow_id: agent_flow_id.clone(),
            model_selection,
            agent_profile_id,
            status: SessionStatus::Idle,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
        };

        self.sessions.write().await.insert(session_id.clone(), meta);
        log_session_create_phase(
            user_id,
            &session_id,
            &context_key,
            &agent_flow_id,
            "web_session_meta_inserted",
            started_at,
            phase_started_at,
        );
        tracing::debug!(
            target: WEB_LATENCY_TARGET,
            user_id,
            session_id = %session_id,
            context_key = %context_key,
            agent_flow_id = %agent_flow_id,
            elapsed_ms = started_at.elapsed().as_millis(),
            "web session runtime create completed"
        );
        session_id
    }

    /// Get session metadata.
    pub async fn get_session(&self, session_id: &str) -> Option<SessionMeta> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// Replace the execution profile for an already materialized runtime session.
    pub async fn set_session_execution_profile(
        &self,
        session_id: &str,
        agent_profile_id: Option<String>,
        execution_profile: Option<AgentExecutionProfile>,
    ) -> bool {
        let Some(meta) = self.sessions.read().await.get(session_id).cloned() else {
            return false;
        };
        let sid = derive_web_session_id(meta.user_id, session_id);
        let Some(executor_arc) = self.registry.get(&sid).await else {
            return false;
        };
        {
            let mut executor = executor_arc.write().await;
            executor.set_execution_profile(execution_profile.unwrap_or_default());
        }
        if let Some(meta) = self.sessions.write().await.get_mut(session_id) {
            meta.agent_profile_id = agent_profile_id;
            meta.last_activity_at = Utc::now();
        }
        true
    }

    /// Delete a session from the runtime registry.
    pub async fn delete_session(&self, session_id: &str) -> bool {
        let sid = self.resolve_session_id(session_id).await;
        if let Some(sid) = sid {
            self.registry.remove(&sid).await;
        }
        self.sessions.write().await.remove(session_id).is_some()
    }

    // --- Task lifecycle ---

    /// Register a new task and return the RunningTask handle.
    ///
    /// Does NOT start execution — use `start_task_execution` for that.
    pub async fn register_task(&self, session_id: &str, task_text: String) -> Option<RunningTask> {
        if let Some(sid) = self.resolve_session_id(session_id).await {
            self.registry.renew_cancellation_token(&sid).await;
        }

        // Update session last_activity_at.
        {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.last_activity_at = Utc::now();
                meta.status = SessionStatus::Processing;
            } else {
                return None;
            }
        }

        let task_id = Uuid::new_v4().to_string();
        let task_meta = TaskMeta {
            task_id: task_id.clone(),
            session_id: session_id.to_string(),
            task_text: task_text.clone(),
            status: TaskStatus::Running,
            created_at: Utc::now(),
            finished_at: None,
        };

        let running = RunningTask::new(task_id.clone());

        self.tasks.write().await.insert(task_id.clone(), task_meta);
        self.running_tasks
            .write()
            .await
            .insert(task_id.clone(), running.clone());

        Some(running)
    }

    /// Register an existing task id for a resumed run and return the new
    /// in-memory running handle.
    pub async fn register_existing_task(
        &self,
        session_id: &str,
        task_id: &str,
        task_text: String,
    ) -> Option<RunningTask> {
        if let Some(sid) = self.resolve_session_id(session_id).await {
            self.registry.renew_cancellation_token(&sid).await;
        }

        {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.last_activity_at = Utc::now();
                meta.status = SessionStatus::Processing;
            } else {
                return None;
            }
        }

        let task_id = task_id.to_string();
        {
            let mut tasks = self.tasks.write().await;
            let created_at = tasks
                .get(&task_id)
                .map_or_else(Utc::now, |meta| meta.created_at);
            tasks.insert(
                task_id.clone(),
                TaskMeta {
                    task_id: task_id.clone(),
                    session_id: session_id.to_string(),
                    task_text,
                    status: TaskStatus::Running,
                    created_at,
                    finished_at: None,
                },
            );
        }

        let running = RunningTask::new(task_id.clone());
        self.running_tasks
            .write()
            .await
            .insert(task_id, running.clone());

        Some(running)
    }

    /// Mark a task as completed.
    pub async fn complete_task(&self, task_id: &str, session_id: &str) {
        let mut update_session = true;
        {
            let mut tasks = self.tasks.write().await;
            if let Some(meta) = tasks.get_mut(task_id) {
                if meta.status == TaskStatus::Cancelled {
                    update_session = false;
                } else {
                    meta.status = TaskStatus::Completed;
                    meta.finished_at = Some(Utc::now());
                }
            }
        }
        if update_session {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.status = SessionStatus::Idle;
            }
        }
        self.running_tasks.write().await.remove(task_id);
    }

    /// Mark a task as failed.
    pub async fn fail_task(&self, task_id: &str, session_id: &str) {
        let mut update_session = true;
        {
            let mut tasks = self.tasks.write().await;
            if let Some(meta) = tasks.get_mut(task_id) {
                if meta.status == TaskStatus::Cancelled {
                    update_session = false;
                } else {
                    meta.status = TaskStatus::Failed;
                    meta.finished_at = Some(Utc::now());
                }
            }
        }
        if update_session {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.status = SessionStatus::Error;
            }
        }
        self.running_tasks.write().await.remove(task_id);
    }

    /// Cancel a running task.
    pub async fn cancel_task(&self, task_id: &str, session_id: &str) -> bool {
        let sid = self.resolve_session_id(session_id).await;
        if let Some(sid) = sid {
            self.registry.cancel(&sid).await;
        }
        {
            let mut tasks = self.tasks.write().await;
            if let Some(meta) = tasks.get_mut(task_id) {
                meta.status = TaskStatus::Cancelled;
                meta.finished_at = Some(Utc::now());
            }
        }
        {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.status = SessionStatus::Idle;
                meta.last_activity_at = Utc::now();
            }
        }
        self.running_tasks.write().await.remove(task_id).is_some()
    }

    /// Get a running task handle by id.
    pub async fn get_running_task(&self, task_id: &str) -> Option<RunningTask> {
        self.running_tasks.read().await.get(task_id).cloned()
    }

    /// Return an active in-memory running task for a session, if one exists.
    pub async fn running_task_for_session(&self, session_id: &str) -> Option<String> {
        let running_task_ids = self
            .running_tasks
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let tasks = self.tasks.read().await;
        running_task_ids.into_iter().find(|task_id| {
            tasks.get(task_id).is_some_and(|meta| {
                meta.session_id == session_id && meta.status == TaskStatus::Running
            })
        })
    }

    /// Get task metadata.
    pub async fn get_task(&self, task_id: &str) -> Option<TaskMeta> {
        self.tasks.read().await.get(task_id).cloned()
    }

    /// Hydrate `AgentMemory` from durable storage for an existing web session.
    ///
    /// When a web session is re-created (e.g. after a backend restart), the
    /// runtime `AgentSession` starts with an empty memory. This method loads
    /// the previously persisted memory snapshot from durable storage and
    /// restores volatile execution metadata (such as `last_task`) that is
    /// derivable from the loaded messages.
    ///
    /// Storage key: `users/{user_id}/topics/{context_key}/flows/{flow_id}/memory.json`
    ///
    /// If the load fails, the session continues with empty memory and a
    /// warning is logged. A production setup with `OXIDE_WEB_REQUIRE_DURABLE_STORAGE=true`
    /// should fail at startup (in `build_r2_backed_app_state`) before reaching
    /// this point if the storage backend is unavailable.
    async fn hydrate_agent_memory(
        &self,
        session: &mut AgentSession,
        user_id: i64,
        web_session_id: &str,
        context_key: &str,
        agent_flow_id: &str,
    ) {
        tracing::debug!(
            user_id,
            web_session_id,
            context_key,
            agent_flow_id,
            "hydrating web agent session from durable storage"
        );

        match self
            .storage
            .load_agent_memory_for_flow(user_id, context_key.to_string(), agent_flow_id.to_string())
            .await
        {
            Ok(Some(memory)) => {
                let message_count = memory.get_messages().len();
                tracing::info!(
                    user_id,
                    web_session_id,
                    context_key,
                    agent_flow_id,
                    message_count,
                    "web agent memory hydrated from durable storage"
                );

                session.memory = memory;
                session.restore_last_task_from_memory();
            }

            Ok(None) => {
                tracing::debug!(
                    user_id,
                    web_session_id,
                    context_key,
                    agent_flow_id,
                    "no persisted web agent memory found; starting with empty memory"
                );
            }

            Err(error) => {
                tracing::warn!(
                    user_id,
                    web_session_id,
                    context_key,
                    agent_flow_id,
                    ?error,
                    "failed to load persisted web agent memory; continuing with empty memory"
                );
            }
        }
    }

    /// Resolve session_id string to `SessionId`.
    async fn resolve_session_id(&self, session_id: &str) -> Option<SessionId> {
        let sessions = self.sessions.read().await;
        let meta = sessions.get(session_id)?;
        // Re-derive the SessionId using the same hash as create_session.
        let sid = derive_web_session_id(meta.user_id, session_id);
        Some(sid)
    }
}

/// Derive SessionId from user_id and session_id string.
///
/// Must match the logic in `create_session`.
fn derive_web_session_id(user_id: i64, session_id: &str) -> SessionId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    user_id.hash(&mut h);
    SessionId::from(h.finish() as i64)
}

fn derive_search_probe_session_id(parent_session_id: &str) -> SessionId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    "search-probe".hash(&mut h);
    parent_session_id.hash(&mut h);
    Uuid::new_v4().hash(&mut h);
    SessionId::from(h.finish() as i64)
}

fn search_probe_execution_profile(options: SearchProbeRuntimeOptions) -> AgentExecutionProfile {
    let allowed_tools = options
        .tool_allowlist
        .into_iter()
        .map(|tool| tool.trim().to_string())
        .filter(|tool| !tool.is_empty())
        .filter(|tool| tool != "crawl4ai_markdown")
        .collect::<HashSet<_>>();
    AgentExecutionProfile::new(
        Some("search-probe".to_string()),
        options.prompt_instructions,
        ToolAccessPolicy::new(Some(allowed_tools), HashSet::new()),
    )
}

fn is_fresh_web_session_context(context_key: &str) -> bool {
    context_key.starts_with("web-session-")
}

/// Agent memory checkpoint that delegates to the configured storage provider.
struct StorageFlowCheckpoint {
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    agent_flow_id: String,
}

async fn inject_topic_agents_md_for_session(
    storage: Arc<dyn StorageProvider>,
    user_id: i64,
    context_key: String,
    session: &mut AgentSession,
) {
    if session.memory.has_topic_agents_md() {
        return;
    }

    let topic_agents_md = match storage.get_topic_agents_md(user_id, context_key).await {
        Ok(record) => record.map(|record| record.agents_md),
        Err(_) => None,
    };

    let Some(topic_agents_md) = topic_agents_md.map(|content| content.trim().to_string()) else {
        return;
    };
    if topic_agents_md.is_empty() {
        return;
    }

    session
        .memory
        .add_message(AgentMessage::topic_agents_md(topic_agents_md));
}

#[async_trait::async_trait]
impl oxide_agent_core::agent::AgentMemoryCheckpoint for StorageFlowCheckpoint {
    async fn persist(&self, memory: &AgentMemory) -> Result<(), anyhow::Error> {
        self.storage
            .save_agent_memory_for_flow(
                self.user_id,
                self.context_key.clone(),
                self.agent_flow_id.clone(),
                memory,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_memory_storage::InMemoryStorage;
    use oxide_agent_core::agent::compaction::AgentMessageKind;
    use oxide_agent_core::storage::UpsertTopicAgentsMdOptions;

    /// Helper: build a `WebSessionManager` backed by an explicit storage provider.
    fn make_manager_with_storage(storage: Arc<dyn StorageProvider>) -> WebSessionManager {
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        WebSessionManager::new_with_storage(registry, llm, settings, storage)
    }

    /// Helper: resolve a session_id string to its `AgentExecutor` read handle.
    ///
    /// Returns the `Arc<RwLock<AgentExecutor>>` from the registry so the
    /// caller can `.read().await` on it without lifetime issues.
    async fn resolve_executor_arc(
        manager: &WebSessionManager,
        session_id: &str,
    ) -> Arc<tokio::sync::RwLock<oxide_agent_core::agent::AgentExecutor>> {
        let sid = manager
            .resolve_session_id(session_id)
            .await
            .expect("session id must resolve");
        manager
            .session_registry()
            .get(&sid)
            .await
            .expect("executor must exist")
    }

    // -----------------------------------------------------------------------
    // Existing test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_session_bootstraps_topic_agents_md_into_memory() {
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let manager = WebSessionManager::new(registry, llm, settings);
        let storage = manager.storage();
        storage
            .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                user_id: 77,
                topic_id: "topic-a".to_string(),
                agents_md: "# Topic AGENTS\nBootstrap instructions".to_string(),
            })
            .await
            .expect("topic AGENTS.md must be stored");

        let session_id = manager
            .create_session(77, Some("topic-a".to_string()), Some("flow-a".to_string()))
            .await;
        let sid = manager
            .resolve_session_id(&session_id)
            .await
            .expect("session id must resolve");
        let executor = manager
            .session_registry()
            .get(&sid)
            .await
            .expect("executor must exist");
        let executor = executor.read().await;

        assert!(executor.session().memory.has_topic_agents_md());
        assert!(
            executor
                .session()
                .memory
                .get_messages()
                .iter()
                .any(|message| message.content.contains("Bootstrap instructions"))
        );
    }

    #[tokio::test]
    async fn fresh_web_session_can_skip_initial_durable_bootstrap_reads() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let manager = make_manager_with_storage(storage.clone());
        let user_id = 78;
        let session_id = "fresh-skip".to_string();
        let context_key = "web-session-fresh-skip".to_string();
        let agent_flow_id = "main".to_string();

        let mut memory = AgentMemory::new(usize::MAX);
        memory.add_message(AgentMessage::user_task("persisted task should not load"));
        storage
            .save_agent_memory_for_flow(
                user_id,
                context_key.clone(),
                agent_flow_id.clone(),
                &memory,
            )
            .await
            .expect("memory should be saved");
        storage
            .upsert_topic_agents_md(UpsertTopicAgentsMdOptions {
                user_id,
                topic_id: context_key.clone(),
                agents_md: "# Topic AGENTS\nShould not inject".to_string(),
            })
            .await
            .expect("topic AGENTS.md should be stored");

        manager
            .create_session_with_model_selection(
                user_id,
                session_id.clone(),
                context_key,
                agent_flow_id,
                WebSessionRuntimeOptions {
                    skip_fresh_durable_bootstrap: true,
                    ..WebSessionRuntimeOptions::default()
                },
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, &session_id).await;
        let executor = executor_arc.read().await;

        assert!(executor.session().memory.get_messages().is_empty());
        assert!(executor.session().last_task.is_none());
    }

    #[tokio::test]
    async fn web_session_uses_context_scoped_sandbox_scope() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let manager = make_manager_with_storage(storage);
        let context_key = "web-session-scope-test".to_string();

        manager
            .create_session_with_id(
                91,
                "scope-test".to_string(),
                context_key.clone(),
                "main".to_string(),
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "scope-test").await;
        let executor = executor_arc.read().await;

        assert_eq!(
            executor.session().sandbox_scope().namespace(),
            context_key,
            "web sessions should use a per-session sandbox namespace"
        );
    }

    #[tokio::test]
    async fn web_session_applies_model_route_override_from_selection() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings {
            agent_model_routes: Some(vec![
                ModelInfo {
                    id: "opencode-go/deepseek-v4-flash".to_string(),
                    provider: "opencode_go".to_string(),
                    max_output_tokens: 32_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                },
                ModelInfo {
                    id: "mistral-large".to_string(),
                    provider: "mistral".to_string(),
                    max_output_tokens: 16_000,
                    context_window_tokens: 128_000,
                    weight: 1,
                },
            ]),
            ..AgentSettings::default()
        });
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let manager = WebSessionManager::new_with_storage(registry, llm, settings, storage);

        manager
            .create_session_with_model_selection(
                91,
                "model-selection-test".to_string(),
                "web-session-model-selection-test".to_string(),
                "main".to_string(),
                WebSessionRuntimeOptions {
                    model_selection: Some(ModelSelection {
                        qualified_id: "opencode-go/kimi-k2.6".to_string(),
                    }),
                    ..WebSessionRuntimeOptions::default()
                },
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "model-selection-test").await;
        let executor = executor_arc.read().await;
        let routes = executor
            .model_routes_override()
            .expect("model route override should be set");

        assert_eq!(
            routes.len(),
            1,
            "web model selection must not add fallback routes"
        );
        assert_eq!(routes[0].id, "opencode-go/kimi-k2.6");
        assert_eq!(routes[0].provider, "opencode-go");
        assert_eq!(
            routes[0].max_output_tokens,
            DEFAULT_AGENT_MODEL_MAX_OUTPUT_TOKENS
        );
        assert!(
            routes
                .iter()
                .all(|route| route.id != "opencode-go/deepseek-v4-flash")
        );
        assert!(routes.iter().all(|route| route.provider != "mistral"));
    }

    #[tokio::test]
    async fn web_session_applies_opencode_zen_model_route_override_from_selection() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings {
            agent_model_routes: Some(vec![
                ModelInfo {
                    id: "opencode-zen/deepseek-v4-flash-free".to_string(),
                    provider: "opencode_zen".to_string(),
                    max_output_tokens: 16_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                },
                ModelInfo {
                    id: "opencode-go/deepseek-v4-flash".to_string(),
                    provider: "opencode_go".to_string(),
                    max_output_tokens: 32_000,
                    context_window_tokens: 200_000,
                    weight: 1,
                },
            ]),
            ..AgentSettings::default()
        });
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let manager = WebSessionManager::new_with_storage(registry, llm, settings, storage);

        manager
            .create_session_with_model_selection(
                91,
                "zen-model-selection-test".to_string(),
                "web-session-zen-selection-test".to_string(),
                "main".to_string(),
                WebSessionRuntimeOptions {
                    model_selection: Some(ModelSelection {
                        qualified_id: "opencode-zen/deepseek-v4-flash-free".to_string(),
                    }),
                    ..WebSessionRuntimeOptions::default()
                },
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "zen-model-selection-test").await;
        let executor = executor_arc.read().await;
        let routes = executor
            .model_routes_override()
            .expect("model route override should be set");

        assert_eq!(
            routes.len(),
            1,
            "web model selection must not add fallback routes"
        );
        assert_eq!(routes[0].id, "opencode-zen/deepseek-v4-flash-free");
        assert_eq!(routes[0].provider, "opencode-zen");
        assert!(
            routes
                .iter()
                .all(|route| route.id != "opencode-go/deepseek-v4-flash")
        );
    }

    #[tokio::test]
    async fn web_session_applies_openai_base_model_route_override_from_selection() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings {
            agent_model_routes: Some(vec![ModelInfo {
                id: "hf.co/test/model".to_string(),
                provider: "openai-base:local".to_string(),
                max_output_tokens: 8_000,
                context_window_tokens: 64_000,
                weight: 1,
            }]),
            ..AgentSettings::default()
        });
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let manager = WebSessionManager::new_with_storage(registry, llm, settings, storage);

        manager
            .create_session_with_model_selection(
                91,
                "openai-base-model-selection-test".to_string(),
                "web-session-openai-base-selection-test".to_string(),
                "main".to_string(),
                WebSessionRuntimeOptions {
                    model_selection: Some(ModelSelection {
                        qualified_id: "openai-base:local/hf.co/test/model".to_string(),
                    }),
                    ..WebSessionRuntimeOptions::default()
                },
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "openai-base-model-selection-test").await;
        let executor = executor_arc.read().await;
        let routes = executor
            .model_routes_override()
            .expect("model route override should be set");

        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id, "hf.co/test/model");
        assert_eq!(routes[0].provider, "openai-base:local");
        assert_eq!(routes[0].max_output_tokens, 8_000);
        assert_eq!(routes[0].context_window_tokens, 64_000);
    }

    #[tokio::test]
    async fn search_probe_executor_inherits_model_route_without_registry_insert() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let registry = SessionRegistry::new();
        let settings = Arc::new(AgentSettings {
            agent_model_routes: Some(vec![ModelInfo {
                id: "opencode-go/deepseek-v4-flash".to_string(),
                provider: "opencode_go".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]),
            ..AgentSettings::default()
        });
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        let manager = WebSessionManager::new_with_storage(registry, llm, settings, storage);

        manager
            .create_session_with_model_selection(
                91,
                "search-probe-model-selection-test".to_string(),
                "web-session-search-probe-model-selection-test".to_string(),
                "main".to_string(),
                WebSessionRuntimeOptions {
                    model_selection: Some(ModelSelection {
                        qualified_id: "opencode-go/deepseek-v4-flash".to_string(),
                    }),
                    ..WebSessionRuntimeOptions::default()
                },
            )
            .await;
        let registry_len_before = manager.session_registry().len().await;

        let probe = manager
            .create_search_probe_executor(
                "search-probe-model-selection-test",
                SearchProbeRuntimeOptions {
                    tool_allowlist: vec!["web_markdown".to_string()],
                    prompt_instructions: Some("probe instructions".to_string()),
                },
            )
            .await
            .expect("probe executor should be created for existing web session");
        let registry_len_after = manager.session_registry().len().await;

        assert_eq!(registry_len_before, 1);
        assert_eq!(registry_len_after, registry_len_before);
        assert_ne!(
            probe.session().session_id,
            derive_web_session_id(91, "search-probe-model-selection-test")
        );
        assert!(probe.session().memory.get_messages().is_empty());

        let routes = probe
            .model_routes_override()
            .expect("probe should inherit selected model route");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id, "opencode-go/deepseek-v4-flash");
        assert_eq!(routes[0].provider, "opencode-go");
        assert_eq!(routes[0].max_output_tokens, 32_000);
    }

    #[tokio::test]
    async fn last_main_agent_final_message_reads_latest_parent_assistant_response() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let manager = make_manager_with_storage(storage);

        manager
            .create_session_with_id(
                92,
                "search-probe-final-message-test".to_string(),
                "web-session-search-probe-final-message-test".to_string(),
                "main".to_string(),
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "search-probe-final-message-test").await;
        {
            let mut executor = executor_arc.write().await;
            executor
                .session_mut()
                .memory
                .add_message(AgentMessage::user_task("Initial question"));
            executor
                .session_mut()
                .memory
                .add_message(AgentMessage::assistant("Older final answer"));
            executor
                .session_mut()
                .memory
                .add_message(AgentMessage::assistant("Latest final answer  "));
        }

        let message = manager
            .last_main_agent_final_message("search-probe-final-message-test")
            .await;

        assert_eq!(message.as_deref(), Some("Latest final answer"));
        assert_eq!(manager.last_main_agent_final_message("missing").await, None);
    }

    #[tokio::test]
    async fn search_probe_executor_applies_tool_allowlist() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let manager = make_manager_with_storage(storage);

        manager
            .create_session_with_id(
                92,
                "search-probe-tool-policy-test".to_string(),
                "web-session-search-probe-tool-policy-test".to_string(),
                "main".to_string(),
            )
            .await;

        let probe = manager
            .create_search_probe_executor(
                "search-probe-tool-policy-test",
                SearchProbeRuntimeOptions {
                    tool_allowlist: vec!["definitely_not_a_real_tool".to_string()],
                    prompt_instructions: None,
                },
            )
            .await
            .expect("probe executor should be created for existing web session");

        assert!(probe.current_tool_definitions().is_empty());

        let probe = manager
            .create_search_probe_executor(
                "search-probe-tool-policy-test",
                SearchProbeRuntimeOptions {
                    tool_allowlist: vec![
                        "web_markdown".to_string(),
                        "crawl4ai_markdown".to_string(),
                    ],
                    prompt_instructions: None,
                },
            )
            .await
            .expect("probe executor should be created for existing web session");

        let tool_names = probe
            .current_tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(tool_names.contains("web_markdown"));
        assert!(!tool_names.contains("crawl4ai_markdown"));
    }

    #[tokio::test]
    async fn search_probe_executor_requires_existing_parent_session() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());
        let manager = make_manager_with_storage(storage);

        let probe = manager
            .create_search_probe_executor(
                "missing-session",
                SearchProbeRuntimeOptions {
                    tool_allowlist: vec!["web_markdown".to_string()],
                    prompt_instructions: None,
                },
            )
            .await;

        assert!(probe.is_none());
        assert_eq!(manager.session_registry().len().await, 0);
    }

    // -----------------------------------------------------------------------
    // Regression: memory hydration after manager restart
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn web_session_hydrates_agent_memory_after_manager_restart() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());

        let user_id = 42i64;
        let context_key = "web-session-abc123".to_string();
        let agent_flow_id = "main".to_string();
        let session_id = "abc123".to_string();

        // --- Phase 1: simulate first session run ---------------------------
        {
            let manager_1 = make_manager_with_storage(storage.clone());
            manager_1
                .create_session_with_id(
                    user_id,
                    session_id.clone(),
                    context_key.clone(),
                    agent_flow_id.clone(),
                )
                .await;

            // Persist a memory snapshot that contains a user task.
            let mut memory = AgentMemory::new(usize::MAX);
            memory.add_message(AgentMessage::user_task("Проверь работу песочницы"));
            storage
                .save_agent_memory_for_flow(
                    user_id,
                    context_key.clone(),
                    agent_flow_id.clone(),
                    &memory,
                )
                .await
                .expect("memory should be saved");
        }
        // manager_1 dropped — simulates backend restart.

        // --- Phase 2: re-create session from the same storage ---------------
        {
            let manager_2 = make_manager_with_storage(storage.clone());
            manager_2
                .create_session_with_id(
                    user_id,
                    session_id.clone(),
                    context_key.clone(),
                    agent_flow_id.clone(),
                )
                .await;

            let executor_arc = resolve_executor_arc(&manager_2, &session_id).await;
            let executor = executor_arc.read().await;
            let messages = executor.session().memory.get_messages();

            let has_user_task = messages.iter().any(|msg| {
                msg.kind == AgentMessageKind::UserTask
                    && msg.content.contains("Проверь работу песочницы")
            });
            assert!(
                has_user_task,
                "web session should hydrate AgentMemory from durable storage after restart"
            );

            // Verify restore_last_task_from_memory was effective.
            assert_eq!(
                executor.session().last_task.as_deref(),
                Some("Проверь работу песочницы"),
                "restore_last_task_from_memory should recover the last user task"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Regression: no duplicate AGENTS.md after hydration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn web_session_hydration_does_not_duplicate_topic_agents_md() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());

        let user_id = 55i64;
        let context_key = "web-session-dup-test".to_string();
        let agent_flow_id = "main".to_string();
        let session_id = "dup-test".to_string();

        // Persist memory that already contains a pinned AGENTS.md message.
        {
            let mut memory = AgentMemory::new(usize::MAX);
            memory.add_message(AgentMessage::topic_agents_md("# Topic Instructions"));
            storage
                .save_agent_memory_for_flow(
                    user_id,
                    context_key.clone(),
                    agent_flow_id.clone(),
                    &memory,
                )
                .await
                .expect("memory should be saved");
        }

        let manager = make_manager_with_storage(storage.clone());
        manager
            .create_session_with_id(
                user_id,
                session_id.clone(),
                context_key.clone(),
                agent_flow_id.clone(),
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, &session_id).await;
        let executor = executor_arc.read().await;
        let topic_md_count = executor
            .session()
            .memory
            .get_messages()
            .iter()
            .filter(|msg| msg.kind == AgentMessageKind::TopicAgentsMd)
            .count();

        assert_eq!(
            topic_md_count, 1,
            "AGENTS.md should not be duplicated after hydration; found {topic_md_count}"
        );
    }

    // -----------------------------------------------------------------------
    // No memory in storage -> empty session with no panic
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn web_session_starts_empty_when_no_persisted_memory() {
        let storage: Arc<dyn StorageProvider> = Arc::new(InMemoryStorage::new());

        let manager = make_manager_with_storage(storage);
        manager
            .create_session_with_id(
                10,
                "fresh-session".to_string(),
                "fresh-ctx".to_string(),
                "main".to_string(),
            )
            .await;

        let executor_arc = resolve_executor_arc(&manager, "fresh-session").await;
        let executor = executor_arc.read().await;
        assert!(
            executor.session().memory.get_messages().is_empty(),
            "session should start with empty memory when nothing is persisted"
        );
        assert!(
            executor.session().last_task.is_none(),
            "last_task should be None when memory is empty"
        );
    }
}
