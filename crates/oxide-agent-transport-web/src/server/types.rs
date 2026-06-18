//! Core types, constants, and environment helpers for the web transport server.

use crate::persistence::{InMemoryWebUiStore, WebAuthSessionRecord, WebUiStore};
use crate::session::WebSessionManager;
use anyhow::Result as AnyResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use moka::future::Cache;
#[cfg(not(feature = "socket_e2e"))]
use oxide_agent_core::sandbox::{SandboxAdmin, SandboxAdminRuntime};
use oxide_agent_core::sandbox::{SandboxContainerRecord, SandboxScope, sandbox_backend_available};
use oxide_agent_core::storage::StorageProvider;
#[cfg(feature = "storage-sqlx")]
use oxide_agent_core::storage::{SqlxStorage, SqlxStorageConfig};
#[cfg(feature = "storage-sqlx")]
use oxide_agent_core::{config::AgentSettings, llm::LlmClient};
#[cfg(feature = "storage-sqlx")]
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_web_contracts::{
    CurrentUser, ListAgentProfilesResponse, ListSessionsResponse, UserSettingsResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap as StdHashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const AUTH_COOKIE_NAME: &str = "oxide_web_session";
pub(crate) const CSRF_HEADER_NAME: &str = "x-csrf-token";
pub(crate) const WEB_SESSION_SCHEMA_VERSION: u32 = 1;
pub(crate) const WEB_TASK_SCHEMA_VERSION: u32 = 1;
pub(crate) const WEB_SESSION_FLOW_ID: &str = "main";
pub(crate) const WEB_SESSION_DEFAULT_TITLE: &str = "New session";
pub(crate) const MAX_SESSION_TITLE_CHARS: usize = 160;
pub(crate) const MAX_TASK_INPUT_CHARS: usize = 65_536;
pub(crate) const TASK_PREVIEW_CHARS: usize = 96;
pub(crate) const DEFAULT_TASK_EVENTS_LIMIT: usize = 200;
pub(crate) const MAX_TASK_EVENTS_LIMIT: usize = 500;
pub(crate) const DEFAULT_WEB_CHAT_UPLOAD_MAX_MB: u64 = 200;
pub(crate) const DEFAULT_WEB_MAX_SANDBOX_CONTAINERS_PER_USER: usize = 10;
pub(crate) const AUTH_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
pub(crate) const AUTH_RATE_LIMIT_MAX_FAILURES: u32 = 5;
pub(crate) const AUTH_CACHE_TTL: Duration = Duration::from_secs(60);
pub(crate) const AUTH_CACHE_MAX_CAPACITY: u64 = 1024;
pub(crate) const USER_SETTINGS_CACHE_TTL: Duration = Duration::from_secs(60);
/// How long a closed `TaskEventLog` stays queryable in the global
/// `EVENT_LOGS` registry for late subscribers before a background cleanup
/// task removes it. 60s is long enough for a browser tab to reconnect
/// after a network blip, short enough to keep the map bounded.
pub(crate) const EVENT_LOG_RETENTION_AFTER_CLOSE: Duration = Duration::from_secs(60);
pub(crate) const USER_SETTINGS_CACHE_MAX_CAPACITY: u64 = 1024;
pub(crate) const AGENT_PROFILES_CACHE_TTL: Duration = Duration::from_secs(60);
pub(crate) const AGENT_PROFILES_CACHE_MAX_CAPACITY: u64 = 1024;
pub(crate) const SESSION_SUMMARIES_CACHE_TTL: Duration = Duration::from_secs(15);
pub(crate) const SESSION_SUMMARIES_CACHE_MAX_CAPACITY: u64 = 1024;

#[derive(Debug, Clone)]
pub(crate) struct CachedAuthSession {
    pub user: CurrentUser,
    pub auth_session: WebAuthSessionRecord,
    pub cached_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Store kind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebStoreKind {
    InMemory,
    Sqlx,
    Custom,
}

// ---------------------------------------------------------------------------
// Sandbox admin facade
// ---------------------------------------------------------------------------

#[async_trait]
pub(crate) trait WebSandboxControl: Send + Sync {
    async fn destroy_scope(&self, scope: SandboxScope) -> AnyResult<()>;

    async fn list_user_sandboxes(&self, user_id: i64) -> AnyResult<Vec<SandboxContainerRecord>>;

    async fn delete_sandbox_by_name(&self, user_id: i64, container_name: &str) -> AnyResult<bool>;
}

#[cfg(not(feature = "socket_e2e"))]
#[derive(Default)]
struct RuntimeWebSandboxControl {
    admin: SandboxAdminRuntime,
}

#[cfg(not(feature = "socket_e2e"))]
#[async_trait]
impl WebSandboxControl for RuntimeWebSandboxControl {
    async fn destroy_scope(&self, scope: SandboxScope) -> AnyResult<()> {
        self.admin.destroy_scope(scope).await
    }

    async fn list_user_sandboxes(&self, user_id: i64) -> AnyResult<Vec<SandboxContainerRecord>> {
        self.admin.list_user_sandboxes(user_id).await
    }

    async fn delete_sandbox_by_name(&self, user_id: i64, container_name: &str) -> AnyResult<bool> {
        self.admin
            .delete_sandbox_by_name(user_id, container_name)
            .await
    }
}

#[cfg(feature = "socket_e2e")]
#[derive(Default)]
struct NoopWebSandboxControl;

#[cfg(feature = "socket_e2e")]
#[async_trait]
impl WebSandboxControl for NoopWebSandboxControl {
    async fn destroy_scope(&self, _scope: SandboxScope) -> AnyResult<()> {
        Ok(())
    }

    async fn list_user_sandboxes(&self, _user_id: i64) -> AnyResult<Vec<SandboxContainerRecord>> {
        Ok(Vec::new())
    }

    async fn delete_sandbox_by_name(
        &self,
        _user_id: i64,
        _container_name: &str,
    ) -> AnyResult<bool> {
        Ok(false)
    }
}

#[cfg(feature = "socket_e2e")]
fn default_web_sandbox_control() -> Arc<dyn WebSandboxControl> {
    Arc::new(NoopWebSandboxControl)
}

#[cfg(not(feature = "socket_e2e"))]
fn default_web_sandbox_control() -> Arc<dyn WebSandboxControl> {
    Arc::new(RuntimeWebSandboxControl::default())
}

// ---------------------------------------------------------------------------
// Startup error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebStartupError {
    InMemoryStoreNotAllowed,
    StoreUnavailable(String),
    StaticAssetsUnavailable(String),
}

impl fmt::Display for WebStartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InMemoryStoreNotAllowed => write!(
                f,
                "in-memory web UI store is not allowed for this startup mode; configure SQLx/Postgres storage or set OXIDE_WEB_ALLOW_IN_MEMORY_STORE=true for explicit dev/test use"
            ),
            Self::StoreUnavailable(message) => {
                write!(f, "web UI store is unavailable during startup: {message}")
            }
            Self::StaticAssetsUnavailable(message) => {
                write!(
                    f,
                    "web UI static assets are unavailable during startup: {message}"
                )
            }
        }
    }
}

impl std::error::Error for WebStartupError {}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<WebSessionManager>,
    pub web_store: Arc<dyn WebUiStore>,
    sandbox_control: Arc<dyn WebSandboxControl>,
    pub web_store_kind: WebStoreKind,
    pub web_assets: WebAssetsConfig,
    pub(crate) auth_rate_limiter: Arc<AsyncMutex<AuthRateLimiter>>,
    pub(crate) auth_cache: Cache<String, CachedAuthSession>,
    pub(crate) user_settings_cache: Cache<i64, UserSettingsResponse>,
    pub(crate) agent_profiles_cache: Cache<i64, ListAgentProfilesResponse>,
    pub(crate) session_summaries_cache: Cache<i64, ListSessionsResponse>,
    pub task_progress: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    pub task_timeline: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    /// Tracks the JoinHandle for each running task so it can be aborted on completion.
    pub task_handles: Arc<RwLock<StdHashMap<String, Arc<tokio::task::JoinHandle<()>>>>>,
    /// When `false`, the async auto-title worker is skipped (for tests with scripted LLM).
    pub auto_title_enabled: bool,
    large_input_attachments_supported: bool,
    /// Local directory where agent tool artifacts (e.g., browser-live screenshots) are stored.
    pub artifact_dir: PathBuf,
    /// Durable storage handle for browser artifact BYTEA storage.
    /// `Some` when `WebStoreKind::Sqlx`, `None` for in-memory/custom stores.
    pub storage: Option<Arc<dyn StorageProvider>>,
}

impl AppState {
    pub fn new(session_manager: Arc<WebSessionManager>) -> Self {
        Self::new_in_memory_for_dev_test(session_manager)
    }

    pub fn new_in_memory_for_dev_test(session_manager: Arc<WebSessionManager>) -> Self {
        Self::new_with_web_store_kind(
            session_manager,
            Arc::new(InMemoryWebUiStore::new()),
            WebStoreKind::InMemory,
        )
    }

    pub fn new_with_web_store(
        session_manager: Arc<WebSessionManager>,
        web_store: Arc<dyn WebUiStore>,
    ) -> Self {
        Self::new_with_web_store_kind(session_manager, web_store, WebStoreKind::Custom)
    }

    fn new_with_web_store_kind(
        session_manager: Arc<WebSessionManager>,
        web_store: Arc<dyn WebUiStore>,
        web_store_kind: WebStoreKind,
    ) -> Self {
        Self {
            session_manager,
            web_store,
            sandbox_control: default_web_sandbox_control(),
            web_store_kind,
            web_assets: WebAssetsConfig::from_env(),
            artifact_dir: Self::artifact_dir_from_env(),
            auth_rate_limiter: Arc::new(AsyncMutex::new(AuthRateLimiter::new())),
            auth_cache: Cache::builder()
                .max_capacity(AUTH_CACHE_MAX_CAPACITY)
                .time_to_live(AUTH_CACHE_TTL)
                .build(),
            user_settings_cache: Cache::builder()
                .max_capacity(USER_SETTINGS_CACHE_MAX_CAPACITY)
                .time_to_live(USER_SETTINGS_CACHE_TTL)
                .build(),
            agent_profiles_cache: Cache::builder()
                .max_capacity(AGENT_PROFILES_CACHE_MAX_CAPACITY)
                .time_to_live(AGENT_PROFILES_CACHE_TTL)
                .build(),
            session_summaries_cache: Cache::builder()
                .max_capacity(SESSION_SUMMARIES_CACHE_MAX_CAPACITY)
                .time_to_live(SESSION_SUMMARIES_CACHE_TTL)
                .build(),
            task_progress: Arc::new(RwLock::new(StdHashMap::new())),
            task_timeline: Arc::new(RwLock::new(StdHashMap::new())),
            task_handles: Arc::new(RwLock::new(StdHashMap::new())),
            auto_title_enabled: true,
            large_input_attachments_supported: sandbox_backend_available(),
            storage: None,
        }
    }

    #[cfg(feature = "storage-sqlx")]
    pub fn new_with_sqlx_web_store(
        session_manager: Arc<WebSessionManager>,
        sqlx_storage: Arc<SqlxStorage>,
    ) -> Self {
        let storage_provider: Arc<dyn StorageProvider> =
            Arc::clone(&sqlx_storage) as Arc<dyn StorageProvider>;
        let mut state = Self::new_with_web_store_kind(
            session_manager,
            Arc::new(crate::persistence::SqlxWebUiStore::new(sqlx_storage)),
            WebStoreKind::Sqlx,
        );
        state.storage = Some(storage_provider);
        state
    }

    #[must_use]
    pub const fn web_store_kind(&self) -> WebStoreKind {
        self.web_store_kind
    }

    fn artifact_dir_from_env() -> PathBuf {
        std::env::var("OXIDE_WEB_ARTIFACT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".oxide/tool-artifacts")
            })
    }

    pub fn validate_web_store_for_startup(&self) -> Result<(), WebStartupError> {
        if self.web_store_kind == WebStoreKind::InMemory
            && !web_in_memory_store_allowed()
            && durable_web_store_required()
        {
            return Err(WebStartupError::InMemoryStoreNotAllowed);
        }
        self.web_assets.validate_for_startup()?;
        Ok(())
    }

    pub async fn reconcile_unfinished_tasks_on_startup(
        &self,
    ) -> Result<Vec<oxide_agent_web_contracts::WebTaskRecord>, WebStartupError> {
        self.web_store
            .mark_unfinished_tasks_interrupted("web backend restarted", chrono::Utc::now())
            .await
            .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))
    }

    /// Access the underlying session manager (for test use).
    #[must_use]
    pub fn session_manager(&self) -> Arc<WebSessionManager> {
        self.session_manager.clone()
    }

    /// Access the web UI store (for test use).
    #[must_use]
    pub fn web_store(&self) -> Arc<dyn WebUiStore> {
        self.web_store.clone()
    }

    #[must_use]
    pub(crate) fn sandbox_control(&self) -> Arc<dyn WebSandboxControl> {
        self.sandbox_control.clone()
    }

    /// Durable storage handle for browser artifacts. `None` when using
    /// in-memory or custom stores (browser artifacts fall back to filesystem).
    #[must_use]
    pub fn storage(&self) -> Option<Arc<dyn StorageProvider>> {
        self.storage.clone()
    }

    #[must_use]
    pub const fn large_input_attachments_supported(&self) -> bool {
        self.large_input_attachments_supported
    }

    #[cfg(test)]
    pub(crate) fn set_sandbox_control(&mut self, sandbox_control: Arc<dyn WebSandboxControl>) {
        self.sandbox_control = sandbox_control;
    }
}

// ---------------------------------------------------------------------------
// Auth rate limiter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct AuthRateLimitEntry {
    failures: u32,
    window_started: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct AuthRateLimiter {
    entries: StdHashMap<String, AuthRateLimitEntry>,
}

impl AuthRateLimiter {
    pub(crate) fn new() -> Self {
        Self {
            entries: StdHashMap::new(),
        }
    }

    pub(crate) fn is_limited(&mut self, key: &str, now: Instant) -> bool {
        let Some(entry) = self.entries.get(key) else {
            return false;
        };
        if now.duration_since(entry.window_started) >= AUTH_RATE_LIMIT_WINDOW {
            self.entries.remove(key);
            return false;
        }
        entry.failures >= AUTH_RATE_LIMIT_MAX_FAILURES
    }

    pub(crate) fn record_failure(&mut self, key: String, now: Instant) {
        let entry = self
            .entries
            .entry(key)
            .or_insert_with(|| AuthRateLimitEntry {
                failures: 0,
                window_started: now,
            });
        if now.duration_since(entry.window_started) >= AUTH_RATE_LIMIT_WINDOW {
            entry.failures = 0;
            entry.window_started = now;
        }
        entry.failures = entry.failures.saturating_add(1);
    }

    pub(crate) fn clear(&mut self, key: &str) {
        self.entries.remove(key);
    }
}

// ---------------------------------------------------------------------------
// Web assets config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAssetsConfig {
    pub dir: Option<PathBuf>,
    pub required: bool,
}

impl WebAssetsConfig {
    #[must_use]
    pub fn from_env() -> Self {
        let explicit_dir = std::env::var("OXIDE_WEB_STATIC_DIR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        let default_dir = PathBuf::from("crates/oxide-agent-web-ui/dist");
        let dir = explicit_dir.or_else(|| default_dir.exists().then_some(default_dir));
        Self {
            dir,
            required: web_static_assets_required(),
        }
    }

    #[must_use]
    pub fn disabled_for_tests() -> Self {
        Self {
            dir: None,
            required: false,
        }
    }

    #[must_use]
    pub fn required_dir_for_tests(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            required: true,
        }
    }

    pub(crate) fn validate_for_startup(&self) -> Result<(), WebStartupError> {
        if !self.required && self.dir.is_none() {
            return Ok(());
        }
        let Some(dir) = self.dir.as_deref() else {
            return Err(WebStartupError::StaticAssetsUnavailable(
                "OXIDE_WEB_STATIC_DIR is not configured and no default dist directory exists"
                    .to_string(),
            ));
        };
        let index = dir.join("index.html");
        if !index.is_file() {
            return Err(WebStartupError::StaticAssetsUnavailable(format!(
                "missing frontend index file at {}",
                index.display()
            )));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SQLx-backed app state builder
// ---------------------------------------------------------------------------

#[cfg(feature = "storage-sqlx")]
pub async fn build_sqlx_backed_app_state(
    registry: SessionRegistry,
    llm: Arc<LlmClient>,
    agent_settings: Arc<AgentSettings>,
) -> Result<AppState, WebStartupError> {
    let sqlx_config = SqlxStorageConfig::from_agent_settings(agent_settings.as_ref())
        .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?;
    let sqlx_storage = Arc::new(
        SqlxStorage::connect(sqlx_config)
            .await
            .map_err(|error| WebStartupError::StoreUnavailable(error.to_string()))?,
    );
    let provider_storage = Arc::clone(&sqlx_storage);
    let storage_provider: Arc<dyn StorageProvider> = provider_storage;
    let session_manager =
        WebSessionManager::new_with_storage(registry, llm, agent_settings, storage_provider);
    Ok(AppState::new_with_sqlx_web_store(
        Arc::new(session_manager),
        sqlx_storage,
    ))
}

// ---------------------------------------------------------------------------
// Serializable progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SerializableProgress {
    pub current_iteration: usize,
    pub max_iterations: usize,
    pub is_finished: bool,
    pub error: Option<String>,
    pub current_thought: Option<String>,
    pub current_todos: Option<serde_json::Value>,
    pub last_compaction_status: Option<String>,
    pub repeated_compaction_warning: Option<String>,
    pub last_history_repair_status: Option<String>,
    pub latest_token_snapshot: Option<oxide_agent_core::agent::progress::TokenSnapshot>,
    pub llm_retry: Option<serde_json::Value>,
    pub provider_failover_notice: Option<String>,
}

impl SerializableProgress {
    pub(crate) fn from_state(state: &oxide_agent_core::agent::progress::ProgressState) -> Self {
        Self {
            current_iteration: state.current_iteration,
            max_iterations: state.max_iterations,
            is_finished: state.is_finished,
            error: state.error.clone(),
            current_thought: state.current_thought.clone(),
            current_todos: state
                .current_todos
                .as_ref()
                .and_then(|todos| serde_json::to_value(todos).ok()),
            last_compaction_status: state.last_compaction_status.clone(),
            repeated_compaction_warning: state.repeated_compaction_warning.clone(),
            last_history_repair_status: state.last_history_repair_status.clone(),
            latest_token_snapshot: state.latest_token_snapshot.clone(),
            llm_retry: state
                .llm_retry
                .as_ref()
                .and_then(|retry| serde_json::to_value(retry).ok()),
            provider_failover_notice: state.provider_failover_notice.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Milestones & timeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Milestones {
    /// HTTP request accepted by the web transport.
    pub session_ready_ms: Option<i64>,
    /// When executor lock was actually acquired (real session ready).
    pub executor_lock_acquired_ms: Option<i64>,
    /// When prepare_execution completed.
    pub prepare_execution_done_ms: Option<i64>,
    /// When pre-run compaction completed.
    pub pre_run_compaction_done_ms: Option<i64>,
    /// When Thinking event was sent (not just received).
    pub thinking_sent_ms: Option<i64>,
    /// When first LLM call started.
    pub llm_call_started_ms: Option<i64>,
    /// When the first tool call started.
    pub first_tool_call_ms: Option<i64>,
    /// When the last tool call started.
    pub last_tool_call_ms: Option<i64>,
    /// When the first tool result was received.
    pub first_tool_result_ms: Option<i64>,
    /// When the last tool result was received.
    pub last_tool_result_ms: Option<i64>,
    /// When first Thinking event was received by collector.
    pub first_thinking_ms: Option<i64>,
    /// When final response was produced.
    pub final_response_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskTimelineRecord {
    pub milestones: Milestones,
    pub tool_calls: Vec<crate::session::ToolCallTiming>,
}

// ---------------------------------------------------------------------------
// Task events query
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TaskEventsQuery {
    #[serde(default)]
    pub after_seq: Option<u64>,
    #[serde(default)]
    pub before_seq: Option<u64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Global event log registry
// ---------------------------------------------------------------------------

lazy_static::lazy_static! {
    pub static ref EVENT_LOGS: AsyncMutex<StdHashMap<String, crate::web_transport::TaskEventLog>> =
        AsyncMutex::new(StdHashMap::new());
}

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

pub(crate) fn web_bool_env(key: &str) -> bool {
    web_env_value(key).is_some_and(|value| parse_web_bool(&value))
}

pub(crate) fn parse_web_bool(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

pub(crate) fn web_non_empty_env(key: &str) -> bool {
    web_env_value(key).is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn durable_web_store_required() -> bool {
    is_production_run_mode()
        || web_bool_env("OXIDE_WEB_ENABLED")
        || web_bool_env("OXIDE_WEB_REQUIRE_DURABLE_STORAGE")
}

pub(crate) fn web_static_assets_required() -> bool {
    is_production_run_mode() || web_bool_env("OXIDE_WEB_REQUIRE_STATIC_ASSETS")
}

pub(crate) fn web_in_memory_store_allowed() -> bool {
    web_bool_env("OXIDE_WEB_ALLOW_IN_MEMORY_STORE")
}

pub(crate) fn web_env_value(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

pub(crate) fn web_u64_env(key: &str) -> Option<u64> {
    web_env_value(key).and_then(|value| value.trim().parse::<u64>().ok())
}

pub(crate) fn web_chat_upload_limit_mb() -> u64 {
    web_u64_env("OXIDE_WEB_CHAT_UPLOAD_MAX_MB").unwrap_or(DEFAULT_WEB_CHAT_UPLOAD_MAX_MB)
}

pub(crate) fn web_max_sandbox_containers_per_user() -> usize {
    web_u64_env("OXIDE_WEB_MAX_SANDBOX_CONTAINERS_PER_USER")
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_WEB_MAX_SANDBOX_CONTAINERS_PER_USER)
}

pub(crate) fn web_bootstrap_required(
    registration_enabled: bool,
    users_count: u64,
    bootstrap_token_configured: bool,
) -> bool {
    !registration_enabled && users_count == 0 && bootstrap_token_configured
}

pub(crate) fn is_production_run_mode() -> bool {
    web_env_value("RUN_MODE").is_some_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        value == "prod" || value == "production"
    })
}
