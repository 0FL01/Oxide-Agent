//! `AgentTransport` implementation for the web transport.
//!
//! Collects `AgentEvent`s into an in-memory buffer and exposes progress
//! via the HTTP API. Unlike Telegram transport, this does not send messages
//! to any chat — it only records the event timeline for later inspection.

use oxide_agent_core::agent::progress::{AgentEvent, FileDeliveryKind, ProgressState};
use oxide_agent_runtime::{AgentTransport, DeliveryMode};
use oxide_agent_web_contracts::{PersistedTaskEvent, TaskEventKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
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
        AgentEvent::RuntimeCompactionStarted { .. } => "compaction_started".to_string(),
        AgentEvent::RuntimeCompactionCompleted { .. } => "compaction_completed".to_string(),
        AgentEvent::RuntimeCompactionFailed { .. } => "compaction_failed".to_string(),
        AgentEvent::RuntimeCompactionSkipped { .. } => "compaction_skipped".to_string(),
        AgentEvent::RepeatedCompactionWarning { .. } => "repeated_compaction_warning".to_string(),
        AgentEvent::HistoryRepairApplied { .. } => "history_repair_applied".to_string(),
        AgentEvent::RateLimitRetrying { .. } => "rate_limit_retrying".to_string(),
        AgentEvent::LlmRetrying { .. } => "llm_retrying".to_string(),
        AgentEvent::ProviderFailoverActivated { .. } => "provider_failover_activated".to_string(),
        AgentEvent::Milestone { name, .. } => format!("milestone:{name}"),
    }
}

const WEB_EVENT_SCHEMA_VERSION: u32 = 1;
const EVENT_SUMMARY_MAX_CHARS: usize = 160;
const EVENT_PREVIEW_MAX_CHARS: usize = 4_000;
const REDACTED_EVENT_PAYLOAD: &str = "[redacted sensitive event payload]";

#[derive(Debug, Clone)]
pub struct BrowserEventScope {
    pub user_id: i64,
    pub session_id: String,
    pub task_id: String,
}

impl BrowserEventScope {
    #[must_use]
    pub fn new(user_id: i64, session_id: String, task_id: String) -> Self {
        Self {
            user_id,
            session_id,
            task_id,
        }
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
    pub async fn push(&self, event: &AgentEvent) {
        let event_name = event_variant_name(event);
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
        kind: FileDeliveryKind,
        file_name: &str,
        content: &[u8],
    ) -> Result<(), anyhow::Error> {
        // Record file delivery as a synthetic event so tests can observe it.
        let event = AgentEvent::FileToSend {
            kind,
            file_name: file_name.to_string(),
            content: content.to_vec(),
        };
        self.event_log.push(&event).await;
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
        self.event_log.push(&event).await;
        Ok(())
    }
}

/// Timestamps recorded during event collection for latency milestones.
#[derive(Debug, Clone, Default)]
pub struct MilestoneTimestamps {
    /// When the first `AgentEvent::Thinking` was received.
    pub first_thinking_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the first `AgentEvent::Reasoning` was received.
    pub first_reasoning_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the final event was received (agent finished).
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Map of named milestone timestamps received via AgentEvent::Milestone
    pub named_milestones: std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>,
}

/// Timing capture for an individual tool call observed during event collection.
#[derive(Debug, Clone)]
pub struct CollectedToolCallTiming {
    pub name: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Final event-collection output used to populate web transport APIs.
#[derive(Debug, Clone)]
pub struct EventCollectionResult {
    pub state: ProgressState,
    pub timestamps: MilestoneTimestamps,
    pub tool_calls: Vec<CollectedToolCallTiming>,
    pub persisted_events: Vec<PersistedTaskEvent>,
}

/// Start collecting events from a `Receiver<AgentEvent>` and drive the
/// event log.
///
/// Returns the final `ProgressState` along with milestone timestamps
/// once the channel is closed.
pub async fn collect_events(
    event_log: TaskEventLog,
    mut rx: mpsc::Receiver<AgentEvent>,
    browser_event_scope: Option<BrowserEventScope>,
    live_event_tx: Option<mpsc::UnboundedSender<PersistedTaskEvent>>,
    live_progress_tx: Option<mpsc::UnboundedSender<ProgressState>>,
) -> EventCollectionResult {
    use oxide_agent_core::agent::progress::ProgressState;

    let mut state = ProgressState::new(100); // max_iterations, can be overridden
    let mut timestamps = MilestoneTimestamps::default();
    let mut tool_calls = Vec::new();
    let mut active_tool_calls: HashMap<String, VecDeque<usize>> = HashMap::new();
    let mut persisted_events = Vec::new();
    let mut next_seq = 1;

    while let Some(event) = rx.recv().await {
        let event_received_at = chrono::Utc::now();
        // Classify event type once to avoid borrow-after-move.
        let is_thinking = matches!(&event, AgentEvent::Thinking { .. });
        let is_reasoning = matches!(&event, AgentEvent::Reasoning { .. });
        let is_file_to_send = matches!(&event, AgentEvent::FileToSend { .. });
        let is_terminal = matches!(
            &event,
            AgentEvent::Finished | AgentEvent::Cancelled | AgentEvent::Error(_)
        );
        let is_milestone = matches!(&event, AgentEvent::Milestone { .. });

        // Track first Thinking/Reasoning to derive llm_call_started_ms on collector-side.
        if timestamps.first_thinking_at.is_none() && is_thinking {
            timestamps.first_thinking_at = Some(chrono::Utc::now());
        }
        if timestamps.first_reasoning_at.is_none() && is_reasoning {
            timestamps.first_reasoning_at = Some(chrono::Utc::now());
        }

        // Track named milestones from the agent core.
        if let AgentEvent::Milestone { name, timestamp_ms } = &event {
            if let Some(ts) = chrono::DateTime::from_timestamp_millis(*timestamp_ms) {
                timestamps.named_milestones.insert(name.clone(), ts);
            }
        }

        match &event {
            AgentEvent::ToolCall { name, .. } => {
                let idx = tool_calls.len();
                tool_calls.push(CollectedToolCallTiming {
                    name: name.clone(),
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                });
                active_tool_calls
                    .entry(name.clone())
                    .or_default()
                    .push_back(idx);
            }
            AgentEvent::ToolResult { name, .. } => {
                if let Some(indices) = active_tool_calls.get_mut(name) {
                    if let Some(idx) = indices.pop_front() {
                        if let Some(tool_call) = tool_calls.get_mut(idx) {
                            tool_call.finished_at = Some(chrono::Utc::now());
                        }
                    }
                    if indices.is_empty() {
                        active_tool_calls.remove(name);
                    }
                }
            }
            _ => {}
        }

        // Milestones are tracked separately and FileToSend is already recorded by the
        // transport itself, but both still should update progress state for consistency.
        if !is_milestone && !is_file_to_send {
            event_log.push(&event).await;
        }

        if let Some(scope) = browser_event_scope.as_ref() {
            let persisted_event =
                persisted_event_from_agent_event(scope, next_seq, event_received_at, &event);
            if let Some(live_event_tx) = live_event_tx.as_ref() {
                let _ = live_event_tx.send(persisted_event.clone());
            }
            persisted_events.push(persisted_event);
            next_seq += 1;
        }

        state.update(event);
        if let Some(live_progress_tx) = live_progress_tx.as_ref() {
            let _ = live_progress_tx.send(state.clone());
        }

        // Track finished_at.
        if is_terminal {
            timestamps.finished_at = Some(chrono::Utc::now());
        }
    }

    // Close the event log — signals SSE subscribers to stop.
    event_log.close().await;

    EventCollectionResult {
        state,
        timestamps,
        tool_calls,
        persisted_events,
    }
}

fn persisted_event_from_agent_event(
    scope: &BrowserEventScope,
    seq: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    event: &AgentEvent,
) -> PersistedTaskEvent {
    let (kind, summary, payload, redacted, truncated) = browser_event_parts(event);
    PersistedTaskEvent {
        schema_version: WEB_EVENT_SCHEMA_VERSION,
        task_id: scope.task_id.clone(),
        session_id: scope.session_id.clone(),
        user_id: scope.user_id,
        seq,
        created_at,
        kind,
        summary,
        payload,
        redacted,
        truncated,
    }
}

fn browser_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::Thinking { .. } | AgentEvent::TokenSnapshotUpdated { .. } => {
            token_event_parts(event)
        }
        AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. } => tool_event_parts(event),
        AgentEvent::WaitingForApproval {
            tool_name,
            target_name,
            summary,
        } => {
            let (summary_preview, truncated) = truncate_text(summary, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::TaskStatus,
                format!("waiting_for_approval:{tool_name}"),
                json!({
                    "tool_name": tool_name,
                    "target_name": target_name,
                    "summary": summary_preview,
                }),
                false,
                truncated,
            )
        }
        AgentEvent::Continuation { .. }
        | AgentEvent::Finished
        | AgentEvent::Cancelling { .. }
        | AgentEvent::Cancelled
        | AgentEvent::Error(_)
        | AgentEvent::Reasoning { .. }
        | AgentEvent::LoopDetected { .. }
        | AgentEvent::Milestone { .. } => lifecycle_event_parts(event),
        AgentEvent::TodosUpdated { todos } => (
            TaskEventKind::TodosUpdated,
            "Todos updated".to_string(),
            json!({ "todos": todos }),
            false,
            false,
        ),
        AgentEvent::FileToSend { .. } | AgentEvent::FileToSendWithConfirmation { .. } => {
            file_event_parts(event)
        }
        AgentEvent::RuntimeCompactionStarted { .. }
        | AgentEvent::RuntimeCompactionCompleted { .. }
        | AgentEvent::RuntimeCompactionFailed { .. }
        | AgentEvent::RuntimeCompactionSkipped { .. } => compaction_event_parts(event),
        AgentEvent::RepeatedCompactionWarning { .. }
        | AgentEvent::HistoryRepairApplied { .. }
        | AgentEvent::RateLimitRetrying { .. }
        | AgentEvent::LlmRetrying { .. }
        | AgentEvent::ProviderFailoverActivated { .. } => maintenance_event_parts(event),
    }
}

fn token_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::Thinking { snapshot } => (
            TaskEventKind::Thinking,
            "Thinking".to_string(),
            json!({ "token_snapshot": snapshot }),
            false,
            false,
        ),
        AgentEvent::TokenSnapshotUpdated { snapshot } => (
            TaskEventKind::TokenSnapshotUpdated,
            "Token snapshot updated".to_string(),
            json!({ "token_snapshot": snapshot }),
            false,
            false,
        ),
        _ => unreachable!("token_event_parts called with non-token event"),
    }
}

fn tool_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::ToolCall {
            name,
            input,
            command_preview,
        } => {
            let (input_preview, input_truncated, input_redacted) =
                preview_event_text(input, EVENT_PREVIEW_MAX_CHARS);
            let (command_preview, command_truncated, command_redacted) = command_preview
                .as_ref()
                .map(|preview| preview_event_text(preview, EVENT_PREVIEW_MAX_CHARS))
                .map_or((None, false, false), |(preview, truncated, redacted)| {
                    (Some(preview), truncated, redacted)
                });
            (
                TaskEventKind::ToolCall,
                truncate_summary(name),
                json!({
                    "name": name,
                    "input_preview": input_preview,
                    "command_preview": command_preview,
                }),
                input_redacted || command_redacted,
                input_truncated || command_truncated,
            )
        }
        AgentEvent::ToolResult {
            name,
            output,
            success,
        } => {
            let (output_preview, truncated, redacted) =
                preview_event_text(output, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::ToolResult,
                truncate_summary(name),
                json!({
                    "name": name,
                    "success": success,
                    "output_preview": output_preview,
                }),
                redacted,
                truncated,
            )
        }
        _ => unreachable!("tool_event_parts called with non-tool event"),
    }
}

fn file_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::FileToSend {
            kind,
            file_name,
            content,
        } => (
            TaskEventKind::FileToSend,
            truncate_summary(file_name),
            json!({
                "delivery_kind": kind,
                "file_name": file_name,
                "byte_len": content.len(),
            }),
            true,
            false,
        ),
        AgentEvent::FileToSendWithConfirmation {
            kind,
            file_name,
            content,
            source_path,
            confirmation_tx: _,
        } => (
            TaskEventKind::FileToSend,
            truncate_summary(file_name),
            json!({
                "delivery_kind": kind,
                "file_name": file_name,
                "byte_len": content.len(),
                "source_path": source_path,
            }),
            true,
            false,
        ),
        _ => unreachable!("file_event_parts called with non-file event"),
    }
}

fn lifecycle_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::Continuation { reason, count } => {
            let (reason_preview, truncated) = truncate_text(reason, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::Continuation,
                "Continuation".to_string(),
                json!({ "reason": reason_preview, "count": count }),
                false,
                truncated,
            )
        }
        AgentEvent::Finished => (
            TaskEventKind::Finished,
            "Finished".to_string(),
            json!({}),
            false,
            false,
        ),
        AgentEvent::Cancelling { tool_name } => (
            TaskEventKind::Cancelling,
            truncate_summary(tool_name),
            json!({ "tool_name": tool_name }),
            false,
            false,
        ),
        AgentEvent::Cancelled => (
            TaskEventKind::Cancelled,
            "Cancelled".to_string(),
            json!({}),
            false,
            false,
        ),
        AgentEvent::Error(error) => {
            let (error_preview, truncated) = truncate_text(error, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::Error,
                "Error".to_string(),
                json!({ "message": error_preview }),
                false,
                truncated,
            )
        }
        AgentEvent::Reasoning { summary } => {
            let (summary_preview, truncated) = truncate_text(summary, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::Reasoning,
                "Reasoning".to_string(),
                json!({ "summary": summary_preview }),
                false,
                truncated,
            )
        }
        AgentEvent::LoopDetected {
            loop_type,
            iteration,
        } => (
            TaskEventKind::LoopDetected,
            "Loop detected".to_string(),
            json!({ "loop_type": loop_type, "iteration": iteration }),
            false,
            false,
        ),
        AgentEvent::Milestone { name, timestamp_ms } => (
            TaskEventKind::Milestone,
            truncate_summary(name),
            json!({ "name": name, "timestamp_ms": timestamp_ms }),
            false,
            false,
        ),
        _ => unreachable!("lifecycle_event_parts called with wrong event"),
    }
}

fn compaction_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::RuntimeCompactionStarted {
            reason,
            phase,
            backend,
            provider,
            route,
            token_before,
            history_items_before,
        } => (
            TaskEventKind::RuntimeCompactionStarted,
            "Compaction started".to_string(),
            json!({
                "reason": reason,
                "phase": phase,
                "backend": backend,
                "provider": provider,
                "route": route,
                "token_before": token_before,
                "history_items_before": history_items_before,
            }),
            false,
            false,
        ),
        AgentEvent::RuntimeCompactionCompleted { .. } => compaction_completed_parts(event),
        AgentEvent::RuntimeCompactionFailed {
            reason,
            phase,
            backend,
            provider,
            route,
            error,
        } => {
            let (error_preview, truncated) = truncate_text(error, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::RuntimeCompactionFailed,
                "Compaction failed".to_string(),
                json!({
                    "reason": reason,
                    "phase": phase,
                    "backend": backend,
                    "provider": provider,
                    "route": route,
                    "error": error_preview,
                }),
                false,
                truncated,
            )
        }
        AgentEvent::RuntimeCompactionSkipped {
            reason,
            phase,
            skipped_reason,
        } => {
            let (skipped_reason, truncated) =
                truncate_text(skipped_reason, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::RuntimeCompactionSkipped,
                "Compaction skipped".to_string(),
                json!({
                    "reason": reason,
                    "phase": phase,
                    "skipped_reason": skipped_reason,
                }),
                false,
                truncated,
            )
        }
        _ => unreachable!("compaction_event_parts called with wrong event"),
    }
}

fn compaction_completed_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    let AgentEvent::RuntimeCompactionCompleted {
        reason,
        phase,
        backend,
        provider,
        route,
        token_before,
        token_after,
        history_items_before,
        history_items_after,
        generation,
        repair_applied,
    } = event
    else {
        unreachable!("compaction_completed_parts called with wrong event");
    };
    (
        TaskEventKind::RuntimeCompactionCompleted,
        "Compaction completed".to_string(),
        json!({
            "reason": reason,
            "phase": phase,
            "backend": backend,
            "provider": provider,
            "route": route,
            "token_before": token_before,
            "token_after": token_after,
            "history_items_before": history_items_before,
            "history_items_after": history_items_after,
            "generation": generation,
            "repair_applied": repair_applied,
        }),
        false,
        false,
    )
}

fn maintenance_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::RepeatedCompactionWarning { kind, count } => (
            TaskEventKind::RepeatedCompactionWarning,
            "Repeated compaction warning".to_string(),
            json!({ "kind": kind, "count": count }),
            false,
            false,
        ),
        AgentEvent::HistoryRepairApplied { .. } => history_repair_event_parts(event),
        AgentEvent::RateLimitRetrying {
            attempt,
            max_attempts,
            unbounded,
            wait_secs,
            provider,
        } => (
            TaskEventKind::RateLimitRetrying,
            "Rate limit retrying".to_string(),
            json!({
                "attempt": attempt,
                "max_attempts": max_attempts,
                "unbounded": unbounded,
                "wait_secs": wait_secs,
                "provider": provider,
            }),
            false,
            false,
        ),
        AgentEvent::LlmRetrying { .. } => llm_retrying_event_parts(event),
        AgentEvent::ProviderFailoverActivated {
            from_provider,
            from_model,
            to_provider,
            to_model,
        } => (
            TaskEventKind::ProviderFailoverActivated,
            "Provider failover activated".to_string(),
            json!({
                "from_provider": from_provider,
                "from_model": from_model,
                "to_provider": to_provider,
                "to_model": to_model,
            }),
            false,
            false,
        ),
        _ => unreachable!("maintenance_event_parts called with wrong event"),
    }
}

fn history_repair_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    let AgentEvent::HistoryRepairApplied {
        provider,
        strict_tool_history,
        dropped_tool_results,
        trimmed_tool_calls,
        converted_tool_call_messages,
        dropped_tool_call_messages,
    } = event
    else {
        unreachable!("history_repair_event_parts called with wrong event");
    };
    (
        TaskEventKind::HistoryRepairApplied,
        "History repair applied".to_string(),
        json!({
            "provider": provider,
            "strict_tool_history": strict_tool_history,
            "dropped_tool_results": dropped_tool_results,
            "trimmed_tool_calls": trimmed_tool_calls,
            "converted_tool_call_messages": converted_tool_call_messages,
            "dropped_tool_call_messages": dropped_tool_call_messages,
        }),
        false,
        false,
    )
}

fn llm_retrying_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    let AgentEvent::LlmRetrying {
        attempt,
        max_attempts,
        unbounded,
        wait_secs,
        provider,
        error_class,
    } = event
    else {
        unreachable!("llm_retrying_event_parts called with wrong event");
    };
    (
        TaskEventKind::LlmRetrying,
        "LLM retrying".to_string(),
        json!({
            "attempt": attempt,
            "max_attempts": max_attempts,
            "unbounded": unbounded,
            "wait_secs": wait_secs,
            "provider": provider,
            "error_class": error_class,
        }),
        false,
        false,
    )
}

fn truncate_summary(value: &str) -> String {
    truncate_text(value, EVENT_SUMMARY_MAX_CHARS).0
}

fn preview_event_text(value: &str, max_chars: usize) -> (String, bool, bool) {
    let (redacted_value, redacted) = redact_sensitive_text(value);
    let (preview, truncated) = truncate_text(&redacted_value, max_chars);
    (preview, truncated, redacted)
}

fn truncate_text(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_string(), false);
    }

    let mut truncated = value.chars().take(max_chars).collect::<String>();
    truncated.push_str("...");
    (truncated, true)
}

fn redact_sensitive_text(value: &str) -> (String, bool) {
    if let Ok(mut json) = serde_json::from_str::<Value>(value) {
        let redacted = redact_sensitive_json(&mut json);
        if redacted {
            return (json.to_string(), true);
        }
        return (value.to_string(), false);
    }

    if contains_sensitive_marker(value) {
        return (REDACTED_EVENT_PAYLOAD.to_string(), true);
    }
    (value.to_string(), false)
}

fn redact_sensitive_json(value: &mut Value) -> bool {
    match value {
        Value::Object(map) => {
            let mut redacted = false;
            for (key, value) in map.iter_mut() {
                if is_sensitive_key(key) {
                    *value = Value::String("[redacted]".to_string());
                    redacted = true;
                } else if redact_sensitive_json(value) {
                    redacted = true;
                }
            }
            redacted
        }
        Value::Array(items) => items.iter_mut().any(redact_sensitive_json),
        _ => false,
    }
}

fn contains_sensitive_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    SENSITIVE_KEY_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    SENSITIVE_KEY_MARKERS
        .iter()
        .map(|marker| marker.replace(['-', '_'], ""))
        .any(|marker| normalized.contains(&marker))
}

const SENSITIVE_KEY_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "auth_token",
    "bootstrap_token",
    "cookie",
    "csrf",
    "password",
    "secret",
    "session_token",
    "token",
];

#[cfg(test)]
mod tests {
    use super::{collect_events, event_variant_name, BrowserEventScope, TaskEventLog};
    use oxide_agent_core::agent::compaction::{
        CompactionBackend, CompactionPhase, CompactionReason,
    };
    use oxide_agent_core::agent::progress::AgentEvent;
    use oxide_agent_web_contracts::TaskEventKind;
    use tokio::sync::mpsc;

    #[test]
    fn runtime_compaction_events_use_stable_web_event_names() {
        assert_eq!(
            event_variant_name(&AgentEvent::RuntimeCompactionStarted {
                reason: CompactionReason::Manual,
                phase: CompactionPhase::Manual,
                backend: CompactionBackend::LocalLlmSummary,
                provider: None,
                route: None,
                token_before: 1200,
                history_items_before: 8,
            }),
            "compaction_started"
        );
        assert_eq!(
            event_variant_name(&AgentEvent::RuntimeCompactionCompleted {
                reason: CompactionReason::Manual,
                phase: CompactionPhase::Manual,
                backend: CompactionBackend::LocalLlmSummary,
                provider: "mock".to_string(),
                route: "compact".to_string(),
                token_before: 1200,
                token_after: 700,
                history_items_before: 8,
                history_items_after: 3,
                generation: 1,
                repair_applied: false,
            }),
            "compaction_completed"
        );
        assert_eq!(
            event_variant_name(&AgentEvent::RuntimeCompactionFailed {
                reason: CompactionReason::ContextLimit,
                phase: CompactionPhase::MidTurn,
                backend: CompactionBackend::LocalLlmSummary,
                provider: Some("mock".to_string()),
                route: Some("compact".to_string()),
                error: "summary failed".to_string(),
            }),
            "compaction_failed"
        );
        assert_eq!(
            event_variant_name(&AgentEvent::RuntimeCompactionSkipped {
                reason: CompactionReason::PreTurn,
                phase: CompactionPhase::PreSampling,
                skipped_reason: "already within budget".to_string(),
            }),
            "compaction_skipped"
        );
    }

    #[tokio::test]
    async fn collect_events_records_runtime_compaction_without_pruning_event() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::RuntimeCompactionStarted {
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            backend: CompactionBackend::LocalLlmSummary,
            provider: None,
            route: None,
            token_before: 1200,
            history_items_before: 8,
        })
        .await
        .expect("send runtime compaction start");
        tx.send(AgentEvent::RuntimeCompactionCompleted {
            reason: CompactionReason::Manual,
            phase: CompactionPhase::Manual,
            backend: CompactionBackend::LocalLlmSummary,
            provider: "mock".to_string(),
            route: "compact".to_string(),
            token_before: 1200,
            token_after: 700,
            history_items_before: 8,
            history_items_after: 3,
            generation: 1,
            repair_applied: false,
        })
        .await
        .expect("send runtime compaction completion");
        tx.send(AgentEvent::Finished)
            .await
            .expect("send finished event");
        drop(tx);

        let result = collect_events(event_log.clone(), rx, None, None, None).await;
        let event_names: Vec<String> = event_log
            .drain()
            .await
            .into_iter()
            .map(|entry| entry.event_name)
            .collect();

        assert_eq!(
            event_names,
            vec![
                "compaction_started".to_string(),
                "compaction_completed".to_string(),
                "finished".to_string(),
            ]
        );
        assert!(!event_names.iter().any(|event| event == "pruning_applied"));
        assert!(result
            .state
            .last_compaction_status
            .as_deref()
            .is_some_and(|status| status.contains("Compaction: compacted history")));
    }

    #[tokio::test]
    async fn collect_events_builds_persisted_browser_events_with_payload_previews() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);
        let long_output = "x".repeat(super::EVENT_PREVIEW_MAX_CHARS + 10);

        tx.send(AgentEvent::ToolResult {
            name: "execute_command".to_string(),
            output: long_output,
            success: true,
        })
        .await
        .expect("send tool result");
        tx.send(AgentEvent::Finished).await.expect("send finished");
        drop(tx);

        let result = collect_events(
            event_log,
            rx,
            Some(BrowserEventScope::new(
                7,
                "session-1".to_string(),
                "task-1".to_string(),
            )),
            None,
            None,
        )
        .await;

        assert_eq!(result.persisted_events.len(), 2);
        let tool_result = &result.persisted_events[0];
        assert_eq!(tool_result.seq, 1);
        assert_eq!(tool_result.kind, TaskEventKind::ToolResult);
        assert_eq!(tool_result.summary, "execute_command");
        assert_eq!(tool_result.payload["name"], "execute_command");
        assert_eq!(tool_result.payload["success"], true);
        assert!(tool_result.truncated);
        assert_eq!(result.persisted_events[1].kind, TaskEventKind::Finished);
    }

    #[tokio::test]
    async fn collect_events_redacts_sensitive_tool_payload_previews() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::ToolCall {
            name: "ssh_exec".to_string(),
            input: serde_json::json!({
                "command": "deploy",
                "password": "correct horse battery staple",
                "nested": { "api_key": "sk-live-secret" }
            })
            .to_string(),
            command_preview: Some("export OPENAI_API_KEY=sk-live-secret".to_string()),
        })
        .await
        .expect("send tool call");
        tx.send(AgentEvent::ToolResult {
            name: "ssh_exec".to_string(),
            output: "SESSION_TOKEN=raw-session-token".to_string(),
            success: true,
        })
        .await
        .expect("send tool result");
        drop(tx);

        let result = collect_events(
            event_log,
            rx,
            Some(BrowserEventScope::new(
                7,
                "session-1".to_string(),
                "task-1".to_string(),
            )),
            None,
            None,
        )
        .await;

        let tool_call = &result.persisted_events[0];
        assert_eq!(tool_call.kind, TaskEventKind::ToolCall);
        assert!(tool_call.redacted);
        let input_preview = tool_call.payload["input_preview"]
            .as_str()
            .expect("input preview");
        assert!(input_preview.contains("[redacted]"));
        assert!(!input_preview.contains("correct horse battery staple"));
        assert!(!input_preview.contains("sk-live-secret"));
        assert_eq!(
            tool_call.payload["command_preview"],
            super::REDACTED_EVENT_PAYLOAD
        );

        let tool_result = &result.persisted_events[1];
        assert_eq!(tool_result.kind, TaskEventKind::ToolResult);
        assert!(tool_result.redacted);
        assert_eq!(
            tool_result.payload["output_preview"],
            super::REDACTED_EVENT_PAYLOAD
        );
    }

    #[tokio::test]
    async fn collect_events_streams_live_progress_snapshots() {
        let event_log = TaskEventLog::new();
        let (event_tx, event_rx) = mpsc::channel(8);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

        event_tx
            .send(AgentEvent::Reasoning {
                summary: "Collecting detailed evidence".to_string(),
            })
            .await
            .expect("send reasoning event");
        event_tx
            .send(AgentEvent::Finished)
            .await
            .expect("send finished event");
        drop(event_tx);

        let result = collect_events(event_log, event_rx, None, None, Some(progress_tx)).await;
        let mut snapshots = Vec::new();
        while let Some(snapshot) = progress_rx.recv().await {
            snapshots.push(snapshot);
        }

        assert!(snapshots.iter().any(|snapshot| {
            snapshot.current_thought.as_deref() == Some("Collecting detailed evidence")
        }));
        assert!(snapshots
            .last()
            .is_some_and(|snapshot| snapshot.is_finished));
        assert!(result.state.is_finished);
    }
}
