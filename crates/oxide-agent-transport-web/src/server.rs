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
//! - `GET /sessions/:session_id/tasks/:task_id/timeline` — `TaskTimeline`
//! - `POST /sessions/:session_id/tasks/:task_id/cancel` — cancel task
//! - `GET /health`

use crate::session::{SessionMeta, WebSessionManager};
use crate::web_transport::collect_events;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
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
    pub task_timeline: Arc<RwLock<StdHashMap<String, Milestones>>>,
}

impl AppState {
    pub fn new(session_manager: Arc<WebSessionManager>) -> Self {
        Self {
            session_manager,
            task_progress: Arc::new(RwLock::new(StdHashMap::new())),
            task_timeline: Arc::new(RwLock::new(StdHashMap::new())),
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
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Milestones {
    pub session_ready_ms: Option<i64>,
    pub first_thinking_ms: Option<i64>,
    pub final_response_ms: Option<i64>,
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
}

// Global registry of event logs per task.
lazy_static::lazy_static! {
    static ref EVENT_LOGS: AsyncMutex<StdHashMap<String, crate::web_transport::TaskEventLog>> =
        AsyncMutex::new(StdHashMap::new());
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
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
    let started_at = Instant::now();

    // Initialize timeline and events entry.
    {
        let mut tl = task_timeline.write().await;
        tl.insert(
            task_id.clone(),
            Milestones {
                session_ready_ms: Some(started_at.elapsed().as_millis() as i64),
                first_thinking_ms: None,
                final_response_ms: None,
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
        started_at,
    };
    tokio::spawn(async move {
        execute_agent_task(&session_manager, &session_id_clone, &tid, body, ctx).await;
    });

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
        milestones,
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
        StatusCode::ACCEPTED
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// Task execution
// ---------------------------------------------------------------------------

/// Shared state needed by the task executor.
struct TaskExecutorCtx {
    task_progress: Arc<RwLock<StdHashMap<String, SerializableProgress>>>,
    task_timeline: Arc<RwLock<StdHashMap<String, Milestones>>>,
    started_at: Instant,
}

async fn execute_agent_task(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    task_text: String,
    ctx: TaskExecutorCtx,
) {
    let registry = session_manager.session_registry();
    let sid = derive_session_id(session_manager, session_id).await;
    let Some(sid) = sid else {
        session_manager.fail_task(task_id, session_id).await;
        return;
    };

    let executor_arc = match registry.get(&sid).await {
        Some(e) => e,
        None => {
            session_manager.fail_task(task_id, session_id).await;
            return;
        }
    };

    // Get event log from global registry.
    let event_log = {
        let logs = EVENT_LOGS.lock().await;
        logs.get(task_id)
            .cloned()
            .unwrap_or_else(crate::web_transport::TaskEventLog::new)
    };

    let (tx, rx) = mpsc::channel::<oxide_agent_core::agent::AgentEvent>(100);
    let progress_map = ctx.task_progress.clone();
    let tl_map = ctx.task_timeline.clone();
    let tid = task_id.to_string();
    let start = ctx.started_at;

    tokio::spawn(async move {
        let state = collect_events(event_log, rx).await;
        let progress = SerializableProgress::from_state(&state);
        {
            let mut pm = progress_map.write().await;
            pm.insert(tid.clone(), progress);
        }
        let mut tl = tl_map.write().await;
        if let Some(m) = tl.get_mut(&tid) {
            m.final_response_ms = Some(start.elapsed().as_millis() as i64);
        }
    });

    let result = {
        let mut executor = executor_arc.write().await;
        executor.execute(&task_text, Some(tx)).await
    };

    // Cleanup event log.
    {
        let mut logs = EVENT_LOGS.lock().await;
        logs.remove(task_id);
    }

    match result {
        Ok(_) => {
            session_manager.complete_task(task_id, session_id).await;
            info!(task_id, "Task completed");
        }
        Err(e) => {
            session_manager.fail_task(task_id, session_id).await;
            info!(task_id, error = %e, "Task failed");
        }
    }
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
