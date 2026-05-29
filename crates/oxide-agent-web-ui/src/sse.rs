use crate::api::ApiClient;
use crate::utils::spawn_ui;
use futures_util::{FutureExt, StreamExt};
use gloo_net::eventsource::futures::{EventSource, EventSourceBuilder, EventSourceSubscription};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SessionDetail, SessionSummary, SseConnectionState,
    TaskDetail, TaskEventKind, TaskStatus, TaskSummary,
};
use serde::Deserialize;

#[derive(Clone)]
pub struct TaskStreamConfig {
    pub client: ApiClient,
    pub session_id: String,
    pub task_id: String,
    pub set_session_title: WriteSignal<String>,
    pub set_sessions: WriteSignal<Vec<SessionSummary>>,
    pub set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub set_progress: WriteSignal<Option<ProgressSnapshot>>,
    pub set_active_task: WriteSignal<Option<TaskDetail>>,
    pub set_tasks: WriteSignal<Vec<TaskSummary>>,
    pub set_state: WriteSignal<SseConnectionState>,
    pub set_error: WriteSignal<Option<String>>,
    pub set_streaming_task_id: WriteSignal<Option<String>>,
    pub set_last_terminal_status: WriteSignal<Option<TaskStatus>>,
}

pub fn spawn_task_stream(config: TaskStreamConfig) {
    let poll_config = config.clone();
    spawn_ui(async move {
        poll_task_detail_until_paused_or_terminal(poll_config).await;
    });
    spawn_ui(async move {
        run_task_stream(config).await;
    });
}

async fn poll_task_detail_until_paused_or_terminal(config: TaskStreamConfig) {
    for _ in 0..120 {
        TimeoutFuture::new(500).await;
        if matches!(
            refresh_task_detail(&config).await,
            Some(TaskStatus::WaitingForUserInput)
                | Some(TaskStatus::Completed)
                | Some(TaskStatus::Failed)
                | Some(TaskStatus::Cancelled)
                | Some(TaskStatus::Interrupted)
        ) {
            config.set_state.set(SseConnectionState::TerminalClosed);
            config.set_streaming_task_id.set(None);
            poll_session_summary_after_terminal(&config).await;
            return;
        }
    }
}

async fn run_task_stream(config: TaskStreamConfig) {
    let mut last_seq = 0;
    let mut attempts = 0_u8;

    loop {
        if backfill_missed_events(&config, &mut last_seq).await {
            config.set_state.set(SseConnectionState::TerminalClosed);
            config.set_streaming_task_id.set(None);
            return;
        }

        config.set_state.set(SseConnectionState::Reconnecting);
        let url = format!(
            "/api/v1/sessions/{}/tasks/{}/stream?after_seq={last_seq}",
            config.session_id, config.task_id
        );
        let mut source = match EventSourceBuilder::new().with_credentials(true).build(&url) {
            Ok(source) => source,
            Err(error) => {
                attempts = attempts.saturating_add(1);
                config.set_error.set(Some(error.to_string()));
                if attempts >= 3 {
                    config.set_state.set(SseConnectionState::Disconnected);
                    config.set_streaming_task_id.set(None);
                    return;
                }
                continue;
            }
        };
        let Some(streams) = subscribe_task_streams(&mut source, &config, &mut attempts) else {
            if attempts >= 3 {
                config.set_state.set(SseConnectionState::Disconnected);
                config.set_streaming_task_id.set(None);
                return;
            }
            continue;
        };

        if process_stream_messages(&config, streams, &mut last_seq).await {
            config.set_state.set(SseConnectionState::TerminalClosed);
            config.set_streaming_task_id.set(None);
            poll_session_summary_after_terminal(&config).await;
            return;
        }

        attempts = attempts.saturating_add(1);
        if attempts >= 3 {
            config.set_state.set(SseConnectionState::Disconnected);
            config.set_streaming_task_id.set(None);
            return;
        }
    }
}

// ── SSE stream subscriptions ─────────────────────────────────────────────

struct TaskEventStreams {
    snapshot_events: EventSourceSubscription,
    task_events: EventSourceSubscription,
    progress_events: EventSourceSubscription,
    status_events: EventSourceSubscription,
    keepalive_events: EventSourceSubscription,
}

fn subscribe_task_streams(
    source: &mut EventSource,
    config: &TaskStreamConfig,
    attempts: &mut u8,
) -> Option<TaskEventStreams> {
    let snapshot_events = subscribe_one(source, "snapshot", config, attempts)?;
    let task_events = subscribe_one(source, "task_event", config, attempts)?;
    let progress_events = subscribe_one(source, "progress", config, attempts)?;
    let status_events = subscribe_one(source, "task_status", config, attempts)?;
    let keepalive_events = subscribe_one(source, "keepalive", config, attempts)?;
    Some(TaskEventStreams {
        snapshot_events,
        task_events,
        progress_events,
        status_events,
        keepalive_events,
    })
}

fn subscribe_one(
    source: &mut EventSource,
    event_type: &str,
    config: &TaskStreamConfig,
    attempts: &mut u8,
) -> Option<EventSourceSubscription> {
    match source.subscribe(event_type) {
        Ok(events) => Some(events),
        Err(error) => {
            *attempts = attempts.saturating_add(1);
            config.set_error.set(Some(error.to_string()));
            None
        }
    }
}

// ── Message processing ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TaskSseSnapshotMessage {
    last_seq: u64,
}

#[derive(Debug, Deserialize)]
struct TaskSseProgressMessage {
    task_id: String,
    progress: ProgressSnapshot,
}

#[derive(Debug, Deserialize)]
struct TaskStatusMessage {
    status: TaskStatus,
}

async fn process_stream_messages(
    config: &TaskStreamConfig,
    streams: TaskEventStreams,
    last_seq: &mut u64,
) -> bool {
    let TaskEventStreams {
        mut snapshot_events,
        mut task_events,
        mut progress_events,
        mut status_events,
        mut keepalive_events,
    } = streams;
    config.set_state.set(SseConnectionState::Connected);
    loop {
        futures_util::select! {
            message = task_events.next().fuse() => {
                let Some(message) = message else {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return false;
                };
                if handle_task_event_message(config, message, last_seq).await {
                    return true;
                }
            }
            message = progress_events.next().fuse() => {
                let Some(message) = message else {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return false;
                };
                handle_progress_message(config, message);
            }
            message = snapshot_events.next().fuse() => {
                let Some(message) = message else {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return false;
                };
                handle_snapshot_message(config, message, last_seq);
            }
            message = status_events.next().fuse() => {
                let Some(message) = message else {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return false;
                };
                if handle_status_message(config, message).await {
                    let _ = refresh_task_detail(config).await;
                    return true;
                }
            }
            message = keepalive_events.next().fuse() => {
                let Some(message) = message else {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return false;
                };
                if handle_keepalive_message(config, message).await {
                    return true;
                }
            }
        }
    }
}

async fn handle_task_event_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    last_seq: &mut u64,
) -> bool {
    let Ok((_event_type, event)) = message else {
        config.set_state.set(SseConnectionState::Disconnected);
        return false;
    };
    let Some(payload) = event.data().as_string() else {
        return false;
    };
    match serde_json::from_str::<PersistedTaskEvent>(&payload) {
        Ok(event) => {
            if event.seq > *last_seq {
                *last_seq = event.seq;
                append_unique_event(config.set_events, event.clone());
            }
            // No HTTP refresh here — progress comes via SSE progress channel.
            // Terminal events still need a detail refresh to pick up final_response_markdown.
            event.kind == TaskEventKind::Finished
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
            false
        }
    }
}

fn handle_progress_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) {
    let Ok((_event_type, event)) = message else {
        return;
    };
    let Some(payload) = event.data().as_string() else {
        return;
    };
    match serde_json::from_str::<TaskSseProgressMessage>(&payload) {
        Ok(message) => {
            if message.task_id == config.task_id {
                config.set_progress.set(Some(message.progress));
            }
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
        }
    }
}

fn handle_snapshot_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    last_seq: &mut u64,
) {
    let Ok((_event_type, event)) = message else {
        return;
    };
    let Some(payload) = event.data().as_string() else {
        return;
    };
    match serde_json::from_str::<TaskSseSnapshotMessage>(&payload) {
        Ok(message) => {
            // Snapshot gives us the authoritative last_seq for the replay cursor
            if message.last_seq > *last_seq {
                *last_seq = message.last_seq;
            }
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
        }
    }
}

async fn handle_status_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) -> bool {
    let Ok((_event_type, event)) = message else {
        config.set_state.set(SseConnectionState::Disconnected);
        return false;
    };
    let Some(payload) = event.data().as_string() else {
        return false;
    };
    match serde_json::from_str::<TaskStatusMessage>(&payload) {
        Ok(event) => {
            matches!(event.status, TaskStatus::WaitingForUserInput) || event.status.is_terminal()
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
            false
        }
    }
}

async fn handle_keepalive_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) -> bool {
    if message.is_err() {
        config.set_state.set(SseConnectionState::Disconnected);
        return false;
    }
    // Periodic refresh on keepalive to pick up any missed state
    matches!(
        refresh_task_detail(config).await,
        Some(TaskStatus::WaitingForUserInput)
            | Some(TaskStatus::Completed)
            | Some(TaskStatus::Failed)
            | Some(TaskStatus::Cancelled)
            | Some(TaskStatus::Interrupted)
    )
}

// ── Backfill and refresh ─────────────────────────────────────────────────

async fn backfill_missed_events(config: &TaskStreamConfig, last_seq: &mut u64) -> bool {
    match config
        .client
        .task_events(&config.session_id, &config.task_id, *last_seq)
        .await
    {
        Ok(response) => {
            for event in response.events {
                if event.seq > *last_seq {
                    *last_seq = event.seq;
                    append_unique_event(config.set_events, event.clone());
                }
                if event.kind == TaskEventKind::Finished {
                    let _ = refresh_task_detail(config).await;
                    return true;
                }
            }
        }
        Err(error) => config.set_error.set(Some(error.to_string())),
    }

    // Backfill also refreshes progress as a safety net
    let _ = refresh_progress(config).await;
    false
}

async fn refresh_progress(config: &TaskStreamConfig) -> bool {
    match config
        .client
        .task_progress(&config.session_id, &config.task_id)
        .await
    {
        Ok(response) => {
            let terminal = response.status.is_terminal();
            config.set_progress.set(response.progress);
            terminal
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
            false
        }
    }
}

async fn refresh_task_detail(config: &TaskStreamConfig) -> Option<TaskStatus> {
    match config
        .client
        .get_task(&config.session_id, &config.task_id)
        .await
    {
        Ok(response) => {
            let detail = response.task;
            let summary = task_detail_to_summary(&detail);
            let status = summary.status;
            config.set_progress.set(detail.last_progress.clone());
            if detail.status.is_terminal() {
                config.set_last_terminal_status.set(Some(status));
                config.set_active_task.set(None);
            } else {
                config.set_active_task.set(Some(detail));
            }
            config.set_tasks.update(|items| {
                if let Some(existing) = items
                    .iter_mut()
                    .find(|item| item.task_id == summary.task_id)
                {
                    *existing = summary;
                } else {
                    items.push(summary);
                }
                items.sort_by_key(|item| item.updated_at);
            });
            refresh_session_summary(config).await;
            Some(status)
        }
        Err(error) => {
            config.set_error.set(Some(error.to_string()));
            None
        }
    }
}

async fn refresh_session_summary(config: &TaskStreamConfig) {
    match config.client.get_session(&config.session_id).await {
        Ok(response) => {
            let summary = session_detail_to_summary(response.session);
            config.set_session_title.set(summary.title.clone());
            upsert_session_summary(config.set_sessions, summary);
        }
        Err(error) => config.set_error.set(Some(error.to_string())),
    }
}

async fn poll_session_summary_after_terminal(config: &TaskStreamConfig) {
    for _ in 0..20 {
        TimeoutFuture::new(1_500).await;
        refresh_session_summary(config).await;
    }
}

fn upsert_session_summary(set_sessions: WriteSignal<Vec<SessionSummary>>, summary: SessionSummary) {
    set_sessions.update(|items| {
        if let Some(existing) = items
            .iter_mut()
            .find(|item| item.session_id == summary.session_id)
        {
            *existing = summary;
        } else {
            items.push(summary);
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    });
}

fn session_detail_to_summary(session: SessionDetail) -> SessionSummary {
    SessionSummary {
        session_id: session.session_id,
        title: session.title,
        last_preview: session.last_preview,
        active_task_id: session.active_task_id,
        last_task_status: session.last_task_status,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn append_unique_event(events: WriteSignal<Vec<PersistedTaskEvent>>, event: PersistedTaskEvent) {
    events.update(|items| {
        if !items.iter().any(|item| item.seq == event.seq) {
            items.push(event);
            items.sort_by_key(|item| item.seq);
        }
    });
}

fn task_detail_to_summary(task: &TaskDetail) -> TaskSummary {
    TaskSummary {
        task_id: task.task_id.clone(),
        status: task.status,
        input_markdown: task.input_markdown.clone(),
        input_edited_at: task.input_edited_at,
        final_response_markdown: task.final_response_markdown.clone(),
        error_message: task.error_message.clone(),
        pending_user_input: task.pending_user_input.clone(),
        last_event_seq: task.last_event_seq,
        created_at: task.created_at,
        started_at: task.started_at,
        updated_at: task.updated_at,
        finished_at: task.finished_at,
    }
}
