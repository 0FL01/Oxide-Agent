#![deny(missing_docs)]
//! Read-only web monitor transport for task snapshots and live events.

use async_stream::stream;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use oxide_agent_core::agent::{TaskEvent, TaskId, TaskSnapshot};
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_runtime::{
    ObserverAccessRegistry, ObserverAccessResolveError, ObserverAccessToken, TaskEventBroadcaster,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::error;

/// Options required to start the web monitor server.
pub struct WebMonitorServerOptions {
    /// Socket address to bind HTTP listener.
    pub bind_addr: SocketAddr,
    /// Shared storage backend for task snapshots and persisted events.
    pub storage: Arc<dyn StorageProvider>,
    /// Shared runtime broadcaster used for replay and live fan-out.
    pub task_events: Arc<TaskEventBroadcaster>,
    /// Shared token registry for observer-only auth.
    pub observer_access: Arc<ObserverAccessRegistry>,
}

/// Running web monitor server handle.
pub struct WebMonitorServerHandle {
    local_addr: SocketAddr,
    join_handle: JoinHandle<()>,
}

impl WebMonitorServerHandle {
    /// Returns actual listener address.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for WebMonitorServerHandle {
    fn drop(&mut self) {
        self.join_handle.abort();
    }
}

/// Errors emitted by web monitor server bootstrap.
#[derive(Debug, Error)]
pub enum WebMonitorServerError {
    /// Failed to bind socket.
    #[error("failed to bind web monitor listener: {0}")]
    Bind(std::io::Error),
}

/// Start web monitor server and return running handle.
pub async fn spawn_web_monitor(
    options: WebMonitorServerOptions,
) -> Result<WebMonitorServerHandle, WebMonitorServerError> {
    let listener = TcpListener::bind(options.bind_addr)
        .await
        .map_err(WebMonitorServerError::Bind)?;
    let local_addr = listener.local_addr().map_err(WebMonitorServerError::Bind)?;

    let app = build_router(WebMonitorAppState {
        storage: options.storage,
        task_events: options.task_events,
        observer_access: options.observer_access,
    });

    let join_handle = tokio::spawn(async move {
        if let Err(serve_error) = axum::serve(listener, app).await {
            error!(error = %serve_error, "web monitor server exited with error");
        }
    });

    Ok(WebMonitorServerHandle {
        local_addr,
        join_handle,
    })
}

#[derive(Clone)]
struct WebMonitorAppState {
    storage: Arc<dyn StorageProvider>,
    task_events: Arc<TaskEventBroadcaster>,
    observer_access: Arc<ObserverAccessRegistry>,
}

fn build_router(state: WebMonitorAppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/watch/{token}", get(watch_page))
        .route("/api/observer/{token}/snapshot", get(observer_snapshot))
        .route("/api/observer/{token}/events", get(observer_events))
        .with_state(state)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[derive(Serialize)]
struct SnapshotResponse {
    task_id: String,
    snapshot: Option<TaskSnapshot>,
    replay_events: Vec<TaskEvent>,
}

#[derive(Deserialize)]
struct EventsQuery {
    after: Option<u64>,
}

async fn watch_page(
    State(state): State<WebMonitorAppState>,
    Path(token): Path<String>,
) -> Result<Response, ApiError> {
    let task_id = authorize_token(&state, &token).await?;
    let safe_token = serde_json::to_string(&token).map_err(ApiError::Serialize)?;
    let safe_task_id = serde_json::to_string(&task_id.to_string()).map_err(ApiError::Serialize)?;

    let html = format!(
        "<!doctype html>
<html lang=\"en\">
  <head>
    <meta charset=\"utf-8\" />
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
    <title>Oxide Task Watch</title>
    <style>
      :root {{
        --bg: #f4f7fb;
        --card: #ffffff;
        --text: #1f2937;
        --muted: #6b7280;
        --accent: #0f766e;
        --border: #dbe3ee;
      }}
      body {{ margin: 0; font-family: \"IBM Plex Sans\", \"Segoe UI\", sans-serif; background: radial-gradient(circle at 20% 0%, #e8f6f4, var(--bg)); color: var(--text); }}
      main {{ max-width: 900px; margin: 2rem auto; padding: 0 1rem; }}
      .card {{ background: var(--card); border: 1px solid var(--border); border-radius: 14px; padding: 1rem; box-shadow: 0 10px 30px rgba(14, 42, 64, 0.06); }}
      h1 {{ margin-top: 0; margin-bottom: 0.25rem; font-weight: 650; }}
      .meta {{ color: var(--muted); margin-bottom: 0.75rem; font-size: 0.95rem; }}
      pre {{ background: #0b1320; color: #d7e2f0; border-radius: 10px; padding: 0.75rem; overflow-x: auto; max-height: 16rem; }}
      .events {{ margin-top: 1rem; display: grid; gap: 0.5rem; }}
      .event {{ border: 1px solid var(--border); border-radius: 10px; padding: 0.65rem; background: #fbfcfe; }}
      .state {{ font-weight: 600; color: var(--accent); }}
    </style>
  </head>
  <body>
    <main>
      <section class=\"card\">
        <h1>Task Watch</h1>
        <div class=\"meta\">Task <code id=\"task-id\"></code></div>
        <div class=\"meta\" id=\"stream-status\">Loading snapshot...</div>
        <pre id=\"snapshot\">Loading...</pre>
      </section>
      <section class=\"events\" id=\"events\"></section>
    </main>
    <script>
      const token = {safe_token};
      const taskId = {safe_task_id};
      const streamStatus = document.getElementById('stream-status');
      const snapshotNode = document.getElementById('snapshot');
      const eventsNode = document.getElementById('events');
      document.getElementById('task-id').textContent = taskId;

      const terminalStates = new Set(['completed', 'failed', 'cancelled', 'stopped']);
      const isTerminalState = (state) => terminalStates.has(state);

      const renderEvent = (event) => {{
        const box = document.createElement('article');
        box.className = 'event';
        box.innerHTML = `<div><span class=\"state\">${{event.state}}</span> seq=${{event.sequence}}</div><div>${{new Date(event.occurred_at).toLocaleString()}}</div>`;
        eventsNode.prepend(box);
      }};

      fetch(`/api/observer/${{token}}/snapshot`)
        .then((response) => response.ok ? response.json() : Promise.reject(response.status))
        .then((payload) => {{
          snapshotNode.textContent = JSON.stringify(payload.snapshot ?? null, null, 2);
          payload.replay_events.forEach(renderEvent);
          const last = payload.replay_events.at(-1);
          const finalState = last?.state ?? payload.snapshot?.metadata?.state ?? null;
          if (isTerminalState(finalState)) {{
            streamStatus.textContent = 'Task reached terminal state';
            return;
          }}
          const after = last ? `?after=${{last.sequence}}` : '';
          const source = new EventSource(`/api/observer/${{token}}/events${{after}}`);
          streamStatus.textContent = 'Live stream connected';
          source.addEventListener('task_event', (msg) => {{
            const event = JSON.parse(msg.data);
            renderEvent(event);
            if (isTerminalState(event.state)) {{
              streamStatus.textContent = 'Task reached terminal state';
              source.close();
            }}
          }});
          source.addEventListener('stream_notice', (msg) => {{
            streamStatus.textContent = msg.data;
          }});
          source.onerror = () => {{
            streamStatus.textContent = 'Live stream disconnected';
          }};
        }})
        .catch(() => {{
          streamStatus.textContent = 'Snapshot request failed';
          snapshotNode.textContent = 'Unable to load snapshot';
        }});
    </script>
  </body>
</html>"
    );

    Ok(with_no_store(Html(html).into_response()))
}

async fn observer_snapshot(
    State(state): State<WebMonitorAppState>,
    Path(token): Path<String>,
) -> Result<Response, ApiError> {
    let task_id = authorize_token(&state, &token).await?;
    let snapshot = state
        .storage
        .load_task_snapshot(task_id)
        .await
        .map_err(ApiError::Storage)?;
    let replay_events = state
        .storage
        .load_task_events(task_id)
        .await
        .map_err(ApiError::Storage)?;

    Ok(with_no_store(
        Json(SnapshotResponse {
            task_id: task_id.to_string(),
            snapshot,
            replay_events,
        })
        .into_response(),
    ))
}

async fn observer_events(
    State(state): State<WebMonitorAppState>,
    Path(token): Path<String>,
    Query(query): Query<EventsQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let task_id = authorize_token(&state, &token).await?;
    let resume_after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let after = match (query.after, resume_after) {
        (Some(from_query), Some(from_header)) => Some(from_query.max(from_header)),
        (Some(from_query), None) => Some(from_query),
        (None, Some(from_header)) => Some(from_header),
        (None, None) => None,
    };
    let subscription = state
        .task_events
        .subscribe(task_id, after)
        .await
        .map_err(ApiError::Storage)?;

    let event_stream = stream! {
        for replay_event in subscription.replay_events {
            let terminal = replay_event.state.is_terminal();
            yield Ok::<Event, Infallible>(encode_task_event(&replay_event));
            if terminal {
                return;
            }
        }

        if let Some(mut receiver) = subscription.live_receiver {
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        let terminal = event.state.is_terminal();
                        yield Ok::<Event, Infallible>(encode_task_event(&event));
                        if terminal {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        yield Ok::<Event, Infallible>(
                            Event::default()
                                .event("stream_notice")
                                .data(format!("lagged_by={skipped}")),
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        }
    };

    Ok(with_no_store(
        Sse::new(event_stream)
            .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
            .into_response(),
    ))
}

fn encode_task_event(event: &TaskEvent) -> Event {
    match serde_json::to_string(event) {
        Ok(payload) => Event::default()
            .id(event.sequence.to_string())
            .event("task_event")
            .data(payload),
        Err(error) => Event::default()
            .event("stream_notice")
            .data(format!("event_serialization_failed={error}")),
    }
}

async fn authorize_token(state: &WebMonitorAppState, raw_token: &str) -> Result<TaskId, ApiError> {
    let token = ObserverAccessToken::from_secret(raw_token.to_string());
    let grant = state
        .observer_access
        .resolve(&token)
        .await
        .map_err(ApiError::ResolveAccess)?;
    Ok(grant.task_id)
}

#[derive(Debug, Error)]
enum ApiError {
    #[error("task data unavailable")]
    Storage(oxide_agent_core::storage::StorageError),
    #[error("serialization error")]
    Serialize(serde_json::Error),
    #[error("observer access resolution failed")]
    ResolveAccess(ObserverAccessResolveError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = match self {
            Self::Storage(_) | Self::Serialize(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            }
            Self::ResolveAccess(ObserverAccessResolveError::InvalidToken) => {
                (StatusCode::UNAUTHORIZED, "invalid_token")
            }
            Self::ResolveAccess(
                ObserverAccessResolveError::ExpiredToken | ObserverAccessResolveError::RevokedToken,
            ) => (StatusCode::FORBIDDEN, "inactive_token"),
        };

        let body = Json(ErrorResponse {
            code,
            message: self.to_string(),
        });

        (status, [(header::CACHE_CONTROL, "no-store")], body).into_response()
    }
}

fn with_no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Serialize)]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::{build_router, WebMonitorAppState};
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::header;
    use axum::http::Request;
    use axum::http::StatusCode;
    use oxide_agent_core::agent::{
        AgentMemory, SessionId, TaskCheckpoint, TaskEvent, TaskEventKind, TaskMetadata,
        TaskSnapshot, TaskState,
    };
    use oxide_agent_core::storage::{
        Message, PendingInputPoll, StorageError, StorageProvider, UserConfig,
    };
    use oxide_agent_runtime::{
        ObserverAccessRegistry, ObserverAccessRegistryOptions, TaskEventBroadcaster,
        TaskEventBroadcasterOptions,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tower::util::ServiceExt;

    #[derive(Default)]
    struct TestStorage {
        snapshots: Mutex<HashMap<oxide_agent_core::agent::TaskId, TaskSnapshot>>,
        events: Mutex<HashMap<oxide_agent_core::agent::TaskId, Vec<TaskEvent>>>,
    }

    #[async_trait]
    impl StorageProvider for TestStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _system_prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model_name: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn list_task_snapshots(&self) -> Result<Vec<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.values().cloned().collect())
        }

        async fn append_task_event(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
            event: TaskEvent,
        ) -> Result<(), StorageError> {
            self.events
                .lock()
                .await
                .entry(task_id)
                .or_default()
                .push(event);
            Ok(())
        }

        async fn load_task_events(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Vec<TaskEvent>, StorageError> {
            Ok(self
                .events
                .lock()
                .await
                .get(&task_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn save_pending_input_poll(
            &self,
            _poll: &PendingInputPoll,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_pending_input_poll_by_id(
            &self,
            _poll: &PendingInputPoll,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_pending_input_poll_by_task(
            &self,
            _task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Option<PendingInputPoll>, StorageError> {
            Ok(None)
        }

        async fn load_pending_input_poll_by_id(
            &self,
            _poll_id: &str,
        ) -> Result<Option<PendingInputPoll>, StorageError> {
            Ok(None)
        }

        async fn delete_pending_input_poll(
            &self,
            _task_id: oxide_agent_core::agent::TaskId,
            _poll_id: &str,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn task_event(
        task_id: oxide_agent_core::agent::TaskId,
        sequence: u64,
        state: TaskState,
    ) -> TaskEvent {
        let kind = if sequence == 1 {
            TaskEventKind::Created
        } else {
            TaskEventKind::StateChanged
        };
        TaskEvent::new(task_id, sequence, kind, state, None)
    }

    fn task_snapshot(task_id: oxide_agent_core::agent::TaskId, state: TaskState) -> TaskSnapshot {
        let mut metadata = TaskMetadata::new();
        metadata.id = task_id;
        metadata.state = state;
        let checkpoint = TaskCheckpoint::new(state, 2);
        metadata.updated_at = checkpoint.persisted_at;

        TaskSnapshot {
            schema_version: oxide_agent_core::agent::task::TASK_SNAPSHOT_SCHEMA_VERSION,
            metadata,
            session_id: Some(SessionId::from(1)),
            task: "watch".to_string(),
            checkpoint,
            recovery_note: None,
            pending_input: None,
            agent_memory: None,
            stop_report: None,
        }
    }

    async fn build_app_with_token(
        storage: Arc<TestStorage>,
    ) -> (
        axum::Router,
        oxide_agent_runtime::ObserverAccessToken,
        TaskMetadata,
    ) {
        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));
        let observer_access = Arc::new(ObserverAccessRegistry::new(
            ObserverAccessRegistryOptions::new(),
        ));
        let metadata = TaskMetadata::new();
        let issue = observer_access.issue(metadata.id).await;
        assert!(issue.is_ok());
        let (token, _) = match issue {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };

        let app = build_router(WebMonitorAppState {
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_events: broadcaster,
            observer_access,
        });

        (app, token, metadata)
    }

    #[tokio::test]
    async fn invalid_token_returns_unauthorized_for_snapshot() {
        let storage = Arc::new(TestStorage::default());
        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));
        let observer_access = Arc::new(ObserverAccessRegistry::new(
            ObserverAccessRegistryOptions::new(),
        ));

        let app = build_router(WebMonitorAppState {
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_events: broadcaster,
            observer_access,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/observer/oa_invalid/snapshot")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn revoked_token_returns_forbidden_for_snapshot() {
        let storage = Arc::new(TestStorage::default());
        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));
        let observer_access = Arc::new(ObserverAccessRegistry::new(
            ObserverAccessRegistryOptions::new(),
        ));

        let task_id = TaskMetadata::new().id;
        let issue = observer_access.issue(task_id).await;
        assert!(issue.is_ok());
        let (token, _) = match issue {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };
        assert!(observer_access.revoke(&token).await);

        let app = build_router(WebMonitorAppState {
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_events: broadcaster,
            observer_access,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/observer/{}/snapshot", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn events_endpoint_replays_terminal_event_and_finishes_stream() {
        let storage = Arc::new(TestStorage::default());
        let task_id = TaskMetadata::new().id;
        let snapshot = task_snapshot(task_id, TaskState::Stopped);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 1, TaskState::Pending))
            .await
            .is_ok());
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 2, TaskState::Stopped))
            .await
            .is_ok());

        let broadcaster = Arc::new(TaskEventBroadcaster::new(TaskEventBroadcasterOptions::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
        )));
        let observer_access = Arc::new(ObserverAccessRegistry::new(
            ObserverAccessRegistryOptions::new(),
        ));
        let issue = observer_access.issue(task_id).await;
        assert!(issue.is_ok());
        let (token, _) = match issue {
            Ok(value) => value,
            Err(error) => panic!("failed to issue token: {error}"),
        };

        let app = build_router(WebMonitorAppState {
            storage: Arc::clone(&storage) as Arc<dyn StorageProvider>,
            task_events: Arc::clone(&broadcaster),
            observer_access,
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/observer/{}/events", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers().get("content-type");
        assert!(content_type.is_some());
        let content_type = content_type
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/event-stream"));

        let body = to_bytes(response.into_body(), usize::MAX).await;
        assert!(body.is_ok());
        let body = match body {
            Ok(bytes) => String::from_utf8(bytes.to_vec())
                .unwrap_or_else(|error| panic!("utf8 decode failed: {error}")),
            Err(error) => panic!("read body failed: {error}"),
        };

        assert!(body.contains("task_event"));
        assert!(body.contains("\"sequence\":2"));
        assert!(body.contains("\"state\":\"stopped\""));
    }

    #[tokio::test]
    async fn success_responses_set_no_store_cache_header() {
        let storage = Arc::new(TestStorage::default());
        let (app, token, metadata) = build_app_with_token(Arc::clone(&storage)).await;
        let snapshot = task_snapshot(metadata.id, TaskState::Running);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let watch = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/watch/{}", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));
        assert_eq!(watch.status(), StatusCode::OK);
        assert_eq!(
            watch.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static("no-store"))
        );

        let snapshot_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/observer/{}/snapshot", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));
        assert_eq!(snapshot_response.status(), StatusCode::OK);
        assert_eq!(
            snapshot_response.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static("no-store"))
        );

        let events = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/observer/{}/events", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));
        assert_eq!(events.status(), StatusCode::OK);
        assert_eq!(
            events.headers().get(header::CACHE_CONTROL),
            Some(&header::HeaderValue::from_static("no-store"))
        );
    }

    #[tokio::test]
    async fn events_resume_from_last_event_id_without_duplicates() {
        let storage = Arc::new(TestStorage::default());
        let (app, token, metadata) = build_app_with_token(Arc::clone(&storage)).await;
        let task_id = metadata.id;
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 1, TaskState::Pending))
            .await
            .is_ok());
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 2, TaskState::Running))
            .await
            .is_ok());
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 3, TaskState::Stopped))
            .await
            .is_ok());

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/observer/{}/events?after=1", token.secret()))
                    .header("last-event-id", "2")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await;
        assert!(body.is_ok());
        let body = match body {
            Ok(bytes) => String::from_utf8(bytes.to_vec())
                .unwrap_or_else(|error| panic!("utf8 decode failed: {error}")),
            Err(error) => panic!("read body failed: {error}"),
        };

        assert!(body.contains("\"sequence\":3"));
        assert!(!body.contains("\"sequence\":2"));
    }

    #[tokio::test]
    async fn watch_page_script_skips_live_stream_for_terminal_replay() {
        let storage = Arc::new(TestStorage::default());
        let (app, token, metadata) = build_app_with_token(Arc::clone(&storage)).await;
        let task_id = metadata.id;
        let snapshot = task_snapshot(task_id, TaskState::Stopped);
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());
        assert!(storage
            .append_task_event(task_id, task_event(task_id, 1, TaskState::Stopped))
            .await
            .is_ok());

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/watch/{}", token.secret()))
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request build failed: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("router request failed: {error}"));

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await;
        assert!(body.is_ok());
        let body = match body {
            Ok(bytes) => String::from_utf8(bytes.to_vec())
                .unwrap_or_else(|error| panic!("utf8 decode failed: {error}")),
            Err(error) => panic!("read body failed: {error}"),
        };

        assert!(body.contains(
            "const finalState = last?.state ?? payload.snapshot?.metadata?.state ?? null;"
        ));
        assert!(body.contains("if (isTerminalState(finalState))"));
    }
}
