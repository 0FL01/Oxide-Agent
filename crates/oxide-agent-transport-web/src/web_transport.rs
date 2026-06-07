//! `AgentTransport` implementation for the web transport.
//!
//! Collects `AgentEvent`s into an in-memory buffer and exposes progress
//! via the HTTP API. Unlike Telegram transport, this does not send messages
//! to any chat — it only records the event timeline for later inspection.

use crate::persistence::{WebTaskFileRecord, WebUiStore, WEB_TASK_FILE_SCHEMA_VERSION};
use oxide_agent_core::agent::progress::{AgentEvent, FileDeliveryKind, ProgressState};
use oxide_agent_runtime::{AgentTransport, DeliveryMode};
use oxide_agent_web_contracts::{PersistedTaskEvent, ProgressSnapshot, TaskEventKind, TaskStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use uuid::Uuid;

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
const TOOL_DISPLAY_MARKDOWN_MAX_CHARS: usize = 12_000;
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

#[derive(Debug, Clone)]
struct BrowserStoredFile {
    file_id: String,
    download_url: String,
    content_type: String,
    size_bytes: usize,
}

impl BrowserStoredFile {
    fn receipt(&self) -> oxide_agent_core::agent::progress::FileDeliveryReceipt {
        oxide_agent_core::agent::progress::FileDeliveryReceipt {
            file_id: Some(self.file_id.clone()),
            download_url: Some(self.download_url.clone()),
        }
    }
}

/// Events collected for a single task execution.
#[derive(Debug, Clone)]
pub struct TaskEventLog {
    /// Lightweight event entries kept for backwards compatibility with existing
    /// `drain`/`take`/`snapshot` callers. New SSE consumers should prefer
    /// `persisted_snapshot` and the broadcast channel.
    pub events: Arc<RwLock<Vec<TaskEventEntry>>>,
    /// Full `PersistedTaskEvent` rows in append order, deduped by `seq`.
    /// New consumers (SSE, late subscribers) read from this for the
    /// authoritative in-memory view of persisted events.
    persisted: Arc<RwLock<Vec<PersistedTaskEvent>>>,
    /// Latest task status known to the persister. Updated via
    /// `notify_status`; read by late subscribers that connect after a
    /// status change has already been broadcast.
    last_status: Arc<RwLock<Option<TaskStatus>>>,
    /// Latest progress snapshot known to the persister. Updated via
    /// `notify_progress`; read by late subscribers.
    last_progress_snapshot: Arc<RwLock<Option<ProgressSnapshot>>>,
    /// Broadcasts each new event to SSE subscribers.
    broadcast_tx: broadcast::Sender<TaskEventLogMessage>,
    /// Flag set when the task is done.
    done: Arc<RwLock<bool>>,
    /// Monotonic timestamp recorded the first time `close()` is called.
    /// Used by the lifecycle cleanup task in `EVENT_LOGS` to evict the
    /// entry after a retention window. `None` while the log is still open.
    closed_at: Arc<RwLock<Option<Instant>>>,
}

/// A simplified event entry that stores only the event name.
/// Full event data is available in `ProgressState` via the `/progress` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskEventEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_name: String,
}

/// Messages broadcast to live SSE subscribers.
///
/// `Persisted` carries the full `PersistedTaskEvent` row so subscribers do not
/// need a separate DB read to render the event. `Status` and `Progress` carry
/// out-of-band status/progress updates from the persister (e.g. terminal
/// state changes, progress snapshots) so subscribers see them without a DB
/// poll. `Closed` is the terminal sentinel that tells subscribers the log
/// will not produce any more events.
#[derive(Debug, Clone)]
pub enum TaskEventLogMessage {
    Persisted {
        event: PersistedTaskEvent,
    },
    Status {
        status: TaskStatus,
        final_response_available: bool,
        last_seq: u64,
    },
    Progress {
        snapshot: ProgressSnapshot,
        last_seq: u64,
    },
    Closed,
}

/// Capacity of the in-process broadcast channel. Sized to comfortably cover
/// bursty tool-call sequences; a slow consumer that overflows this ring buffer
/// is expected to fall back to DB replay.
const TASK_EVENT_BROADCAST_CAPACITY: usize = 256;

impl TaskEventLog {
    /// Create a new empty event log.
    #[must_use]
    pub fn new() -> Self {
        let (broadcast_tx, _broadcast_rx) = broadcast::channel(TASK_EVENT_BROADCAST_CAPACITY);
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            persisted: Arc::new(RwLock::new(Vec::new())),
            last_status: Arc::new(RwLock::new(None)),
            last_progress_snapshot: Arc::new(RwLock::new(None)),
            broadcast_tx,
            done: Arc::new(RwLock::new(false)),
            closed_at: Arc::new(RwLock::new(None)),
        }
    }

    /// Record a lightweight event entry. The full `PersistedTaskEvent` row
    /// (with `seq`, `kind`, `payload`, etc.) is recorded separately via
    /// `push_persisted`; that path also broadcasts to SSE subscribers.
    pub async fn push(&self, event: &AgentEvent) {
        let event_name = event_variant_name(event);
        let entry = TaskEventEntry {
            timestamp: chrono::Utc::now(),
            event_name,
        };
        self.events.write().await.push(entry);
    }

    /// Record a fully built `PersistedTaskEvent` and broadcast it to SSE
    /// subscribers. Dedupes by `seq` so duplicate appends (e.g. retry from the
    /// DB persister path) do not produce duplicate SSE events.
    pub async fn push_persisted(&self, event: PersistedTaskEvent) {
        let seq = event.seq;
        let event_for_broadcast = event.clone();
        {
            let mut store = self.persisted.write().await;
            if let Some(existing) = store.iter().position(|e| e.seq == seq) {
                store[existing] = event;
            } else {
                store.push(event);
            }
        }
        // Broadcast to SSE subscribers. Ignore error if no active receivers.
        let _ = self.broadcast_tx.send(TaskEventLogMessage::Persisted {
            event: event_for_broadcast,
        });
    }

    /// Subscribe to new events as they are pushed. The returned receiver will
    /// receive events published after this call (not the current snapshot).
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<TaskEventLogMessage> {
        self.broadcast_tx.subscribe()
    }

    /// Returns the highest `seq` recorded via `push_persisted`, or 0 if none.
    pub async fn latest_seq(&self) -> u64 {
        let store = self.persisted.read().await;
        store.iter().map(|e| e.seq).max().unwrap_or(0)
    }

    /// Returns a snapshot of all persisted events recorded via `push_persisted`,
    /// in append order.
    pub async fn persisted_snapshot(&self) -> Vec<PersistedTaskEvent> {
        self.persisted.read().await.clone()
    }

    /// Broadcast a status change to live subscribers. Stores the latest
    /// status so late subscribers can read it via `last_status` instead of
    /// falling back to DB.
    pub async fn notify_status(
        &self,
        status: TaskStatus,
        final_response_available: bool,
        last_seq: u64,
    ) {
        *self.last_status.write().await = Some(status);
        let _ = self.broadcast_tx.send(TaskEventLogMessage::Status {
            status,
            final_response_available,
            last_seq,
        });
    }

    /// Broadcast a progress snapshot to live subscribers. Stores the latest
    /// snapshot so late subscribers can read it via `last_progress_snapshot`.
    pub async fn notify_progress(&self, snapshot: ProgressSnapshot, last_seq: u64) {
        *self.last_progress_snapshot.write().await = Some(snapshot.clone());
        let _ = self
            .broadcast_tx
            .send(TaskEventLogMessage::Progress { snapshot, last_seq });
    }

    /// Returns the latest status broadcast via `notify_status`, or `None`
    /// if no status change has been broadcast yet.
    pub async fn last_status(&self) -> Option<TaskStatus> {
        *self.last_status.read().await
    }

    /// Returns the latest progress snapshot broadcast via `notify_progress`,
    /// or `None` if no progress update has been broadcast yet.
    pub async fn last_progress_snapshot(&self) -> Option<ProgressSnapshot> {
        self.last_progress_snapshot.read().await.clone()
    }

    /// Mark the event log as done — SSE subscribers will stop receiving after this.
    /// Idempotent: subsequent calls do nothing and do not refresh the
    /// `closed_at` timestamp, so the lifecycle TTL is anchored to the
    /// first close.
    pub async fn close(&self) {
        {
            let mut d = self.done.write().await;
            if *d {
                return;
            }
            *d = true;
        }
        *self.closed_at.write().await = Some(Instant::now());
        // Send a terminal sentinel to wake up any waiting receivers.
        let _ = self.broadcast_tx.send(TaskEventLogMessage::Closed);
    }

    /// Returns `true` if `close()` has been called.
    pub async fn is_closed(&self) -> bool {
        *self.done.read().await
    }

    /// Returns the monotonic instant at which `close()` was first called,
    /// or `None` if the log is still open. The lifecycle cleanup task in
    /// `EVENT_LOGS` uses this to schedule eviction.
    pub async fn closed_at(&self) -> Option<Instant> {
        *self.closed_at.read().await
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

fn delivered_file_download_url(scope: &BrowserEventScope, file_id: &str) -> String {
    format!(
        "/api/v1/sessions/{}/tasks/{}/files/{file_id}",
        scope.session_id, scope.task_id
    )
}

fn infer_content_type(file_name: &str) -> &'static str {
    match Path::new(file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("txt") | Some("log") | Some("md") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("csv") => "text/csv; charset=utf-8",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("m4a") => "audio/mp4",
        Some("flac") => "audio/flac",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("zip") => "application/zip",
        Some("gz") => "application/gzip",
        Some("tar") => "application/x-tar",
        _ => "application/octet-stream",
    }
}

async fn persist_browser_file(
    web_store: &dyn WebUiStore,
    scope: &BrowserEventScope,
    kind: FileDeliveryKind,
    file_name: &str,
    content: &[u8],
) -> Result<BrowserStoredFile, String> {
    let file_id = Uuid::new_v4().to_string();
    let content_type = infer_content_type(file_name).to_string();
    let record = WebTaskFileRecord {
        schema_version: WEB_TASK_FILE_SCHEMA_VERSION,
        user_id: scope.user_id,
        session_id: scope.session_id.clone(),
        task_id: scope.task_id.clone(),
        file_id: file_id.clone(),
        file_name: file_name.to_string(),
        content_type: content_type.clone(),
        size_bytes: content.len() as u64,
        delivery_kind: kind,
        created_at: chrono::Utc::now(),
    };
    web_store
        .save_task_file(record, content.to_vec())
        .await
        .map_err(|error| error.to_string())?;
    Ok(BrowserStoredFile {
        file_id: file_id.clone(),
        download_url: delivered_file_download_url(scope, &file_id),
        content_type,
        size_bytes: content.len(),
    })
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
    ) -> Result<oxide_agent_core::agent::progress::FileDeliveryReceipt, anyhow::Error> {
        // Record file delivery as a synthetic event so tests can observe it.
        let event = AgentEvent::FileToSend {
            kind,
            file_name: file_name.to_string(),
            content: content.to_vec(),
        };
        self.event_log.push(&event).await;
        Ok(Default::default())
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
    pub id: String,
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
    browser_file_store: Option<Arc<dyn WebUiStore>>,
    live_event_tx: Option<mpsc::UnboundedSender<PersistedTaskEvent>>,
    live_progress_tx: Option<mpsc::UnboundedSender<ProgressState>>,
) -> EventCollectionResult {
    use oxide_agent_core::agent::progress::ProgressState;

    let mut state = ProgressState::new(100); // max_iterations, can be overridden
    let mut timestamps = MilestoneTimestamps::default();
    let mut tool_calls = Vec::new();
    let mut active_tool_calls: HashMap<String, usize> = HashMap::new();
    let mut persisted_events = Vec::new();
    let mut next_seq = 1;

    while let Some(event) = rx.recv().await {
        let event_received_at = chrono::Utc::now();
        // Classify event type once to avoid borrow-after-move.
        let is_thinking = matches!(&event, AgentEvent::Thinking { .. });
        let is_reasoning = matches!(&event, AgentEvent::Reasoning { .. });
        let is_file_to_send = matches!(
            &event,
            AgentEvent::FileToSend { .. } | AgentEvent::FileToSendWithConfirmation { .. }
        );
        let is_terminal = matches!(
            &event,
            AgentEvent::Finished | AgentEvent::Cancelled | AgentEvent::Error(_)
        );
        let is_milestone = matches!(&event, AgentEvent::Milestone { .. });
        let mut file_storage_error = None::<String>;
        let stored_file = match (&browser_event_scope, browser_file_store.as_ref(), &event) {
            (
                Some(scope),
                Some(web_store),
                AgentEvent::FileToSend {
                    kind,
                    file_name,
                    content,
                },
            ) => match persist_browser_file(web_store.as_ref(), scope, *kind, file_name, content)
                .await
            {
                Ok(file) => Some(file),
                Err(error) => {
                    file_storage_error = Some(error);
                    None
                }
            },
            (
                Some(scope),
                Some(web_store),
                AgentEvent::FileToSendWithConfirmation {
                    kind,
                    file_name,
                    content,
                    ..
                },
            ) => match persist_browser_file(web_store.as_ref(), scope, *kind, file_name, content)
                .await
            {
                Ok(file) => Some(file),
                Err(error) => {
                    file_storage_error = Some(error);
                    None
                }
            },
            _ => None,
        };

        // Track first Thinking/Reasoning to derive llm_call_started_ms on collector-side.
        if timestamps.first_thinking_at.is_none() && is_thinking {
            timestamps.first_thinking_at = Some(chrono::Utc::now());
        }
        if timestamps.first_reasoning_at.is_none() && is_reasoning {
            timestamps.first_reasoning_at = Some(chrono::Utc::now());
        }

        // Track named milestones from the agent core.
        if let AgentEvent::Milestone { name, timestamp_ms } = &event
            && let Some(ts) = chrono::DateTime::from_timestamp_millis(*timestamp_ms) {
                timestamps.named_milestones.insert(name.clone(), ts);
            }

        match &event {
            AgentEvent::ToolCall { id, name, .. } => {
                let idx = tool_calls.len();
                let key = tool_event_pairing_key(id, name);
                tool_calls.push(CollectedToolCallTiming {
                    id: id.clone(),
                    name: name.clone(),
                    started_at: chrono::Utc::now(),
                    finished_at: None,
                });
                active_tool_calls.insert(key, idx);
            }
            AgentEvent::ToolResult { id, name, .. } => {
                let key = tool_event_pairing_key(id, name);
                if let Some(idx) = active_tool_calls.remove(&key)
                    && let Some(tool_call) = tool_calls.get_mut(idx) {
                        tool_call.finished_at = Some(chrono::Utc::now());
                    }
            }
            _ => {}
        }

        // Milestones are tracked separately and FileToSend is already recorded by the
        // transport itself, but both still should update progress state for consistency.
        if !is_milestone && !is_file_to_send {
            event_log.push(&event).await;
        }

        if let Some(scope) = browser_event_scope.as_ref()
            && should_persist_browser_event(&event) {
                let persisted_event = persisted_event_from_agent_event(
                    scope,
                    next_seq,
                    event_received_at,
                    &event,
                    stored_file.as_ref(),
                    file_storage_error.as_deref(),
                );
                if let Some(live_event_tx) = live_event_tx.as_ref() {
                    let _ = live_event_tx.send(persisted_event.clone());
                }
                persisted_events.push(persisted_event);
                next_seq += 1;
            }

        match event {
            AgentEvent::FileToSendWithConfirmation {
                kind,
                file_name,
                content,
                confirmation_tx,
                ..
            } => {
                let confirmation = file_storage_error.map(Err).unwrap_or_else(|| {
                    Ok(stored_file
                        .as_ref()
                        .map_or_else(Default::default, BrowserStoredFile::receipt))
                });
                let _ = confirmation_tx.send(confirmation);
                state.update(AgentEvent::FileToSend {
                    kind,
                    file_name,
                    content,
                });
            }
            other => state.update(other),
        }
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

fn tool_event_pairing_key(id: &str, name: &str) -> String {
    if id.is_empty() {
        format!("legacy-name:{name}")
    } else {
        format!("id:{id}")
    }
}

fn persisted_event_from_agent_event(
    scope: &BrowserEventScope,
    seq: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    event: &AgentEvent,
    stored_file: Option<&BrowserStoredFile>,
    file_storage_error: Option<&str>,
) -> PersistedTaskEvent {
    let (kind, summary, payload, redacted, truncated) =
        browser_event_parts(event, stored_file, file_storage_error);
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

fn should_persist_browser_event(event: &AgentEvent) -> bool {
    !matches!(
        event,
        AgentEvent::RateLimitRetrying { .. }
            | AgentEvent::LlmRetrying { .. }
            | AgentEvent::ProviderFailoverActivated { .. }
    )
}

fn browser_event_parts(
    event: &AgentEvent,
    stored_file: Option<&BrowserStoredFile>,
    file_storage_error: Option<&str>,
) -> (TaskEventKind, String, Value, bool, bool) {
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
        AgentEvent::TodosUpdated { source, todos } => (
            TaskEventKind::TodosUpdated,
            "Todos updated".to_string(),
            json!({ "source": source.as_str(), "todos": todos }),
            false,
            false,
        ),
        AgentEvent::FileToSend { .. } | AgentEvent::FileToSendWithConfirmation { .. } => {
            file_event_parts(event, stored_file, file_storage_error)
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
            id,
            source,
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
                    "id": id,
                    "source": source.as_str(),
                    "name": name,
                    "input_preview": input_preview,
                    "command_preview": command_preview,
                }),
                input_redacted || command_redacted,
                input_truncated || command_truncated,
            )
        }
        AgentEvent::ToolResult {
            id,
            source,
            name,
            output,
            success,
        } => {
            let (output_preview, truncated, redacted) =
                preview_event_text(output, EVENT_PREVIEW_MAX_CHARS);
            let result_summary = tool_result_summary(name, *success, output);
            let display_payload = tool_display_payload(name, *success, output);
            // Extract duration_ms from the full ToolOutput JSON before truncation,
            // so the UI can display timing even when the preview is too large to parse.
            let duration_ms: Option<i64> = serde_json::from_str::<Value>(output)
                .ok()
                .as_ref()
                .and_then(|v| v.get("duration_ms"))
                .and_then(|v| v.as_i64());
            (
                TaskEventKind::ToolResult,
                truncate_summary(result_summary.as_deref().unwrap_or(name)),
                json!({
                    "id": id,
                    "source": source.as_str(),
                    "name": name,
                    "success": success,
                    "result_summary": result_summary,
                    "duration_ms": duration_ms,
                    "display_payload": display_payload,
                    "output_preview": output_preview,
                }),
                redacted,
                truncated,
            )
        }
        _ => unreachable!("tool_event_parts called with non-tool event"),
    }
}

fn file_event_parts(
    event: &AgentEvent,
    stored_file: Option<&BrowserStoredFile>,
    file_storage_error: Option<&str>,
) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::FileToSend {
            kind,
            file_name,
            content,
        } => (
            TaskEventKind::FileToSend,
            truncate_summary(file_name),
            file_event_payload(
                *kind,
                file_name,
                content.len(),
                stored_file,
                file_storage_error,
            ),
            false,
            false,
        ),
        AgentEvent::FileToSendWithConfirmation {
            kind,
            file_name,
            content,
            source_path: _,
            confirmation_tx: _,
        } => (
            TaskEventKind::FileToSend,
            truncate_summary(file_name),
            file_event_payload(
                *kind,
                file_name,
                content.len(),
                stored_file,
                file_storage_error,
            ),
            false,
            false,
        ),
        _ => unreachable!("file_event_parts called with non-file event"),
    }
}

fn file_event_payload(
    kind: FileDeliveryKind,
    file_name: &str,
    byte_len: usize,
    stored_file: Option<&BrowserStoredFile>,
    file_storage_error: Option<&str>,
) -> Value {
    let mut payload = json!({
        "delivery_kind": kind,
        "file_name": file_name,
        "byte_len": byte_len,
    });
    if let Some(object) = payload.as_object_mut() {
        if let Some(stored_file) = stored_file {
            object.insert("file_id".to_string(), json!(stored_file.file_id));
            object.insert("download_url".to_string(), json!(stored_file.download_url));
            object.insert("content_type".to_string(), json!(stored_file.content_type));
            object.insert("size_bytes".to_string(), json!(stored_file.size_bytes));
        }
        if let Some(error) = file_storage_error {
            object.insert("delivery_error".to_string(), json!(error));
        }
    }
    payload
}

fn lifecycle_event_parts(event: &AgentEvent) -> (TaskEventKind, String, Value, bool, bool) {
    match event {
        AgentEvent::Continuation {
            source,
            reason,
            count,
        } => {
            let (reason_preview, truncated) = truncate_text(reason, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::Continuation,
                "Continuation".to_string(),
                json!({ "source": source.as_str(), "reason": reason_preview, "count": count }),
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
        AgentEvent::Reasoning { source, summary } => {
            let (summary_preview, truncated) = truncate_text(summary, EVENT_PREVIEW_MAX_CHARS);
            (
                TaskEventKind::Reasoning,
                "Reasoning".to_string(),
                json!({ "source": source.as_str(), "summary": summary_preview }),
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

fn tool_display_payload(name: &str, success: bool, output: &str) -> Option<Value> {
    if name != "crawl4ai_markdown" || !success {
        return None;
    }

    let output = serde_json::from_str::<Value>(output).ok()?;
    let stdout_text = stream_text_from_output(&output, "stdout")?;
    let crawl = serde_json::from_str::<Value>(&stdout_text).ok()?;
    let markdown = crawl.get("markdown").and_then(Value::as_str).unwrap_or("");
    let (safe_markdown, _) = redact_sensitive_text(markdown);
    let (markdown_preview, markdown_preview_truncated) =
        truncate_text(&safe_markdown, TOOL_DISPLAY_MARKDOWN_MAX_CHARS);

    Some(json!({
        "provider": "crawl4ai_markdown",
        "url": crawl.get("url").and_then(Value::as_str),
        "final_url": crawl.get("final_url").and_then(Value::as_str),
        "status_code": crawl.get("status_code").and_then(Value::as_u64),
        "markdown_kind": crawl.get("markdown_kind").and_then(Value::as_str),
        "markdown": markdown_preview,
        "chars": crawl
            .get("chars")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| markdown.chars().count() as u64),
        "truncated": crawl
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "markdown_preview_truncated": markdown_preview_truncated,
        "fresh": crawl.get("fresh").and_then(Value::as_bool),
    }))
}

fn stream_text_from_output(output: &Value, stream_name: &str) -> Option<String> {
    let stream = output.get(stream_name)?;
    if stream
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return None;
    }

    if let Some(text) = stream.get("text").and_then(Value::as_str)
        && !text.is_empty() {
            return Some(text.to_string());
        }

    let head = stream.get("head").and_then(Value::as_str);
    let tail = stream.get("tail").and_then(Value::as_str);
    match (head, tail) {
        (Some(head), Some(tail)) => Some(format!("{head}\n...\n{tail}")),
        (Some(head), None) => Some(head.to_string()),
        (None, Some(tail)) => Some(tail.to_string()),
        _ => None,
    }
}

fn tool_result_summary(name: &str, success: bool, output: &str) -> Option<String> {
    if success {
        return None;
    }

    let value = serde_json::from_str::<Value>(output).ok()?;
    let payload = value.get("structured_payload")?;
    match (name, payload.get("provider").and_then(Value::as_str)) {
        ("web_markdown", Some("web_markdown")) => {
            let error_kind = payload.get("error_kind").and_then(Value::as_str)?;
            let host = payload.get("host").and_then(Value::as_str);
            let status_code = payload.get("status_code").and_then(Value::as_u64);

            Some(match error_kind {
                "anti_bot" => host
                    .map(|host| format!("anti_bot at {host}"))
                    .unwrap_or_else(|| "anti_bot".to_string()),
                "http_status" => status_code
                    .map(|code| format!("http_status {code}"))
                    .unwrap_or_else(|| "http_status".to_string()),
                "timeout" => host
                    .map(|host| format!("timeout at {host}"))
                    .unwrap_or_else(|| "timeout".to_string()),
                "network" => host
                    .map(|host| format!("network at {host}"))
                    .unwrap_or_else(|| "network".to_string()),
                other => host
                    .map(|host| format!("{other} at {host}"))
                    .unwrap_or_else(|| other.to_string()),
            })
        }
        ("crawl4ai_markdown", Some("crawl4ai_markdown")) => {
            let error_kind = payload.get("error_kind").and_then(Value::as_str)?;
            let host = payload.get("host").and_then(Value::as_str);
            let status_code = payload.get("status_code").and_then(Value::as_u64);

            Some(match error_kind {
                "crawl4ai_http_status" => status_code
                    .map(|code| format!("http_status {code}"))
                    .unwrap_or_else(|| "http_status".to_string()),
                "crawl4ai_unavailable" => "crawl4ai unavailable".to_string(),
                "crawl4ai_auth_failed" => "auth_failed".to_string(),
                "timeout" => host
                    .map(|host| format!("timeout at {host}"))
                    .unwrap_or_else(|| "timeout".to_string()),
                "dns_failed" => host
                    .map(|host| format!("dns_failed at {host}"))
                    .unwrap_or_else(|| "dns_failed".to_string()),
                "network" => host
                    .map(|host| format!("network at {host}"))
                    .unwrap_or_else(|| "network".to_string()),
                other => other.to_string(),
            })
        }
        ("duckduckgo_search" | "duckduckgo_news", Some("duckduckgo")) => {
            let error_kind = payload.get("error_kind").and_then(Value::as_str)?;
            Some(match error_kind {
                "rate_limited" => "rate_limited".to_string(),
                "blocked" => "blocked".to_string(),
                "parser_break" => "parser_break".to_string(),
                "timeout" => "timeout".to_string(),
                other => other.to_string(),
            })
        }
        _ => None,
    }
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
    use super::{
        collect_events, event_variant_name, BrowserEventScope, TaskEventLog, TaskEventLogMessage,
    };
    use crate::persistence::{InMemoryWebUiStore, WebUiStore};
    use oxide_agent_core::agent::compaction::{
        CompactionBackend, CompactionPhase, CompactionReason,
    };
    use oxide_agent_core::agent::progress::{AgentEvent, AgentEventSource, FileDeliveryKind};
    use oxide_agent_web_contracts::{
        PersistedTaskEvent, ProgressSnapshot, TaskEventKind, TaskStatus,
    };
    use std::sync::Arc;
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

        let result = collect_events(event_log.clone(), rx, None, None, None, None).await;
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
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
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
    async fn collect_events_persists_crawl_display_payload_for_truncated_output() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);
        let markdown = format!("# Title\n\n{}", "body ".repeat(1_200));
        let crawl_stdout = serde_json::json!({
            "provider": "crawl4ai_markdown",
            "url": "https://arxiv.org/abs/2602.10604",
            "final_url": "https://arxiv.org/abs/2602.10604",
            "status_code": 200,
            "markdown_kind": "markdown",
            "markdown": markdown,
            "truncated": false,
            "chars": 6_009,
            "elapsed_ms": 2936,
            "fresh": false
        })
        .to_string();
        let output = serde_json::json!({
            "tool_call_id": "call-crawl",
            "tool_name": "crawl4ai_markdown",
            "status": "success",
            "success": true,
            "duration_ms": 2968,
            "stdout": { "binary": false, "bytes_captured": crawl_stdout.len(), "bytes_total_known": crawl_stdout.len(), "text": crawl_stdout, "truncated": false },
            "stderr": { "binary": false, "bytes_captured": 0, "bytes_total_known": 0, "text": "", "truncated": false },
            "error_message": null,
            "timeout_reason": null,
            "cancellation_reason": null,
            "cleanup_status": "not_needed",
            "artifacts": []
        })
        .to_string();

        tx.send(AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
            name: "crawl4ai_markdown".to_string(),
            output,
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
            None,
        )
        .await;

        let tool_result = &result.persisted_events[0];
        assert!(tool_result.truncated);
        let display = &tool_result.payload["display_payload"];
        assert_eq!(display["provider"], "crawl4ai_markdown");
        assert_eq!(display["final_url"], "https://arxiv.org/abs/2602.10604");
        assert_eq!(display["chars"], 6_009);
        assert!(display["markdown"].as_str().unwrap().starts_with("# Title"));
    }

    #[tokio::test]
    async fn collect_events_persists_confirmed_file_delivery_and_returns_download_url_receipt() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);
        let (confirmation_tx, confirmation_rx) = tokio::sync::oneshot::channel();
        let web_store: Arc<dyn WebUiStore> = Arc::new(InMemoryWebUiStore::new());
        let scope = BrowserEventScope::new(7, "session-1".to_string(), "task-1".to_string());

        tx.send(AgentEvent::FileToSendWithConfirmation {
            kind: FileDeliveryKind::Document,
            file_name: "report.pdf".to_string(),
            content: b"pdf-ish".to_vec(),
            source_path: "/workspace/report.pdf".to_string(),
            confirmation_tx,
        })
        .await
        .expect("send file delivery event");
        tx.send(AgentEvent::Finished).await.expect("send finished");
        drop(tx);

        let result = collect_events(
            event_log,
            rx,
            Some(scope.clone()),
            Some(Arc::clone(&web_store)),
            None,
            None,
        )
        .await;

        let receipt = confirmation_rx.await.expect("receive delivery receipt");
        let receipt = receipt.expect("delivery should succeed");
        let file_id = receipt.file_id.expect("file id in receipt");
        let download_url = receipt.download_url.expect("download url in receipt");
        assert_eq!(
            download_url,
            format!(
                "/api/v1/sessions/{}/tasks/{}/files/{}",
                scope.session_id, scope.task_id, file_id
            )
        );

        let file_event = &result.persisted_events[0];
        assert_eq!(file_event.kind, TaskEventKind::FileToSend);
        assert_eq!(file_event.payload["file_id"], file_id);
        assert_eq!(file_event.payload["download_url"], download_url);
        assert_eq!(file_event.payload["content_type"], "application/pdf");
        assert_eq!(file_event.payload["size_bytes"], 7);

        let stored_file = web_store
            .load_task_file(7, &scope.session_id, &scope.task_id, &file_id)
            .await
            .expect("load stored file")
            .expect("stored file exists");
        assert_eq!(stored_file.record.file_name, "report.pdf");
        assert_eq!(stored_file.record.content_type, "application/pdf");
        assert_eq!(stored_file.content, b"pdf-ish".to_vec());
    }

    #[tokio::test]
    async fn collect_events_summarizes_web_markdown_failures() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);
        let output = serde_json::json!({
            "tool_call_id": "call-web-markdown",
            "tool_name": "web_markdown",
            "status": "failure",
            "success": false,
            "duration_ms": 418,
            "stdout": { "binary": false, "bytes_captured": 0, "bytes_total_known": 0, "text": "", "truncated": false },
            "stderr": { "binary": false, "bytes_captured": 0, "bytes_total_known": 0, "text": "", "truncated": false },
            "structured_payload": {
                "provider": "web_markdown",
                "kind": "fetch",
                "host": "ftbwiki.org",
                "error_kind": "anti_bot",
                "status_code": null
            },
            "error_message": "web_markdown blocked by anti-bot protection at ftbwiki.org",
            "timeout_reason": null,
            "cancellation_reason": null,
            "cleanup_status": "not_needed",
            "truncation": { "artifact_write_failed": false, "content_truncated": false, "max_stderr_bytes": 65536, "max_stdout_bytes": 65536, "max_tool_output_content_bytes": 131072, "stderr_truncated": false, "stdout_truncated": false },
            "artifacts": []
        })
        .to_string();

        tx.send(AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
            name: "web_markdown".to_string(),
            output,
            success: false,
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
            None,
        )
        .await;

        let tool_result = &result.persisted_events[0];
        assert_eq!(tool_result.kind, TaskEventKind::ToolResult);
        assert_eq!(tool_result.summary, "anti_bot at ftbwiki.org");
        assert_eq!(
            tool_result.payload["result_summary"],
            "anti_bot at ftbwiki.org"
        );
    }

    #[tokio::test]
    async fn collect_events_summarizes_duckduckgo_failures() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);
        let output = serde_json::json!({
            "tool_call_id": "call-duckduckgo",
            "tool_name": "duckduckgo_search",
            "status": "failure",
            "success": false,
            "duration_ms": 458,
            "stdout": { "binary": false, "bytes_captured": 0, "bytes_total_known": 0, "text": "", "truncated": false },
            "stderr": { "binary": false, "bytes_captured": 0, "bytes_total_known": 0, "text": "", "truncated": false },
            "structured_payload": {
                "provider": "duckduckgo",
                "kind": "search",
                "query": "Thaumcraft 4 Pech spawning biome",
                "error_kind": "blocked",
                "provider_unavailable": true,
                "results": []
            },
            "error_message": "DuckDuckGo is temporarily blocking requests",
            "timeout_reason": null,
            "cancellation_reason": null,
            "cleanup_status": "not_needed",
            "truncation": { "artifact_write_failed": false, "content_truncated": false, "max_stderr_bytes": 65536, "max_stdout_bytes": 65536, "max_tool_output_content_bytes": 131072, "stderr_truncated": false, "stdout_truncated": false },
            "artifacts": []
        })
        .to_string();

        tx.send(AgentEvent::ToolResult {
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
            name: "duckduckgo_search".to_string(),
            output,
            success: false,
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
            None,
        )
        .await;

        let tool_result = &result.persisted_events[0];
        assert_eq!(tool_result.kind, TaskEventKind::ToolResult);
        assert_eq!(tool_result.summary, "blocked");
        assert_eq!(tool_result.payload["result_summary"], "blocked");
    }

    #[tokio::test]
    async fn collect_events_omits_rate_limit_retrying_browser_events() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::RateLimitRetrying {
            attempt: 1,
            max_attempts: 15,
            unbounded: false,
            wait_secs: Some(10),
            provider: "llm-provider/opencode-go".to_string(),
        })
        .await
        .expect("send rate limit retrying");
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
            None,
        )
        .await;

        assert_eq!(result.persisted_events.len(), 1);
        assert_eq!(result.persisted_events[0].kind, TaskEventKind::Finished);
        assert_eq!(result.persisted_events[0].seq, 1);
    }

    #[tokio::test]
    async fn collect_events_omits_llm_retrying_browser_events() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::LlmRetrying {
            attempt: 1,
            max_attempts: 15,
            unbounded: false,
            wait_secs: Some(1),
            provider: "llm-provider/opencode-go".to_string(),
            error_class: "server_error".to_string(),
        })
        .await
        .expect("send llm retrying");
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
            None,
        )
        .await;

        assert_eq!(result.persisted_events.len(), 1);
        assert_eq!(result.persisted_events[0].kind, TaskEventKind::Finished);
        assert_eq!(result.persisted_events[0].seq, 1);
    }

    #[tokio::test]
    async fn collect_events_omits_provider_failover_browser_events() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::ProviderFailoverActivated {
            from_provider: "llm-provider/opencode-go".to_string(),
            from_model: "mimo-v2.5".to_string(),
            to_provider: "llm-provider/backup".to_string(),
            to_model: "backup-model".to_string(),
        })
        .await
        .expect("send provider failover");
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
            None,
        )
        .await;

        assert_eq!(result.persisted_events.len(), 1);
        assert_eq!(result.persisted_events[0].kind, TaskEventKind::Finished);
        assert_eq!(result.persisted_events[0].seq, 1);
    }

    #[tokio::test]
    async fn collect_events_redacts_sensitive_tool_payload_previews() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::ToolCall {
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
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
            id: "tool-1".to_string(),
            source: AgentEventSource::Root,
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
    async fn collect_events_pairs_concurrent_tool_timings_by_id() {
        let event_log = TaskEventLog::new();
        let (tx, rx) = mpsc::channel(8);

        tx.send(AgentEvent::ToolCall {
            id: "tool-a".to_string(),
            source: AgentEventSource::Root,
            name: "duckduckgo_search".to_string(),
            input: "q1".to_string(),
            command_preview: None,
        })
        .await
        .expect("send first tool call");
        tx.send(AgentEvent::ToolCall {
            id: "tool-b".to_string(),
            source: AgentEventSource::Root,
            name: "duckduckgo_search".to_string(),
            input: "q2".to_string(),
            command_preview: None,
        })
        .await
        .expect("send second tool call");
        tx.send(AgentEvent::ToolResult {
            id: "tool-b".to_string(),
            source: AgentEventSource::Root,
            name: "duckduckgo_search".to_string(),
            output: "result2".to_string(),
            success: true,
        })
        .await
        .expect("send second tool result");
        tx.send(AgentEvent::ToolResult {
            id: "tool-a".to_string(),
            source: AgentEventSource::Root,
            name: "duckduckgo_search".to_string(),
            output: "result1".to_string(),
            success: false,
        })
        .await
        .expect("send first tool result");
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
            None,
        )
        .await;

        assert_eq!(result.tool_calls.len(), 2);
        assert_eq!(result.tool_calls[0].id, "tool-a");
        assert_eq!(result.tool_calls[1].id, "tool-b");
        assert!(result
            .tool_calls
            .iter()
            .all(|call| call.finished_at.is_some()));
        assert_eq!(result.persisted_events[0].payload["id"], "tool-a");
        assert_eq!(result.persisted_events[1].payload["id"], "tool-b");
        assert_eq!(result.persisted_events[2].payload["id"], "tool-b");
        assert_eq!(result.persisted_events[3].payload["id"], "tool-a");
    }

    #[tokio::test]
    async fn collect_events_streams_live_progress_snapshots() {
        let event_log = TaskEventLog::new();
        let (event_tx, event_rx) = mpsc::channel(8);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

        event_tx
            .send(AgentEvent::Reasoning {
                source: AgentEventSource::Root,
                summary: "Collecting detailed evidence".to_string(),
            })
            .await
            .expect("send reasoning event");
        event_tx
            .send(AgentEvent::Finished)
            .await
            .expect("send finished event");
        drop(event_tx);

        let result = collect_events(event_log, event_rx, None, None, None, Some(progress_tx)).await;
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

    fn sample_persisted_event(seq: u64) -> PersistedTaskEvent {
        PersistedTaskEvent {
            schema_version: 1,
            task_id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            user_id: 7,
            seq,
            created_at: chrono::DateTime::from_timestamp(1_700_000_000, 0)
                .expect("timestamp is valid"),
            kind: TaskEventKind::ToolResult,
            summary: format!("event-{seq}"),
            payload: serde_json::json!({ "seq": seq }),
            redacted: false,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn push_persisted_broadcasts_full_event_to_subscribers() {
        let log = TaskEventLog::new();
        let mut rx = log.subscribe();
        let event = sample_persisted_event(11);

        log.push_persisted(event.clone()).await;

        match rx.recv().await {
            Ok(TaskEventLogMessage::Persisted { event: received }) => {
                assert_eq!(received.seq, 11);
                assert_eq!(received.kind, TaskEventKind::ToolResult);
                assert_eq!(received.payload, serde_json::json!({ "seq": 11 }));
            }
            other => panic!("expected Persisted message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn push_persisted_dedupes_by_seq() {
        let log = TaskEventLog::new();
        log.push_persisted(sample_persisted_event(1)).await;
        log.push_persisted(sample_persisted_event(2)).await;
        log.push_persisted(sample_persisted_event(2)).await;
        log.push_persisted(sample_persisted_event(3)).await;

        let snapshot = log.persisted_snapshot().await;
        assert_eq!(snapshot.len(), 3, "duplicate seq=2 should be deduped");
        assert_eq!(
            snapshot.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[tokio::test]
    async fn latest_seq_tracks_max_persisted_seq() {
        let log = TaskEventLog::new();
        assert_eq!(log.latest_seq().await, 0, "empty log reports 0");
        log.push_persisted(sample_persisted_event(5)).await;
        log.push_persisted(sample_persisted_event(3)).await;
        log.push_persisted(sample_persisted_event(7)).await;
        assert_eq!(log.latest_seq().await, 7);
    }

    #[tokio::test]
    async fn close_broadcasts_closed_sentinel() {
        let log = TaskEventLog::new();
        let mut rx = log.subscribe();
        log.close().await;
        match rx.recv().await {
            Ok(TaskEventLogMessage::Closed) => {}
            other => panic!("expected Closed sentinel, got {other:?}"),
        }
        assert!(log.is_closed().await);
    }

    #[tokio::test]
    async fn notify_status_broadcasts_status_message_and_records_last_status() {
        let log = TaskEventLog::new();
        let mut rx = log.subscribe();
        log.notify_status(TaskStatus::Completed, true, 7).await;
        match rx.recv().await {
            Ok(TaskEventLogMessage::Status {
                status,
                final_response_available,
                last_seq,
            }) => {
                assert_eq!(status, TaskStatus::Completed);
                assert!(final_response_available);
                assert_eq!(last_seq, 7);
            }
            other => panic!("expected Status message, got {other:?}"),
        }
        assert_eq!(log.last_status().await, Some(TaskStatus::Completed));
    }

    #[tokio::test]
    async fn notify_progress_broadcasts_progress_message_and_records_snapshot() {
        let log = TaskEventLog::new();
        let mut rx = log.subscribe();
        let snapshot = ProgressSnapshot {
            current_iteration: 3,
            max_iterations: 10,
            is_finished: false,
            error: None,
            current_thought: Some("thinking".to_string()),
            current_todos: None,
            last_compaction_status: None,
            repeated_compaction_warning: None,
            last_history_repair_status: None,
            latest_token_snapshot: None,
            llm_retry: None,
            provider_failover_notice: None,
        };
        log.notify_progress(snapshot.clone(), 5).await;
        match rx.recv().await {
            Ok(TaskEventLogMessage::Progress {
                snapshot: received,
                last_seq,
            }) => {
                assert_eq!(received.current_iteration, 3);
                assert_eq!(last_seq, 5);
            }
            other => panic!("expected Progress message, got {other:?}"),
        }
        let stored = log.last_progress_snapshot().await;
        assert_eq!(stored.unwrap().current_iteration, 3);
    }

    #[tokio::test]
    async fn closed_at_is_none_before_close_and_set_after_close() {
        let log = TaskEventLog::new();
        assert!(log.closed_at().await.is_none());
        assert!(!log.is_closed().await);
        log.close().await;
        assert!(log.closed_at().await.is_some());
        assert!(log.is_closed().await);
    }

    #[tokio::test]
    async fn close_is_idempotent_and_does_not_refresh_closed_at() {
        let log = TaskEventLog::new();
        log.close().await;
        let first = log.closed_at().await;
        // Sleep long enough that a re-close would record a measurably
        // later instant if it refreshed the timestamp.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        log.close().await;
        let second = log.closed_at().await;
        assert_eq!(first, second, "second close must not refresh closed_at");
    }
}
