//! Session management for the web transport.
//!
//! Provides a high-level session service that:
//! - creates / retrieves / removes sessions via `SessionRegistry`
//! - tracks `TaskTimeline` for latency measurements
//! - owns `InMemoryStorage` and `WebAgentTransport` instances

use crate::in_memory_storage::InMemoryStorage;
use crate::web_transport::TaskEventLog;
use chrono::{DateTime, Utc};
use oxide_agent_core::agent::memory::AgentMessage;
use oxide_agent_core::agent::{AgentExecutor, AgentMemory, AgentSession, SessionId};
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::llm::LlmClient;
use oxide_agent_core::sandbox::SandboxScope;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::SessionRegistry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Session metadata returned via HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionMeta {
    pub session_id: String,
    pub user_id: i64,
    pub context_key: String,
    pub agent_flow_id: String,
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
        Self {
            registry,
            storage: Arc::new(InMemoryStorage::new()),
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
        let session_id_i64 = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            session_id.hash(&mut h);
            user_id.hash(&mut h);
            h.finish() as i64
        };

        let sid = SessionId::from(session_id_i64);
        let sandbox_scope = SandboxScope::new(user_id, "web");

        let mut session = AgentSession::new_with_sandbox_scope(sid, sandbox_scope);
        inject_topic_agents_md_for_session(
            self.storage(),
            user_id,
            context_key.clone().unwrap_or_else(|| "default".to_string()),
            &mut session,
        )
        .await;

        // Attach in-memory checkpoint so memory survives across tasks.
        session.set_memory_checkpoint(Arc::new(InMemoryFlowCheckpoint {
            storage: Arc::new(InMemoryStorage::new()),
            user_id,
            context_key: context_key.clone().unwrap_or_else(|| "default".to_string()),
            agent_flow_id: agent_flow_id
                .clone()
                .unwrap_or_else(|| "default".to_string()),
        }));

        let mut executor =
            AgentExecutor::new(self.llm.clone(), session, self.agent_settings.clone());
        executor.set_agents_md_context(
            self.storage(),
            user_id,
            context_key.clone().unwrap_or_else(|| "default".to_string()),
        );

        self.registry.insert(sid, executor).await;

        let meta = SessionMeta {
            session_id: session_id.clone(),
            user_id,
            context_key: context_key.unwrap_or_else(|| "default".to_string()),
            agent_flow_id: agent_flow_id.unwrap_or_else(|| "default".to_string()),
            status: SessionStatus::Idle,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
        };

        self.sessions.write().await.insert(session_id.clone(), meta);
        session_id
    }

    /// Get session metadata.
    pub async fn get_session(&self, session_id: &str) -> Option<SessionMeta> {
        self.sessions.read().await.get(session_id).cloned()
    }

    /// Delete a session and cancel any running task.
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

    /// Mark a task as completed.
    pub async fn complete_task(&self, task_id: &str, session_id: &str) {
        {
            let mut tasks = self.tasks.write().await;
            if let Some(meta) = tasks.get_mut(task_id) {
                meta.status = TaskStatus::Completed;
                meta.finished_at = Some(Utc::now());
            }
        }
        {
            let mut sessions = self.sessions.write().await;
            if let Some(meta) = sessions.get_mut(session_id) {
                meta.status = SessionStatus::Idle;
            }
        }
        self.running_tasks.write().await.remove(task_id);
    }

    /// Mark a task as failed.
    pub async fn fail_task(&self, task_id: &str, session_id: &str) {
        {
            let mut tasks = self.tasks.write().await;
            if let Some(meta) = tasks.get_mut(task_id) {
                meta.status = TaskStatus::Failed;
                meta.finished_at = Some(Utc::now());
            }
        }
        {
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
        self.running_tasks.write().await.remove(task_id).is_some()
    }

    /// Get a running task handle by id.
    pub async fn get_running_task(&self, task_id: &str) -> Option<RunningTask> {
        self.running_tasks.read().await.get(task_id).cloned()
    }

    /// Get task metadata.
    pub async fn get_task(&self, task_id: &str) -> Option<TaskMeta> {
        self.tasks.read().await.get(task_id).cloned()
    }

    /// Resolve session_id string to `SessionId`.
    async fn resolve_session_id(&self, session_id: &str) -> Option<SessionId> {
        let sessions = self.sessions.read().await;
        let meta = sessions.get(session_id)?;
        // Re-derive the SessionId using the same hash as create_session.
        let sid = derive_web_session_id(meta.user_id, session_id);
        Some(sid)
    }

    /// Access the underlying session registry (for execute_agent_task).
    #[must_use]
    pub fn registry(&self) -> &SessionRegistry {
        &self.registry
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

/// In-memory checkpoint that delegates to `InMemoryStorage`.
struct InMemoryFlowCheckpoint {
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
impl oxide_agent_core::agent::AgentMemoryCheckpoint for InMemoryFlowCheckpoint {
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
    use oxide_agent_core::storage::UpsertTopicAgentsMdOptions;

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
        assert!(executor
            .session()
            .memory
            .get_messages()
            .iter()
            .any(|message| message.content.contains("Bootstrap instructions")));
    }
}
