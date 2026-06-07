//! SSE (Server-Sent Events) streaming for task event replay and live updates.
//!
//! Delivery model: on connect, the handler replays any events the client
//! missed (`after_seq` / `Last-Event-Id`) — from the in-memory `TaskEventLog`
//! snapshot when the task is still active in this process, otherwise from
//! Postgres. After replay, the handler subscribes to the in-process broadcast
//! channel on `TaskEventLog` and yields events as they are produced, with no
//! steady-state DB polling. Postgres remains the source of truth for
//! reconnect, post-restart catch-up, and slow-consumer overflow.

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
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

use super::{DEFAULT_TASK_EVENTS_LIMIT, EVENT_LOGS, MAX_TASK_EVENTS_LIMIT};
use crate::web_transport::TaskEventLogMessage;

/// Keepalive cadence for the broadcast-driven SSE stream. Long enough that
/// it is not a busy loop, short enough to keep proxies and load balancers
/// from closing the connection.
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// SSE streaming endpoint: replays persisted events then live-delivers new
/// ones via the in-process `TaskEventLog` broadcast.
pub(crate) async fn api_sse_task_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, task_id)): Path<(String, String)>,
    Query(query): Query<TaskEventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, (StatusCode, Json<ErrorEnvelope>)> {
    let user = authenticated_user(&state, &headers).await?;
    let task = load_owned_task(&state, user.user_id, &session_id, &task_id).await?;
    let event_log = {
        let logs = EVENT_LOGS.lock().await;
        logs.get(&task_id).cloned()
    };
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
        event_log,
    };
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = stream_state.user_id,
        session_id = %stream_state.session_id,
        task_id = %stream_state.task_id,
        after_seq = stream_state.last_seq,
        limit = stream_state.limit,
        task_status = ?stream_state.task.status,
        task_last_event_seq = stream_state.task.last_event_seq,
        in_memory_log = stream_state.event_log.is_some(),
        "web sse stream opened"
    );

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
    /// In-process event log if the task is active in this server. Used as a
    /// fast replay source and as the live broadcast source.
    event_log: Option<crate::web_transport::TaskEventLog>,
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
        let mut last_progress = stream_state.task.last_progress.clone();
        yield Ok(sse_json_event("snapshot", &TaskSseSnapshot {
            task: task_detail_from_record(stream_state.task.clone()),
            last_seq: stream_state.last_seq,
        }));

        // Reload current task state once for an authoritative status and
        // progress snapshot. This is the only DB call on the steady-state
        // path; live updates afterwards are broadcast-driven. The status
        // is emitted unconditionally on connect so a client that opens the
        // stream after a long task is finished still sees the terminal
        // state.
        let last_status = match sse_reload_task(&stream_state).await {
            Ok(task) => {
                if let Some(event) = progress_event_if_changed(
                    &mut last_progress,
                    &task,
                    &stream_state.task_id,
                ) {
                    yield Ok(event);
                }
                yield Ok(sse_status_event(&task, stream_state.last_seq));
                stream_state.task = task;
                stream_state.task.status
            }
            Err(event) => {
                yield Ok(event);
                return;
            }
        };

        // Capture the latest in-memory seq once; the snapshot is monotonic
        // for the duration of this stream.
        let log_latest_seq = match stream_state.event_log.as_ref() {
            Some(log) => log.latest_seq().await,
            None => 0,
        };

        // Replay any events the client missed. Use the in-memory snapshot
        // when it covers the requested gap; otherwise paginate from DB.
        if stream_state.last_seq < stream_state.task.last_event_seq {
            // Drain the in-memory snapshot first if it covers the gap.
            if let Some(log) = stream_state.event_log.as_ref()
                && log_latest_seq >= stream_state.last_seq {
                    let snapshot = log.persisted_snapshot().await;
                    for event in snapshot {
                        if event.seq <= stream_state.last_seq {
                            continue;
                        }
                        if event.seq > stream_state.task.last_event_seq {
                            break;
                        }
                        stream_state.last_seq = event.seq;
                        yield Ok(sse_persisted_task_event(&event));
                    }
                }

            // Continue with DB pages if the task persisted more events than
            // the in-memory log holds (e.g. task already closed or restarted
            // since the last in-memory write).
            while stream_state.last_seq < stream_state.task.last_event_seq {
                let started_at = Instant::now();
                let response = match stream_state
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
                {
                    Ok(response) => response,
                    Err(error) => {
                        yield Ok(sse_error_event(
                            ErrorCode::BackendUnavailable,
                            format!("Failed to load task events: {error}"),
                            true,
                        ));
                        return;
                    }
                };
                tracing::debug!(
                    target: "oxide_agent_transport_web::web_perf",
                    user_id = stream_state.user_id,
                    session_id = %stream_state.session_id,
                    task_id = %stream_state.task_id,
                    query = "list_task_events",
                    after_seq = stream_state.last_seq,
                    limit = stream_state.limit,
                    latency_ms = started_at.elapsed().as_millis(),
                    events_count = response.events.len(),
                    last_seq = response.last_seq,
                    has_more = response.has_more,
                    "web sse db query"
                );
                if response.events.is_empty() {
                    break;
                }
                for event in response.events {
                    if event.seq <= stream_state.last_seq {
                        continue;
                    }
                    if event.seq > stream_state.task.last_event_seq {
                        break;
                    }
                    stream_state.last_seq = event.seq;
                    yield Ok(sse_persisted_task_event(&event));
                }
                if !response.has_more {
                    break;
                }
            }
        }

        // If the task is already terminal after replay, send the final
        // status (if needed) and close. No live loop entry.
        if stream_state.task.status.is_terminal()
            || matches!(stream_state.task.status, ApiTaskStatus::WaitingForUserInput)
        {
            if stream_state.task.status != last_status {
                yield Ok(sse_status_event(
                    &stream_state.task,
                    stream_state.last_seq,
                ));
            }
            return;
        }

        // Live loop: subscribe to the in-process broadcast and emit events as
        // they arrive. Keepalive is a pure timer; no DB polling.
        let Some(event_log) = stream_state.event_log.clone() else {
            // No in-process log (server restart, non-web task). Replay
            // already returned what was available; without a log there is
            // nothing more to deliver, so close cleanly.
            return;
        };
        let mut broadcast_rx = event_log.subscribe();
        let mut keepalive = tokio::time::interval(SSE_KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick fires immediately; skip it so we only emit
        // keepalives after at least one interval of idleness.
        keepalive.tick().await;

        loop {
            tokio::select! {
                biased;
                message = broadcast_rx.recv() => {
                    match message {
                        Ok(TaskEventLogMessage::Persisted { event }) => {
                            if event.seq <= stream_state.last_seq {
                                continue;
                            }
                            if event.seq > stream_state.task.last_event_seq {
                                stream_state.task.last_event_seq = event.seq;
                            }
                            stream_state.last_seq = event.seq;
                            yield Ok(sse_persisted_task_event(&event));
                        }
                        Ok(TaskEventLogMessage::Status {
                            status,
                            final_response_available,
                            last_seq,
                        }) => {
                            if last_seq > stream_state.task.last_event_seq {
                                stream_state.task.last_event_seq = last_seq;
                            }
                            stream_state.task.status = status;
                            // If the broadcast confirms a final response is
                            // available but our in-memory task record does
                            // not yet carry the content, mark it available
                            // so the emitted `task_status` event reflects
                            // the truth. A subsequent terminal event or
                            // client-side refresh will load the actual
                            // markdown.
                            if final_response_available
                                && stream_state.task.final_response_markdown.is_none()
                            {
                                stream_state.task.final_response_markdown =
                                    Some(String::new());
                            }
                            yield Ok(sse_status_event(
                                &stream_state.task,
                                stream_state.last_seq,
                            ));
                        }
                        Ok(TaskEventLogMessage::Progress { snapshot, .. }) => {
                            stream_state.task.last_progress = Some(snapshot.clone());
                            yield Ok(sse_json_event(
                                "progress",
                                &TaskSseProgress {
                                    task_id: stream_state.task_id.clone(),
                                    progress: snapshot,
                                },
                            ));
                        }
                        Ok(TaskEventLogMessage::Closed) => {
                            // Task is finished; deliver any final status
                            // from a one-shot DB read so the client sees the
                            // terminal state, then close.
                            match sse_reload_task(&stream_state).await {
                                Ok(task) => {
                                    if let Some(event) = progress_event_if_changed(
                                        &mut last_progress,
                                        &task,
                                        &stream_state.task_id,
                                    ) {
                                        yield Ok(event);
                                    }
                                    if task.status != last_status {
                                        yield Ok(sse_status_event(
                                            &task,
                                            stream_state.last_seq,
                                        ));
                                    }
                                }
                                Err(event) => {
                                    yield Ok(event);
                                }
                            }
                            return;
                        }
                        Err(RecvError::Lagged(skipped)) => {
                            // Slow consumer overflowed the broadcast ring.
                            // Log it and replay the missed range from the
                            // in-memory snapshot (cheap fast-path) or DB.
                            tracing::warn!(
                                target: "oxide_agent_transport_web::web_perf",
                                user_id = stream_state.user_id,
                                session_id = %stream_state.session_id,
                                task_id = %stream_state.task_id,
                                skipped = skipped,
                                last_seq = stream_state.last_seq,
                                "sse subscriber lagged; falling back to replay"
                            );
                            // Fast-path: drain the in-memory snapshot for
                            // the missed range. The snapshot is monotonic
                            // and deduped by `seq`, so this is safe even
                            // after the ring overflowed.
                            let snapshot = event_log.persisted_snapshot().await;
                            let mut drained_from_memory = false;
                            for event in snapshot {
                                if event.seq <= stream_state.last_seq {
                                    continue;
                                }
                                if event.seq > stream_state.task.last_event_seq {
                                    break;
                                }
                                stream_state.last_seq = event.seq;
                                yield Ok(sse_persisted_task_event(&event));
                                drained_from_memory = true;
                            }
                            if drained_from_memory {
                                continue;
                            }
                            // Slow-path: single DB page starting from
                            // last_seq. This keeps the recovery bounded
                            // and avoids an unbounded catch-up loop inside
                            // the select! arm.
                            let started_at = Instant::now();
                            let response = match stream_state
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
                            {
                                Ok(response) => response,
                                Err(error) => {
                                    yield Ok(sse_error_event(
                                        ErrorCode::BackendUnavailable,
                                        format!(
                                            "Failed to load task events: {error}"
                                        ),
                                        true,
                                    ));
                                    return;
                                }
                            };
                            tracing::debug!(
                                target: "oxide_agent_transport_web::web_perf",
                                user_id = stream_state.user_id,
                                session_id = %stream_state.session_id,
                                task_id = %stream_state.task_id,
                                query = "list_task_events",
                                after_seq = stream_state.last_seq,
                                limit = stream_state.limit,
                                latency_ms = started_at.elapsed().as_millis(),
                                events_count = response.events.len(),
                                "web sse db query"
                            );
                            for event in response.events {
                                if event.seq <= stream_state.last_seq {
                                    continue;
                                }
                                if event.seq > stream_state.task.last_event_seq {
                                    break;
                                }
                                stream_state.last_seq = event.seq;
                                yield Ok(sse_persisted_task_event(&event));
                            }
                        }
                        Err(RecvError::Closed) => {
                            return;
                        }
                    }
                }
                _ = keepalive.tick() => {
                    yield Ok(sse_json_event("keepalive", &TaskSseKeepalive {
                        last_seq: stream_state.last_seq,
                    }));
                }
            }
        }
    }
}

/// Drain any in-memory events newer than `last_seq` into the live stream.
/// Returns `Some(Event)` with a stop signal only on hard error; otherwise
/// returns `None` and yields events into the caller via the closure.
async fn sse_reload_task(stream_state: &TaskSseStreamState) -> Result<WebTaskRecord, Event> {
    let started_at = Instant::now();
    let task = stream_state
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
        })?;
    tracing::debug!(
        target: "oxide_agent_transport_web::web_perf",
        user_id = stream_state.user_id,
        session_id = %stream_state.session_id,
        task_id = %stream_state.task_id,
        query = "load_task",
        latency_ms = started_at.elapsed().as_millis(),
        found = task.is_some(),
        "web sse db query"
    );
    task.ok_or_else(|| {
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
