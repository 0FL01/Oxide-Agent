//! `AgentTransport` implementation for the web transport.
//!
//! Collects `AgentEvent`s into an in-memory buffer and exposes progress
//! via the HTTP API. Unlike Telegram transport, this does not send messages
//! to any chat — it only records the event timeline for later inspection.

use oxide_agent_core::agent::progress::{AgentEvent, ProgressState};
use oxide_agent_runtime::{AgentTransport, DeliveryMode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

/// Returns the snake_case variant name of an AgentEvent.
fn event_variant_name(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Thinking { .. } => "thinking".to_string(),
        AgentEvent::TokenSnapshotUpdated { .. } => "token_snapshot_updated".to_string(),
        AgentEvent::ToolCall { name, .. } => format!("tool_call:{name}"),
        AgentEvent::ToolResult { name, .. } => format!("tool_result:{name}"),
        AgentEvent::WaitingForApproval { tool_name, .. } => {
            format!("waiting_for_approval:{tool_name}")
        }
        AgentEvent::Continuation { .. } => "continuation".to_string(),
        AgentEvent::TodosUpdated { .. } => "todos_updated".to_string(),
        AgentEvent::FileToSend { file_name, .. } => format!("file_to_send:{file_name}"),
        AgentEvent::FileToSendWithConfirmation { .. } => {
            "file_to_send_with_confirmation".to_string()
        }
        AgentEvent::Finished => "finished".to_string(),
        AgentEvent::Cancelling { .. } => "cancelling".to_string(),
        AgentEvent::Cancelled => "cancelled".to_string(),
        AgentEvent::Error(_) => "error".to_string(),
        AgentEvent::Reasoning { .. } => "reasoning".to_string(),
        AgentEvent::LoopDetected { .. } => "loop_detected".to_string(),
        AgentEvent::Narrative { .. } => "narrative".to_string(),
        AgentEvent::CompactionStarted { .. } => "compaction_started".to_string(),
        AgentEvent::PruningApplied { .. } => "pruning_applied".to_string(),
        AgentEvent::CompactionCompleted { .. } => "compaction_completed".to_string(),
        AgentEvent::CompactionFailed { .. } => "compaction_failed".to_string(),
        AgentEvent::RepeatedCompactionWarning { .. } => "repeated_compaction_warning".to_string(),
        AgentEvent::RateLimitRetrying { .. } => "rate_limit_retrying".to_string(),
    }
}

/// Events collected for a single task execution.
#[derive(Debug, Clone)]
pub struct TaskEventLog {
    /// Events in order of arrival.
    pub events: Arc<RwLock<Vec<TaskEventEntry>>>,
    /// Broadcasts each new event to SSE subscribers.
    broadcast_tx: broadcast::Sender<TaskEventEntry>,
    /// Flag set when the task is done.
    done: Arc<RwLock<bool>>,
}

/// A simplified event entry that stores only the event name.
/// Full event data is available in `ProgressState` via the `/progress` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskEventEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_name: String,
}

impl TaskEventLog {
    /// Create a new empty event log.
    #[must_use]
    pub fn new() -> Self {
        let (broadcast_tx, _broadcast_rx) = broadcast::channel(100);
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            broadcast_tx,
            done: Arc::new(RwLock::new(false)),
        }
    }

    /// Record an event with the current timestamp and broadcast it to SSE subscribers.
    pub async fn push(&self, event: AgentEvent) {
        let event_name = event_variant_name(&event);
        let entry = TaskEventEntry {
            timestamp: chrono::Utc::now(),
            event_name,
        };
        let broadcast_entry = entry.clone();
        self.events.write().await.push(entry);
        // Broadcast to SSE subscribers. Ignore error if no active receivers.
        let _ = self.broadcast_tx.send(broadcast_entry);
    }

    /// Subscribe to new events as they are pushed. The returned receiver will
    /// receive events published after this call (not the current snapshot).
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TaskEventEntry> {
        self.broadcast_tx.subscribe()
    }

    /// Mark the event log as done — SSE subscribers will stop receiving after this.
    pub async fn close(&self) {
        {
            let mut d = self.done.write().await;
            *d = true;
        }
        // Send a sentinel event to wake up any waiting receivers.
        let sentinel = TaskEventEntry {
            timestamp: chrono::Utc::now(),
            event_name: "stream_closed".to_string(),
        };
        let _ = self.broadcast_tx.send(sentinel);
    }

    /// Returns `true` if `close()` has been called.
    pub async fn is_closed(&self) -> bool {
        *self.done.read().await
    }

    /// Drain all events and return them.
    pub async fn drain(&self) -> Vec<TaskEventEntry> {
        let mut events = self.events.write().await;
        std::mem::take(&mut *events)
    }

    /// Take and return the current event log, replacing it with an empty one.
    pub async fn take(&self) -> Vec<TaskEventEntry> {
        self.drain().await
    }

    /// Returns a snapshot of the current event log without consuming it.
    pub async fn snapshot(&self) -> Vec<TaskEventEntry> {
        self.events.read().await.clone()
    }
}

impl Default for TaskEventLog {
    fn default() -> Self {
        Self::new()
    }
}

/// Transport that records events in memory.
///
/// Implements `AgentTransport` from the runtime crate. Does not send
/// messages anywhere — only stores events for later retrieval via HTTP.
#[derive(Clone)]
pub struct WebAgentTransport {
    event_log: TaskEventLog,
}

impl WebAgentTransport {
    /// Create a new web transport with a fresh event log.
    #[must_use]
    pub fn new(event_log: TaskEventLog) -> Self {
        Self { event_log }
    }
}

#[async_trait::async_trait]
impl AgentTransport for WebAgentTransport {
    async fn update_progress(&self, _state: &ProgressState) -> Result<(), anyhow::Error> {
        // ProgressState is stored by the task runner separately;
        // we only need to record agent events here.
        Ok(())
    }

    async fn deliver_file(
        &self,
        _mode: DeliveryMode,
        file_name: &str,
        content: &[u8],
    ) -> Result<(), anyhow::Error> {
        // Record file delivery as a synthetic event so tests can observe it.
        let event = AgentEvent::FileToSend {
            file_name: file_name.to_string(),
            content: content.to_vec(),
        };
        self.event_log.push(event).await;
        Ok(())
    }

    async fn notify_loop_detected(
        &self,
        loop_type: oxide_agent_core::agent::loop_detection::LoopType,
        iteration: usize,
    ) -> Result<(), anyhow::Error> {
        let event = AgentEvent::LoopDetected {
            loop_type,
            iteration,
        };
        self.event_log.push(event).await;
        Ok(())
    }
}

/// Start collecting events from a `Receiver<AgentEvent>` and drive the
/// event log.
///
/// Returns the final `ProgressState` once the channel is closed.
pub async fn collect_events(
    event_log: TaskEventLog,
    mut rx: mpsc::Receiver<AgentEvent>,
) -> ProgressState {
    use oxide_agent_core::agent::progress::ProgressState;

    let mut state = ProgressState::new(100); // max_iterations, can be overridden

    while let Some(event) = rx.recv().await {
        // FileToSend is already recorded by the transport; skip it here.
        if !matches!(event, AgentEvent::FileToSend { .. }) {
            event_log.push(event).await;
        } else {
            state.update(event);
        }
    }

    // Close the event log — signals SSE subscribers to stop.
    event_log.close().await;

    state
}
