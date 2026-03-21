//! HTTP server for the web transport.
//!
//! ## Endpoints
//!
//! - `POST /sessions` — create session
//! - `GET /sessions/:id` — session metadata
//! - `DELETE /sessions/:id` — destroy session
//! - `POST /sessions/:session_id/tasks` — submit task (plain text body), returns `{task_id}`
//! - `GET /sessions/:session_id/tasks/:task_id/progress` — `SerializableProgress`
//! - `GET /sessions/:session_id/tasks/:task_id/events` — event log as JSON
//! - `GET /sessions/:session_id/tasks/:task_id/stream` — SSE event stream
//! - `GET /sessions/:session_id/tasks/:task_id/timeline` — `TaskTimeline`
//! - `POST /sessions/:session_id/tasks/:task_id/cancel` — cancel task
//! - `GET /health`

use crate::session::{SessionMeta, ToolCallTiming, WebSessionManager};
use crate::web_transport::collect_events;
use async_stream::stream as async_stream;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{delete, get, post},
    Json, Router,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap as StdHashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex as AsyncMutex, RwLock};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<WebSessionManager>,
    pub task_progress: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    pub task_timeline: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    /// Tracks the JoinHandle for each running task so it can be aborted on completion.
    pub task_handles: Arc<RwLock<StdHashMap<String, Arc<tokio::task::JoinHandle<()>>>>>,
}

impl AppState {
    pub fn new(session_manager: Arc<WebSessionManager>) -> Self {
        Self {
            session_manager,
            task_progress: Arc::new(RwLock::new(StdHashMap::new())),
            task_timeline: Arc::new(RwLock::new(StdHashMap::new())),
            task_handles: Arc::new(RwLock::new(StdHashMap::new())),
        }
    }

    /// Access the underlying session manager (for test use).
    #[must_use]
    pub fn session_manager(&self) -> Arc<WebSessionManager> {
        self.session_manager.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SerializableProgress {
    pub current_iteration: usize,
    pub max_iterations: usize,
    pub is_finished: bool,
    pub error: Option<String>,
    pub current_thought: Option<String>,
    pub narrative_headline: Option<String>,
    pub last_compaction_status: Option<String>,
    pub repeated_compaction_warning: Option<String>,
    pub latest_token_snapshot: Option<oxide_agent_core::agent::progress::TokenSnapshot>,
}

impl SerializableProgress {
    fn from_state(state: &oxide_agent_core::agent::progress::ProgressState) -> Self {
        Self {
            current_iteration: state.current_iteration,
            max_iterations: state.max_iterations,
            is_finished: state.is_finished,
            error: state.error.clone(),
            current_thought: state.current_thought.clone(),
            narrative_headline: state.narrative_headline.clone(),
            last_compaction_status: state.last_compaction_status.clone(),
            repeated_compaction_warning: state.repeated_compaction_warning.clone(),
            latest_token_snapshot: state.latest_token_snapshot.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Milestones {
    /// HTTP request received (legacy, kept for compatibility).
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
    pub tool_calls: Vec<ToolCallTiming>,
}

/// Re-exported from web_transport to avoid duplication.
pub use crate::web_transport::TaskEventEntry;

#[derive(Debug, Deserialize)]
pub struct CreateSessionBody {
    pub user_id: i64,
    #[serde(default)]
    pub context_key: Option<String>,
    #[serde(default)]
    pub agent_flow_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTaskResponse {
    pub task_id: String,
}

#[derive(Debug, Serialize)]
pub struct TaskTimelineResponse {
    pub task_id: String,
    pub session_id: String,
    pub milestones: Milestones,
    pub tool_calls: Vec<ToolCallTiming>,
}

// Global registry of event logs per task.
lazy_static::lazy_static! {
    pub static ref EVENT_LOGS: AsyncMutex<StdHashMap<String, crate::web_transport::TaskEventLog>> =
        AsyncMutex::new(StdHashMap::new());
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Debug endpoint: list all task IDs currently in EVENT_LOGS.
async fn debug_event_logs() -> Json<Vec<String>> {
    let logs = EVENT_LOGS.lock().await;
    Json(logs.keys().cloned().collect())
}

async fn create_session(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Result<Json<CreateSessionResponse>, StatusCode> {
    let session_id = state
        .session_manager
        .create_session(body.user_id, body.context_key, body.agent_flow_id)
        .await;
    Ok(Json(CreateSessionResponse { session_id }))
}

async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionMeta>, StatusCode> {
    let meta = state
        .session_manager
        .get_session(&session_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(meta))
}

async fn delete_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> StatusCode {
    if state.session_manager.delete_session(&session_id).await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn create_task(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: String,
) -> Result<Json<CreateTaskResponse>, StatusCode> {
    let Some(running_task) = state
        .session_manager
        .register_task(&session_id, body.clone())
        .await
    else {
        return Err(StatusCode::NOT_FOUND);
    };

    let task_id = running_task.task_id.clone();
    let session_id_clone = session_id.clone();
    let task_progress = state.task_progress.clone();
    let task_timeline = state.task_timeline.clone();
    let session_manager = state.session_manager.clone();
    let _http_received_at = Instant::now();

    // Initialize timeline and events entry.
    // Note: session_ready_ms is now set later, after executor lock is acquired.
    {
        let mut tl = task_timeline.write().await;
        tl.insert(
            task_id.clone(),
            TaskTimelineRecord {
                milestones: Milestones {
                    session_ready_ms: None, // Will be set after executor lock acquisition
                    executor_lock_acquired_ms: None,
                    prepare_execution_done_ms: None,
                    pre_run_compaction_done_ms: None,
                    thinking_sent_ms: None,
                    llm_call_started_ms: None,
                    first_tool_call_ms: None,
                    last_tool_call_ms: None,
                    first_tool_result_ms: None,
                    last_tool_result_ms: None,
                    first_thinking_ms: None,
                    final_response_ms: None,
                },
                tool_calls: Vec::new(),
            },
        );
    }
    // Register event log.
    {
        let mut logs = EVENT_LOGS.lock().await;
        logs.insert(task_id.clone(), running_task.event_log.clone());
    }

    let tid = task_id.clone();
    let ctx = TaskExecutorCtx {
        task_progress,
        task_timeline,
    };

    let task_handles = state.task_handles.clone();
    let tid_for_cleanup = tid.clone();

    // Spawn the task and store its JoinHandle so it can be aborted when done.
    let handle = tokio::spawn(async move {
        execute_agent_task(
            session_manager,
            &session_id_clone,
            &tid_for_cleanup,
            body,
            ctx,
        )
        .await;
        // Remove the JoinHandle from the registry when done (avoids leaking the handle).
        let mut handles = task_handles.write().await;
        handles.remove(&tid_for_cleanup);
    });

    // Register the JoinHandle so cancel_task can abort it.
    {
        let mut handles = state.task_handles.write().await;
        handles.insert(task_id.clone(), Arc::new(handle));
    }

    // Yield to let the Tokio runtime schedule the spawned task before returning.
    tokio::task::yield_now().await;

    Ok(Json(CreateTaskResponse { task_id }))
}

async fn get_task_progress(
    State(state): State<AppState>,
    Path((_session_id, task_id)): Path<(String, String)>,
) -> Result<Json<SerializableProgress>, StatusCode> {
    let progress_map = state.task_progress.read().await;
    progress_map
        .get(&task_id)
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn get_task_events(
    Path((_session_id, task_id)): Path<(String, String)>,
) -> Result<Json<Vec<TaskEventEntry>>, StatusCode> {
    let logs = EVENT_LOGS.lock().await;
    match logs.get(&task_id) {
        Some(log) => {
            let events = log.events.read().await;
            Ok(Json(events.clone()))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn get_task_timeline(
    State(state): State<AppState>,
    Path((session_id, task_id)): Path<(String, String)>,
) -> Result<Json<TaskTimelineResponse>, StatusCode> {
    let timelines = state.task_timeline.read().await;
    let milestones = timelines
        .get(&task_id)
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(TaskTimelineResponse {
        task_id,
        session_id,
        milestones: milestones.milestones,
        tool_calls: milestones.tool_calls,
    }))
}

async fn cancel_task(
    State(state): State<AppState>,
    Path((session_id, task_id)): Path<(String, String)>,
) -> StatusCode {
    if state
        .session_manager
        .cancel_task(&task_id, &session_id)
        .await
    {
        // Also abort the spawned task's JoinHandle if tracked.
        let handle = {
            let handles = state.task_handles.read().await;
            handles.get(&task_id).cloned()
        };
        if let Some(h) = handle {
            h.abort();
        }
        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}

/// SSE stream of task events.
///
/// Streams a snapshot of already-received events immediately, then listens for
/// new events via the broadcast channel. The stream closes after `max_duration` or
/// when the task completes (whichever comes first).
async fn sse_task_stream(
    Path((_session_id, task_id)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, StatusCode> {
    let task_id_str = task_id.clone();

    // Poll EVENT_LOGS briefly — the background task may not have registered yet.
    let event_log = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let logs = EVENT_LOGS.lock().await;
            if let Some(log) = logs.get(&task_id_str) {
                break log.clone();
            }
            drop(logs);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| StatusCode::NOT_FOUND)?;

    // Take a snapshot of events already received.
    let initial_events = event_log.snapshot().await;

    // Subscribe to new events AFTER snapshot (gets events pushed after this point).
    let mut rx = event_log.subscribe();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);

    // Combine snapshot + live broadcast into a single SSE stream using `stream!`.
    let stream = async_stream! {
        // First, emit the snapshot events.
        for entry in initial_events {
            yield Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event("task_event")
                    .data(serde_json::to_string(&entry).unwrap_or_default()),
            );
        }

        // Then listen for new events from the broadcast channel.
        // Keep listening until the sender is dropped (channel closed) OR the
        // event_log is closed (task done), which sends a "stream_closed" sentinel.
        loop {
            tokio::select! {
                biased; // Prefer recv over deadline

                // Receive a broadcast event.
                result = rx.recv() => {
                    match result {
                        Ok(entry) => {
                            if entry.event_name == "stream_closed" {
                                break;
                            }
                            let event = Event::default()
                                .event("task_event")
                                .data(serde_json::to_string(&entry).unwrap_or_default());
                            yield Ok(event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            // Channel closed — task is done.
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let event = Event::default()
                                .event("task_event")
                                .data(r#"{"event_name":"lagged"}"#);
                            yield Ok(event);
                        }
                    }
                }

                // Fallback: close stream after max duration.
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream))
}

// ---------------------------------------------------------------------------
// Task execution
// ---------------------------------------------------------------------------

/// Shared state needed by the task executor.
struct TaskExecutorCtx {
    task_progress: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    task_timeline: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
}

struct ExecutorTaskCtx {
    session_manager: Arc<WebSessionManager>,
    session_id: String,
    task_id: String,
    task_text: String,
    executor_arc: Arc<tokio::sync::RwLock<oxide_agent_core::agent::AgentExecutor>>,
    tx: mpsc::Sender<oxide_agent_core::agent::AgentEvent>,
    timeline_map: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    agent_started_at: Instant,
}

async fn execute_agent_task(
    session_manager: Arc<WebSessionManager>,
    session_id: &str,
    task_id: &str,
    task_text: String,
    ctx: TaskExecutorCtx,
) {
    let registry = session_manager.session_registry();
    let sid = derive_session_id(&session_manager, session_id).await;
    let Some(sid) = sid else {
        session_manager.fail_task(task_id, session_id).await;
        return;
    };

    // Record instant when agent execution starts - used as reference
    // for all latency milestones (NOT HTTP request time).
    let agent_started_at = Instant::now();

    let executor_arc = match registry.get(&sid).await {
        Some(e) => e,
        None => {
            session_manager.fail_task(task_id, session_id).await;
            return;
        }
    };

    // Capture chrono timestamp for calculating offsets from named milestones.
    let agent_started_at_chrono = chrono::Utc::now().timestamp_millis();

    // Get event log from global registry.
    let event_log = {
        let logs = EVENT_LOGS.lock().await;
        logs.get(task_id)
            .cloned()
            .unwrap_or_else(crate::web_transport::TaskEventLog::new)
    };

    let (tx, rx) = mpsc::channel::<oxide_agent_core::agent::AgentEvent>(100);

    let tid = task_id.to_string();
    spawn_event_collector(
        event_log,
        rx,
        ctx.task_progress.clone(),
        ctx.task_timeline.clone(),
        tid.clone(),
        agent_started_at_chrono,
    );
    spawn_executor_task(ExecutorTaskCtx {
        session_manager,
        session_id: session_id.to_string(),
        task_id: tid,
        task_text,
        executor_arc,
        tx,
        timeline_map: ctx.task_timeline.clone(),
        agent_started_at,
    });
}

fn spawn_event_collector(
    event_log: crate::web_transport::TaskEventLog,
    rx: mpsc::Receiver<oxide_agent_core::agent::AgentEvent>,
    progress_map: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    timeline_map: Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    task_id: String,
    agent_started_at_ms: i64,
) {
    tokio::spawn(async move {
        let collected = collect_events(event_log, rx).await;
        let progress = SerializableProgress::from_state(&collected.state);

        {
            let mut pm = progress_map.write().await;
            pm.insert(task_id.clone(), progress);
        }

        let mut tl = timeline_map.write().await;
        if let Some(record) = tl.get_mut(&task_id) {
            apply_event_collection(record, &collected, agent_started_at_ms);
        }
    });
}

fn spawn_executor_task(ctx: ExecutorTaskCtx) {
    tokio::spawn(async move {
        let ExecutorTaskCtx {
            session_manager,
            session_id,
            task_id,
            task_text,
            executor_arc,
            tx,
            timeline_map,
            agent_started_at,
        } = ctx;

        let result = {
            let mut executor = executor_arc.write().await;
            let executor_lock_acquired_ms = Some(agent_started_at.elapsed().as_millis() as i64);
            record_executor_lock_acquired(&timeline_map, &task_id, executor_lock_acquired_ms).await;
            executor.execute(&task_text, Some(tx)).await
        };

        match result {
            Ok(_) => {
                session_manager.complete_task(&task_id, &session_id).await;
                info!(task_id = %task_id, "Task completed");
            }
            Err(e) => {
                session_manager.fail_task(&task_id, &session_id).await;
                info!(task_id = %task_id, error = %e, "Task failed");
            }
        }
    });
}

async fn record_executor_lock_acquired(
    timeline_map: &Arc<RwLock<StdHashMap<String, TaskTimelineRecord>>>,
    task_id: &str,
    executor_lock_acquired_ms: Option<i64>,
) {
    let mut tl = timeline_map.write().await;
    if let Some(record) = tl.get_mut(task_id) {
        record.milestones.executor_lock_acquired_ms = executor_lock_acquired_ms;
        record.milestones.session_ready_ms = executor_lock_acquired_ms;
    }
}

fn apply_event_collection(
    record: &mut TaskTimelineRecord,
    collected: &crate::web_transport::EventCollectionResult,
    agent_started_at_ms: i64,
) {
    let tool_calls = collected
        .tool_calls
        .iter()
        .map(|timing| ToolCallTiming {
            name: timing.name.clone(),
            started_at_ms: relative_timestamp_ms(agent_started_at_ms, timing.started_at),
            finished_at_ms: timing
                .finished_at
                .map(|finished_at| relative_timestamp_ms(agent_started_at_ms, finished_at)),
        })
        .collect::<Vec<_>>();

    record.tool_calls = tool_calls;

    // Derive llm_call_started_ms from the collector-side clock (first Thinking or
    // Reasoning event). This keeps it in the same time domain as first_tool_call_ms
    // and makes monotonicity assertions meaningful.
    let llm_started_at = earliest_of(
        collected.timestamps.first_thinking_at,
        collected.timestamps.first_reasoning_at,
    );
    record.milestones.llm_call_started_ms =
        llm_started_at.map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));

    record.milestones.first_thinking_ms = collected
        .timestamps
        .first_thinking_at
        .map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));
    record.milestones.final_response_ms = collected
        .timestamps
        .finished_at
        .map(|ts| relative_timestamp_ms(agent_started_at_ms, ts));
    record.milestones.first_tool_call_ms = record
        .tool_calls
        .iter()
        .map(|timing| timing.started_at_ms)
        .min();
    record.milestones.last_tool_call_ms = record
        .tool_calls
        .iter()
        .map(|timing| timing.started_at_ms)
        .max();
    record.milestones.first_tool_result_ms = record
        .tool_calls
        .iter()
        .filter_map(|timing| timing.finished_at_ms)
        .min();
    record.milestones.last_tool_result_ms = record
        .tool_calls
        .iter()
        .filter_map(|timing| timing.finished_at_ms)
        .max();

    // Named milestones that use the agent's own Unix timestamps.
    // Note: "llm_call_started" is intentionally NOT applied here — it is
    // already derived from the collector-side clock above.
    for (name, ts) in &collected.timestamps.named_milestones {
        let ms = Some(ts.timestamp_millis() - agent_started_at_ms);
        match name.as_str() {
            "prepare_execution_done" => record.milestones.prepare_execution_done_ms = ms,
            "pre_run_compaction_done" => record.milestones.pre_run_compaction_done_ms = ms,
            "thinking_sent" => record.milestones.thinking_sent_ms = ms,
            _ => {}
        }
    }
}

fn earliest_of(
    a: Option<chrono::DateTime<chrono::Utc>>,
    b: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    match (a, b) {
        (Some(ts), None) => Some(ts),
        (None, Some(ts)) => Some(ts),
        (Some(a), Some(b)) => Some(a.min(b)),
        (None, None) => None,
    }
}

fn relative_timestamp_ms(
    agent_started_at_ms: i64,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> i64 {
    timestamp.timestamp_millis() - agent_started_at_ms
}

async fn derive_session_id(
    session_manager: &WebSessionManager,
    session_id: &str,
) -> Option<oxide_agent_core::agent::SessionId> {
    let meta = session_manager.get_session(session_id).await?;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    meta.user_id.hash(&mut h);
    Some(oxide_agent_core::agent::SessionId::from(h.finish() as i64))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::permissive();

    Router::new()
        .route("/health", get(health))
        .route("/debug/event_logs", get(debug_event_logs))
        .route("/sessions", post(create_session))
        .route("/sessions/:id", get(get_session))
        .route("/sessions/:id", delete(delete_session))
        .route("/sessions/:session_id/tasks", post(create_task))
        .route(
            "/sessions/:session_id/tasks/:task_id/progress",
            get(get_task_progress),
        )
        .route(
            "/sessions/:session_id/tasks/:task_id/events",
            get(get_task_events),
        )
        .route(
            "/sessions/:session_id/tasks/:task_id/stream",
            get(sse_task_stream),
        )
        .route(
            "/sessions/:session_id/tasks/:task_id/timeline",
            get(get_task_timeline),
        )
        .route(
            "/sessions/:session_id/tasks/:task_id/cancel",
            post(cancel_task),
        )
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}

pub async fn serve(state: AppState, addr: std::net::SocketAddr) {
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind TCP listener");
    tracing::info!("Web transport listening on {addr}");
    axum::serve(listener, router).await.expect("server error");
}
