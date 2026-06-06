//! SSE (Server-Sent Events) streaming for task event replay and live updates.

use super::{authenticated_user, load_owned_task, task_detail_from_record};
use super::{AppState, TaskEventsQuery};
use axum::{
    extract::{Path, Query, State},
    http::{header::HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        Json,
    },
};
use futures_util::stream::Stream;
use oxide_agent_web_contracts::{
    ErrorCode, ErrorEnvelope, PersistedTaskEvent, ProgressSnapshot, TaskDetail,
    TaskStatus as ApiTaskStatus, WebTaskRecord,
};
use serde::Serialize;
use std::convert::Infallible;
use std::time::Duration;

use super::{DEFAULT_TASK_EVENTS_LIMIT, MAX_TASK_EVENTS_LIMIT};

/// SSE streaming endpoint: replays persisted events then polls for new ones.
pub(crate) async fn api_sse_task_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Query(query): Query<TaskEventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    let stream_state = TaskSseStreamState {
        state,
        user_id: user.user_id,
        session_id,
        task_id,
        last_seq: sse_start_seq(&headers, &query),
        limit: query
            .limit
            .unwrap_or(DEFAULT_TASK_EVENTS_LIMIT)
            .clamp(1, MAX_TASK_EVENTS_LIMIT),
        task,
    };

    Ok(Sse::new(task_sse_stream(stream_state)))
}

struct TaskSseStreamState {
    state: AppState,
    user_id: i64,
    session_id: String,
    task_id: String,
    last_seq: u64,
    limit: usize,
    task: WebTaskRecord,
}

#[derive(Debug, Serialize)]
struct TaskSseSnapshot {
    task: TaskDetail,
    last_seq: u64,
}

#[derive(Debug, Serialize)]
struct TaskSseStatus {
    task_id: String,
    status: ApiTaskStatus,
    final_response_available: bool,
    last_seq: u64,
}

#[derive(Debug, Serialize)]
struct TaskSseProgress {
    task_id: String,
    progress: ProgressSnapshot,
}

#[derive(Debug, Serialize)]
struct TaskSseKeepalive {
    last_seq: u64,
}

#[derive(Debug, Serialize)]
struct TaskSseError {
    code: ErrorCode,
    message: String,
    retryable: bool,
}

fn task_sse_stream(
    mut stream_state: TaskSseStreamState,
) -> impl Stream<Item = Result<Event, Infallible>> {
    async_stream::stream! {
        let mut last_status = stream_state.task.status;
        let mut last_progress = stream_state.task.last_progress.clone();
        yield Ok(sse_json_event("snapshot", &TaskSseSnapshot {
            task: task_detail_from_record(stream_state.task.clone()),
            last_seq: stream_state.last_seq,
        }));

        loop {
            let batch = match sse_replay_batch(&mut stream_state).await {
                Ok(batch) => batch,
                Err(event) => {
                    yield Ok(event);
                    break;
                }
            };
            for event in batch.events {
                yield Ok(event);
            }

            match sse_reload_task(&stream_state).await {
                Ok(task) => {
                    if let Some(event) = progress_event_if_changed(
                        &mut last_progress,
                        &task,
                        &stream_state.task_id,
                    ) {
                        yield Ok(event);
                    }
                    let replay_tail_drained = !batch.has_more
                        && stream_state.last_seq >= task.last_event_seq;
                    let closing_status = status_closes_client_stream(task.status);
                    let should_emit_status = task.status != last_status
                        || (task.status.is_terminal() && replay_tail_drained);
                    if should_emit_status && (!closing_status || replay_tail_drained) {
                        yield Ok(sse_status_event(&task, stream_state.last_seq));
                        last_status = task.status;
                    }
                    if task.status.is_terminal() && replay_tail_drained {
                        break;
                    }
                }
                Err(event) => {
                    yield Ok(event);
                    break;
                }
            }

            if !batch.has_more {
                yield Ok(sse_json_event("keepalive", &TaskSseKeepalive {
                    last_seq: stream_state.last_seq,
                }));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

const fn status_closes_client_stream(status: ApiTaskStatus) -> bool {
    matches!(status, ApiTaskStatus::WaitingForUserInput) || status.is_terminal()
}

struct TaskSseBatch {
    events: Vec<Event>,
    has_more: bool,
}

async fn sse_replay_batch(stream_state: &mut TaskSseStreamState) -> Result<TaskSseBatch, Event> {
    let response = stream_state
        .state
        .web_store
        .list_task_events(
            stream_state.user_id,
            &stream_state.session_id,
            &stream_state.task_id,
            stream_state.last_seq,
            stream_state.limit,
        )
        .await
        .map_err(|error| {
            sse_error_event(
                ErrorCode::BackendUnavailable,
                format!("Failed to load task events: {error}"),
                true,
            )
        })?;

    let mut sse_events = Vec::with_capacity(response.events.len());
    for event in response.events {
        stream_state.last_seq = event.seq;
        sse_events.push(sse_persisted_task_event(&event));
    }
    Ok(TaskSseBatch {
        events: sse_events,
        has_more: response.has_more,
    })
}

async fn sse_reload_task(stream_state: &TaskSseStreamState) -> Result<WebTaskRecord, Event> {
    stream_state
        .state
        .web_store
        .load_task(
            stream_state.user_id,
            &stream_state.session_id,
            &stream_state.task_id,
        )
        .await
        .map_err(|error| {
            sse_error_event(
                ErrorCode::BackendUnavailable,
                format!("Failed to load task status: {error}"),
                true,
            )
        })?
        .ok_or_else(|| {
            sse_error_event(
                ErrorCode::NotFound,
                "Task is no longer available.".to_string(),
                false,
            )
        })
}

fn progress_event_if_changed(
    last_progress: &mut Option<ProgressSnapshot>,
    task: &WebTaskRecord,
    task_id: &str,
) -> Option<Event> {
    if task.last_progress == *last_progress {
        return None;
    }
    let event = task.last_progress.clone().map(|progress| {
        sse_json_event(
            "progress",
            &TaskSseProgress {
                task_id: task_id.to_string(),
                progress,
            },
        )
    });
    *last_progress = task.last_progress.clone();
    event
}

pub(crate) fn sse_start_seq(headers: &HeaderMap, query: &TaskEventsQuery) -> u64 {
    query
        .after_seq
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .unwrap_or_default()
}

fn sse_status_event(task: &WebTaskRecord, last_seq: u64) -> Event {
    sse_json_event(
        "task_status",
        &TaskSseStatus {
            task_id: task.task_id.clone(),
            status: task.status,
            final_response_available: task.final_response_markdown.is_some(),
            last_seq,
        },
    )
}

fn sse_persisted_task_event(event: &PersistedTaskEvent) -> Event {
    sse_json_event("task_event", event).id(event.seq.to_string())
}

fn sse_error_event(code: ErrorCode, message: String, retryable: bool) -> Event {
    sse_json_event(
        "error",
        &TaskSseError {
            code,
            message,
            retryable,
        },
    )
}

fn sse_json_event(name: &'static str, payload: &impl Serialize) -> Event {
    let data = serde_json::to_string(payload).unwrap_or_else(|error| {
        serde_json::json!({
            "code": "internal",
            "message": format!("Failed to serialize SSE payload: {error}"),
            "retryable": false,
        })
        .to_string()
    });
    Event::default().event(name).data(data)
}
