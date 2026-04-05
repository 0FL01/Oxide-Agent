//! Agent session management
//!
//! Manages the lifecycle of an agent session, including
//! timeout tracking, session state, and sandbox.

use super::compaction::CompactionScope;
use super::identity::SessionId;
use super::memory::AgentMemory;
// use super::providers::TodoList;
use crate::config::AGENT_INTERNAL_CONTEXT_WINDOW_CAP_TOKENS;
use crate::llm::InvocationId;
use crate::sandbox::{SandboxManager, SandboxScope};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Additional user context that can be injected into a running agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeContextInjection {
    /// User-visible text payload to append on the next safe iteration boundary.
    pub content: String,
}

/// Exact SSH tool call that is paused pending operator approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSshReplay {
    /// Approval request identifier returned by the SSH provider.
    pub request_id: String,
    /// Stable internal invocation id for the paused tool call.
    pub invocation_id: InvocationId,
    /// Original tool name.
    pub tool_name: String,
    /// Original JSON arguments before approval credentials were injected.
    pub arguments: String,
}

/// Type of user input required before the task can continue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserInputKind {
    /// Free-form text response.
    Text,
    /// Single URL or direct link.
    Url,
    /// File upload or attachment.
    File,
    /// Either a URL or a file upload can resume the task.
    UrlOrFile,
}

/// Pending user input required to resume a paused task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingUserInput {
    /// Kind of input expected from the user.
    pub kind: UserInputKind,
    /// Human-readable prompt shown to the user.
    pub prompt: String,
}

/// Stable memory scope for topic-aware and flow-aware persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMemoryScope {
    /// User owning the scoped memory.
    pub user_id: i64,
    /// Stable transport context key (topic/thread scope).
    pub context_key: String,
    /// Stable flow identifier within the context.
    pub flow_id: String,
}

impl AgentMemoryScope {
    /// Create a new explicit memory scope.
    #[must_use]
    pub fn new(user_id: i64, context_key: impl Into<String>, flow_id: impl Into<String>) -> Self {
        Self {
            user_id,
            context_key: context_key.into(),
            flow_id: flow_id.into(),
        }
    }

    #[must_use]
    fn synthetic(session_id: SessionId) -> Self {
        Self {
            user_id: session_id.as_i64(),
            context_key: format!("session:{session_id}"),
            flow_id: "agent-mode".to_string(),
        }
    }

    /// Convert the memory scope into the compaction/archive scope used today.
    #[must_use]
    pub fn compaction_scope(&self) -> CompactionScope {
        CompactionScope {
            context_key: self.context_key.clone(),
            flow_id: self.flow_id.clone(),
        }
    }
}

#[async_trait]
/// Persistence hook for saving in-flight agent memory snapshots.
pub trait AgentMemoryCheckpoint: Send + Sync {
    /// Persist the provided memory snapshot.
    async fn persist(&self, memory: &AgentMemory) -> Result<()>;
}

#[cfg(not(test))]
const MEMORY_CHECKPOINT_DEBOUNCE_MS: u64 = 1_500;

#[derive(Debug, Clone)]
struct QueuedMemoryCheckpoint {
    memory: AgentMemory,
    hash: u64,
    generation: u64,
}

#[derive(Debug, Default)]
struct MemoryCheckpointState {
    last_persisted_hash: Option<u64>,
    last_persisted_generation: u64,
    next_generation: u64,
    pending: Option<QueuedMemoryCheckpoint>,
    background_task_active: bool,
}

fn checkpoint_debounce_duration() -> Duration {
    #[cfg(test)]
    {
        Duration::from_millis(20)
    }

    #[cfg(not(test))]
    {
        Duration::from_millis(MEMORY_CHECKPOINT_DEBOUNCE_MS)
    }
}

fn memory_checkpoint_hash(memory: &AgentMemory) -> Result<u64> {
    let encoded = bincode::serialize(memory)?;
    let mut hasher = DefaultHasher::new();
    encoded.hash(&mut hasher);
    Ok(hasher.finish())
}

async fn persist_queued_memory_checkpoint(
    checkpoint: Arc<dyn AgentMemoryCheckpoint>,
    state: Arc<AsyncMutex<MemoryCheckpointState>>,
    persist_lock: Arc<AsyncMutex<()>>,
    queued: QueuedMemoryCheckpoint,
    force: bool,
) -> Result<()> {
    let _persist_guard = persist_lock.lock().await;

    {
        let state = state.lock().await;
        if state.last_persisted_hash == Some(queued.hash)
            || queued.generation <= state.last_persisted_generation
        {
            return Ok(());
        }

        if !force
            && state
                .pending
                .as_ref()
                .is_some_and(|pending| pending.generation > queued.generation)
        {
            return Ok(());
        }
    }

    checkpoint.persist(&queued.memory).await?;

    let mut state = state.lock().await;
    if queued.generation > state.last_persisted_generation {
        state.last_persisted_generation = queued.generation;
        state.last_persisted_hash = Some(queued.hash);
    }

    Ok(())
}

async fn run_background_checkpoint_loop(
    checkpoint: Arc<dyn AgentMemoryCheckpoint>,
    state: Arc<AsyncMutex<MemoryCheckpointState>>,
    persist_lock: Arc<AsyncMutex<()>>,
) {
    let debounce = checkpoint_debounce_duration();

    loop {
        sleep(debounce).await;

        let queued = {
            let mut state = state.lock().await;
            match state.pending.take() {
                Some(queued) => queued,
                None => {
                    state.background_task_active = false;
                    break;
                }
            }
        };

        if let Err(error) = persist_queued_memory_checkpoint(
            Arc::clone(&checkpoint),
            Arc::clone(&state),
            Arc::clone(&persist_lock),
            queued,
            false,
        )
        .await
        {
            warn!(error = %error, "Failed to persist coalesced memory checkpoint");
        }
    }
}

/// Thread-safe inbox for runtime context injections.
#[derive(Debug, Clone, Default)]
pub struct RuntimeContextInbox {
    inner: Arc<Mutex<VecDeque<RuntimeContextInjection>>>,
}

impl RuntimeContextInbox {
    /// Create a new empty inbox.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a new runtime context payload.
    pub fn push(&self, injection: RuntimeContextInjection) {
        if let Ok(mut pending) = self.inner.lock() {
            pending.push_back(injection);
        }
    }

    /// Drain all pending runtime context payloads in FIFO order.
    #[must_use]
    pub fn drain(&self) -> Vec<RuntimeContextInjection> {
        if let Ok(mut pending) = self.inner.lock() {
            return pending.drain(..).collect();
        }

        Vec::new()
    }

    /// Returns true when there is at least one pending runtime context payload.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.inner.lock().is_ok_and(|pending| !pending.is_empty())
    }
}

/// Status of an agent session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum AgentStatus {
    /// Agent is idle, waiting for a task
    #[default]
    Idle,
    /// Agent is processing a task
    Processing {
        /// Current step description
        step: String,
        /// Estimated progress percentage (0-100)
        progress_percent: u8,
    },
    /// Agent has completed the task
    Completed,
    /// Agent timed out
    TimedOut,
    /// Agent encountered an error
    Error(String),
}

/// Represents an active agent session
pub struct AgentSession {
    /// Transport-agnostic session ID
    pub session_id: SessionId,
    /// Conversation memory for the active agent hot context
    pub memory: AgentMemory,
    /// Docker sandbox for code execution (lazily initialized)
    sandbox: Option<SandboxManager>,
    /// Stable scope used to resolve this session's persistent sandbox container.
    sandbox_scope: SandboxScope,
    /// Stable scope used by long-term memory and archive persistence.
    memory_scope: AgentMemoryScope,
    /// When the current task started
    started_at: Option<Instant>,
    /// Unique ID for the current task execution (for log correlation)
    pub current_task_id: Option<String>,
    /// Current status
    pub status: AgentStatus,
    /// Cancellation token for the current active task
    /// Set by the caller before starting a task (e.g. bot handler) so that external
    /// cancellation requests are observed by the executor.
    pub cancellation_token: CancellationToken,
    /// Last task text for retry actions.
    pub last_task: Option<String>,
    /// Loaded skills for the current system prompt or dynamic context.
    loaded_skills: HashSet<String>,
    /// Token count for loaded skills.
    skill_token_count: usize,
    /// Additional user context waiting for the next safe iteration boundary.
    runtime_context_inbox: RuntimeContextInbox,
    /// Exact SSH tool calls paused pending operator approval.
    pending_ssh_replays: Vec<PendingSshReplay>,
    /// Pending user input required before the task can resume.
    pending_user_input: Option<PendingUserInput>,
    /// Optional sink used to persist memory snapshots during long-running tasks.
    memory_checkpoint: Option<Arc<dyn AgentMemoryCheckpoint>>,
    /// Shared state for coalescing and deduplicating checkpoint writes.
    checkpoint_state: Arc<AsyncMutex<MemoryCheckpointState>>,
    /// Serializes actual checkpoint writes so stale background tasks cannot win.
    checkpoint_persist_lock: Arc<AsyncMutex<()>>,
}

impl AgentSession {
    /// Create a new agent session for a transport session
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        Self::new_with_scopes(
            session_id,
            SandboxScope::from(session_id.as_i64()),
            AgentMemoryScope::synthetic(session_id),
        )
    }

    /// Create a new agent session with an explicit sandbox scope.
    #[must_use]
    pub fn new_with_sandbox_scope(session_id: SessionId, sandbox_scope: SandboxScope) -> Self {
        Self::new_with_scopes(
            session_id,
            sandbox_scope,
            AgentMemoryScope::synthetic(session_id),
        )
    }

    /// Create a new agent session with explicit sandbox and memory scopes.
    #[must_use]
    pub fn new_with_scopes(
        session_id: SessionId,
        sandbox_scope: SandboxScope,
        memory_scope: AgentMemoryScope,
    ) -> Self {
        Self {
            session_id,
            memory: AgentMemory::new(AGENT_INTERNAL_CONTEXT_WINDOW_CAP_TOKENS),
            sandbox: None,
            sandbox_scope,
            memory_scope,
            started_at: None,
            current_task_id: None,
            status: AgentStatus::Idle,
            cancellation_token: CancellationToken::new(),
            last_task: None,
            loaded_skills: HashSet::new(),
            skill_token_count: 0,
            runtime_context_inbox: RuntimeContextInbox::new(),
            pending_ssh_replays: Vec::new(),
            pending_user_input: None,
            memory_checkpoint: None,
            checkpoint_state: Arc::new(AsyncMutex::new(MemoryCheckpointState::default())),
            checkpoint_persist_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    /// Override the memory scope used for archive and durable memory persistence.
    pub fn set_memory_scope(&mut self, memory_scope: AgentMemoryScope) {
        self.memory_scope = memory_scope;
    }

    /// Access the stable memory scope for this session.
    #[must_use]
    pub fn memory_scope(&self) -> &AgentMemoryScope {
        &self.memory_scope
    }

    /// Build the compaction/archive scope for this session.
    #[must_use]
    pub fn compaction_scope(&self) -> CompactionScope {
        self.memory_scope.compaction_scope()
    }

    /// Install a transport-provided checkpoint sink for memory snapshots.
    pub fn set_memory_checkpoint(&mut self, checkpoint: Arc<dyn AgentMemoryCheckpoint>) {
        self.memory_checkpoint = Some(checkpoint);
    }

    async fn prepare_forced_memory_checkpoint(&self) -> Result<Option<QueuedMemoryCheckpoint>> {
        let hash = memory_checkpoint_hash(&self.memory)?;
        let mut state = self.checkpoint_state.lock().await;

        if state.last_persisted_hash == Some(hash) {
            state.pending = None;
            return Ok(None);
        }

        state.next_generation = state.next_generation.saturating_add(1);
        let generation = state.next_generation;
        state.pending = None;

        Ok(Some(QueuedMemoryCheckpoint {
            memory: self.memory.clone(),
            hash,
            generation,
        }))
    }

    /// Persist the current memory snapshot when a checkpoint sink is configured.
    pub async fn persist_memory_checkpoint(&self) -> Result<()> {
        let Some(checkpoint) = &self.memory_checkpoint else {
            return Ok(());
        };

        let Some(queued) = self.prepare_forced_memory_checkpoint().await? else {
            return Ok(());
        };

        persist_queued_memory_checkpoint(
            Arc::clone(checkpoint),
            Arc::clone(&self.checkpoint_state),
            Arc::clone(&self.checkpoint_persist_lock),
            queued,
            true,
        )
        .await
    }

    /// Persist memory checkpoint in the background (fire-and-forget).
    ///
    /// This spawns a background task to persist the checkpoint without blocking
    /// the caller. Useful for non-critical persistence where latency matters more
    /// than durability guarantees.
    pub fn persist_memory_checkpoint_background(&self) {
        let Some(checkpoint) = self.memory_checkpoint.clone() else {
            return;
        };

        let memory = self.memory.clone();
        let checkpoint_state = Arc::clone(&self.checkpoint_state);
        let persist_lock = Arc::clone(&self.checkpoint_persist_lock);
        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let should_spawn_worker = match async {
                let hash = memory_checkpoint_hash(&memory)?;
                let mut state = checkpoint_state.lock().await;

                if state.last_persisted_hash == Some(hash) {
                    state.pending = None;
                    return Ok(false);
                }

                if state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.hash == hash)
                {
                    return Ok(false);
                }

                state.next_generation = state.next_generation.saturating_add(1);
                let generation = state.next_generation;
                state.pending = Some(QueuedMemoryCheckpoint {
                    memory,
                    hash,
                    generation,
                });

                if state.background_task_active {
                    return Ok(false);
                }

                state.background_task_active = true;
                Ok::<bool, anyhow::Error>(true)
            }
            .await
            {
                Ok(should_spawn_worker) => should_spawn_worker,
                Err(error) => {
                    warn!(
                        error = %error,
                        elapsed_ms = start.elapsed().as_millis(),
                        "Failed to queue memory checkpoint (background)"
                    );
                    return;
                }
            };

            if should_spawn_worker {
                tokio::spawn(run_background_checkpoint_loop(
                    checkpoint,
                    checkpoint_state,
                    persist_lock,
                ));
            }

            debug!(
                elapsed_ms = start.elapsed().as_millis(),
                spawned_worker = should_spawn_worker,
                "Memory checkpoint queued (background)"
            );
        });
    }

    /// Clone the runtime context inbox handle for concurrent transport writes.
    #[must_use]
    pub fn runtime_context_inbox(&self) -> RuntimeContextInbox {
        self.runtime_context_inbox.clone()
    }

    /// Update the effective hot-context budget for this session.
    pub fn set_context_window_tokens(&mut self, max_tokens: usize) {
        self.memory.set_max_tokens(max_tokens);
    }

    /// Queue additional runtime context for the next safe iteration boundary.
    pub fn push_runtime_context(&self, injection: RuntimeContextInjection) {
        self.runtime_context_inbox.push(injection);
    }

    /// Drain pending runtime context payloads in FIFO order.
    #[must_use]
    pub fn drain_runtime_context(&self) -> Vec<RuntimeContextInjection> {
        self.runtime_context_inbox.drain()
    }

    /// Returns true when new runtime context is waiting to be applied.
    #[must_use]
    pub fn has_pending_runtime_context(&self) -> bool {
        self.runtime_context_inbox.has_pending()
    }

    /// Store or replace a pending SSH replay payload.
    pub fn store_pending_ssh_replay(&mut self, replay: PendingSshReplay) {
        self.pending_ssh_replays
            .retain(|entry| entry.request_id != replay.request_id);
        self.pending_ssh_replays.push(replay);
    }

    /// Return a pending SSH replay payload by request id.
    #[must_use]
    pub fn pending_ssh_replay(&self, request_id: &str) -> Option<PendingSshReplay> {
        self.pending_ssh_replays
            .iter()
            .find(|entry| entry.request_id == request_id)
            .cloned()
    }

    /// Remove and return a pending SSH replay payload by request id.
    pub fn take_pending_ssh_replay(&mut self, request_id: &str) -> Option<PendingSshReplay> {
        let index = self
            .pending_ssh_replays
            .iter()
            .position(|entry| entry.request_id == request_id)?;
        Some(self.pending_ssh_replays.remove(index))
    }

    /// Store or replace the pending user input request.
    pub fn set_pending_user_input(&mut self, request: PendingUserInput) {
        self.pending_user_input = Some(request);
    }

    /// Clear the pending user input request.
    pub fn clear_pending_user_input(&mut self) {
        self.pending_user_input = None;
    }

    /// Return the current pending user input request, if any.
    #[must_use]
    pub fn pending_user_input(&self) -> Option<&PendingUserInput> {
        self.pending_user_input.as_ref()
    }

    /// Stable sandbox scope for this session.
    #[must_use]
    pub fn sandbox_scope(&self) -> &SandboxScope {
        &self.sandbox_scope
    }

    /// Renew the cancellation token before a new task
    /// CRITICAL: Prevents old cancellation signals from affecting new tasks
    pub fn renew_cancellation_token(&mut self) {
        self.cancellation_token = CancellationToken::new();
    }

    /// Start a new task, resetting the timer and generating a task ID
    pub fn start_task(&mut self) {
        self.started_at = Some(Instant::now());
        self.current_task_id = Some(uuid::Uuid::new_v4().to_string());
        self.pending_user_input = None;
        self.status = AgentStatus::Processing {
            step: "Initializing...".to_string(),
            progress_percent: 0,
        };
    }

    /// Get elapsed time in seconds since task start
    #[must_use]
    pub fn elapsed_secs(&self) -> u64 {
        self.started_at.map_or(0, |start| start.elapsed().as_secs())
    }

    /// Update the progress status
    pub fn update_progress(&mut self, step: String, progress_percent: u8) {
        self.status = AgentStatus::Processing {
            step,
            progress_percent: progress_percent.min(100),
        };
    }

    /// Mark the task as completed
    pub fn complete(&mut self) {
        self.status = AgentStatus::Completed;
        self.started_at = None;
    }

    /// Mark the task as timed out
    pub fn timeout(&mut self) {
        self.status = AgentStatus::TimedOut;
        self.started_at = None;
    }

    /// Mark the task as failed with an error
    pub fn fail(&mut self, error: String) {
        self.status = AgentStatus::Error(error);
        self.started_at = None;
    }

    /// Reset the session (clear memory, todos, reset status)
    /// Note: Sandbox is persistent and not destroyed here
    pub fn reset(&mut self) {
        self.memory.clear();
        self.status = AgentStatus::Idle;
        self.started_at = None;
        self.current_task_id = None;
        self.last_task = None;
        self.loaded_skills.clear();
        self.skill_token_count = 0;
        let _ = self.runtime_context_inbox.drain();
        self.pending_ssh_replays.clear();
        self.pending_user_input = None;
        if let Ok(mut state) = self.checkpoint_state.try_lock() {
            *state = MemoryCheckpointState::default();
        }

        // Sandbox is persistent, do NOT destroy it here
        // if let Some(mut sandbox) = self.sandbox.take() { ... }
    }

    /// Store the last task text for retries.
    pub fn remember_task(&mut self, task: &str) {
        self.last_task = Some(task.to_string());
    }

    /// Reset loaded skills based on the active system prompt.
    pub fn set_loaded_skills(&mut self, skills: &[crate::agent::skills::SkillContext]) {
        self.loaded_skills = skills.iter().map(|skill| skill.name.clone()).collect();
        self.skill_token_count = skills.iter().map(|skill| skill.token_count).sum();
    }

    /// Register a dynamically loaded skill, returns true if it was new.
    pub fn register_loaded_skill(&mut self, name: &str, token_count: usize) -> bool {
        if self.loaded_skills.insert(name.to_string()) {
            self.skill_token_count = self.skill_token_count.saturating_add(token_count);
            return true;
        }

        false
    }

    /// Check if a skill is already loaded.
    #[must_use]
    pub fn is_skill_loaded(&self, name: &str) -> bool {
        self.loaded_skills.contains(name)
    }

    /// Get total tokens used by loaded skills.
    #[must_use]
    pub const fn skill_token_count(&self) -> usize {
        self.skill_token_count
    }

    /// Clear only the todos list (keeps memory intact)
    pub fn clear_todos(&mut self) {
        self.memory.todos.clear();
    }

    /// Check if the session is currently processing a task
    #[must_use]
    pub const fn is_processing(&self) -> bool {
        matches!(self.status, AgentStatus::Processing { .. })
    }

    /// Check if sandbox is available
    #[must_use]
    pub fn has_sandbox(&self) -> bool {
        self.sandbox
            .as_ref()
            .is_some_and(SandboxManager::is_running)
    }

    /// Ensure sandbox is running, creating it if necessary
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox creation fails.
    pub async fn ensure_sandbox(&mut self) -> Result<&mut SandboxManager> {
        let needs_new = self.sandbox.as_ref().is_none_or(|s| !s.is_running());

        if needs_new {
            debug!(session_id = %self.session_id, "Creating new sandbox");
            let mut sandbox = SandboxManager::new(self.sandbox_scope.clone()).await?;
            sandbox.create_sandbox().await?;
            self.sandbox = Some(sandbox);
            info!(session_id = %self.session_id, "Sandbox created for session");
        }

        self.sandbox
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Sandbox not initialized"))
    }

    /// Force sandbox recreation, wiping previous container state.
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox manager initialization or recreation fails.
    pub async fn force_recreate_sandbox(&mut self) -> Result<()> {
        if self.sandbox.is_none() {
            self.sandbox = Some(SandboxManager::new(self.sandbox_scope.clone()).await?);
        }

        if let Some(sandbox) = self.sandbox.as_mut() {
            sandbox.recreate().await?;
            info!(session_id = %self.session_id, "Sandbox force recreated for session");
            return Ok(());
        }

        Err(anyhow::anyhow!("Sandbox not initialized"))
    }

    /// Get sandbox reference if running
    #[must_use]
    pub fn sandbox(&self) -> Option<&SandboxManager> {
        self.sandbox.as_ref().filter(|s| s.is_running())
    }

    /// Get mutable sandbox reference if running
    pub fn sandbox_mut(&mut self) -> Option<&mut SandboxManager> {
        self.sandbox.as_mut().filter(|s| s.is_running())
    }

    /// Destroy sandbox if running
    ///
    /// # Errors
    ///
    /// Returns an error if sandbox destruction fails.
    pub async fn destroy_sandbox(&mut self) -> Result<()> {
        if let Some(mut sandbox) = self.sandbox.take() {
            sandbox.destroy().await?;
            info!(session_id = %self.session_id, "Sandbox destroyed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Allow clone_on_ref_ptr in tests due to trait object coercion requirements
    #![allow(clippy::clone_on_ref_ptr)]

    use super::{
        AgentMemoryCheckpoint, AgentMemoryScope, AgentSession, PendingSshReplay, PendingUserInput,
        UserInputKind,
    };
    use crate::agent::memory::AgentMessage;
    use crate::llm::InvocationId;
    use crate::sandbox::SandboxScope;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingCheckpoint {
        snapshots: Mutex<Vec<crate::agent::AgentMemory>>,
    }

    impl RecordingCheckpoint {
        fn persisted_count(&self) -> usize {
            self.snapshots
                .lock()
                .expect("snapshots mutex poisoned")
                .len()
        }

        fn latest_message_count(&self) -> usize {
            self.snapshots
                .lock()
                .expect("snapshots mutex poisoned")
                .last()
                .map_or(0, |memory| memory.get_messages().len())
        }
    }

    #[async_trait]
    impl AgentMemoryCheckpoint for RecordingCheckpoint {
        async fn persist(&self, memory: &crate::agent::AgentMemory) -> Result<()> {
            self.snapshots
                .lock()
                .expect("snapshots mutex poisoned")
                .push(memory.clone());
            Ok(())
        }
    }

    #[test]
    fn reset_clears_pending_ssh_replays() {
        let mut session = AgentSession::new(42_i64.into());
        session.store_pending_ssh_replay(PendingSshReplay {
            request_id: "req-1".to_string(),
            invocation_id: InvocationId::from("call-1"),
            tool_name: "ssh_sudo_exec".to_string(),
            arguments: r#"{"command":"journalctl"}"#.to_string(),
        });

        assert!(session.pending_ssh_replay("req-1").is_some());

        session.reset();

        assert!(session.pending_ssh_replay("req-1").is_none());
    }

    #[test]
    fn start_task_clears_pending_user_input() {
        let mut session = AgentSession::new(42_i64.into());
        session.set_pending_user_input(PendingUserInput {
            kind: UserInputKind::UrlOrFile,
            prompt: "Send the APK link or file".to_string(),
        });

        session.start_task();

        assert!(session.pending_user_input().is_none());
    }

    #[test]
    fn synthetic_memory_scope_defaults_to_session_identity() {
        let session = AgentSession::new(42_i64.into());

        assert_eq!(session.memory_scope().user_id, 42);
        assert_eq!(session.memory_scope().context_key, "session:42");
        assert_eq!(session.memory_scope().flow_id, "agent-mode");

        let compaction_scope = session.compaction_scope();
        assert_eq!(compaction_scope.context_key, "session:42");
        assert_eq!(compaction_scope.flow_id, "agent-mode");
    }

    #[test]
    fn explicit_memory_scope_overrides_compaction_scope() {
        let scope = AgentMemoryScope::new(7, "topic-a", "flow-b");
        let session =
            AgentSession::new_with_scopes(42_i64.into(), SandboxScope::from(42_i64), scope.clone());

        assert_eq!(session.memory_scope(), &scope);

        let compaction_scope = session.compaction_scope();
        assert_eq!(compaction_scope.context_key, "topic-a");
        assert_eq!(compaction_scope.flow_id, "flow-b");
    }

    #[tokio::test]
    async fn checkpoint_skips_identical_forced_persists() {
        let checkpoint = Arc::new(RecordingCheckpoint::default());
        let mut session = AgentSession::new(42_i64.into());
        session.set_memory_checkpoint(checkpoint.clone());
        session.memory.add_message(AgentMessage::user("hello"));

        session
            .persist_memory_checkpoint()
            .await
            .expect("first persist should succeed");
        session
            .persist_memory_checkpoint()
            .await
            .expect("second persist should succeed");

        assert_eq!(checkpoint.persisted_count(), 1);
    }

    #[tokio::test]
    async fn background_checkpoint_coalesces_to_latest_snapshot() {
        let checkpoint = Arc::new(RecordingCheckpoint::default());
        let mut session = AgentSession::new(42_i64.into());
        session.set_memory_checkpoint(checkpoint.clone());

        session.memory.add_message(AgentMessage::user("first"));
        session.persist_memory_checkpoint_background();

        session
            .memory
            .add_message(AgentMessage::assistant("second"));
        session.persist_memory_checkpoint_background();

        tokio::time::sleep(super::checkpoint_debounce_duration() * 4).await;

        assert_eq!(checkpoint.persisted_count(), 1);
        assert_eq!(checkpoint.latest_message_count(), 2);
    }
}
