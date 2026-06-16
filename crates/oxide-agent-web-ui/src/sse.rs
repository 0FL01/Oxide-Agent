use crate::api::ApiClient;
use crate::utils::spawn_ui;
use futures_util::{FutureExt, StreamExt};
use gloo_net::eventsource::futures::{EventSource, EventSourceBuilder, EventSourceSubscription};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SessionDetail, SessionSummary, TaskDetail, TaskEventKind,
    TaskStatus, TaskSummary,
};
use serde::Deserialize;
use std::cmp::Ordering;

#[derive(Clone)]
pub struct TaskStreamConfig {
    pub client: ApiClient,
    pub session_id: String,
    pub task_id: String,
    pub initial_last_seq: u64,
    pub set_sessions: WriteSignal<Vec<SessionSummary>>,
    pub set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub set_progress: WriteSignal<Option<ProgressSnapshot>>,
    pub set_active_task: WriteSignal<Option<TaskDetail>>,
    pub set_tasks: WriteSignal<Vec<TaskSummary>>,
    pub set_error: WriteSignal<Option<String>>,
    pub streaming_task_id: ReadSignal<Option<String>>,
    pub set_streaming_task_id: WriteSignal<Option<String>>,
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
    let mut last_seq = config.initial_last_seq;
    for _ in 0..120 {
        TimeoutFuture::new(500).await;
        if !stream_is_current(&config) {
            return;
        }
        let Some((status, task_last_event_seq)) = refresh_task_detail(&config).await else {
            continue;
        };
        if task_status_closes_stream(status) {
            let target_last_seq = (task_last_event_seq > last_seq).then_some(task_last_event_seq);
            let _ = backfill_missed_events_until(&config, &mut last_seq, target_last_seq).await;
            finish_terminal_stream(&config).await;
            return;
        }
    }
}

async fn run_task_stream(config: TaskStreamConfig) {
    let mut last_seq = config.initial_last_seq;
    let mut attempts = 0_u8;

    loop {
        if !stream_is_current(&config) {
            return;
        }
        if backfill_missed_events(&config, &mut last_seq).await {
            finish_terminal_stream(&config).await;
            return;
        }

        let url = format!(
            "/api/v1/sessions/{}/tasks/{}/stream?after_seq={last_seq}",
            config.session_id, config.task_id
        );
        let mut source = match EventSourceBuilder::new().with_credentials(true).build(&url) {
            Ok(source) => source,
            Err(error) => {
                attempts = attempts.saturating_add(1);
                set_error_if_current(&config, error.to_string());
                if attempts >= 3 {
                    clear_streaming_task_if_current(&config);
                    return;
                }
                continue;
            }
        };
        let Some(streams) = subscribe_task_streams(&mut source, &config, &mut attempts) else {
            if attempts >= 3 {
                clear_streaming_task_if_current(&config);
                return;
            }
            continue;
        };

        if process_stream_messages(&config, streams, &mut last_seq).await {
            finish_terminal_stream(&config).await;
            return;
        }

        attempts = attempts.saturating_add(1);
        if attempts >= 3 {
            clear_streaming_task_if_current(&config);
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
            set_error_if_current(config, error.to_string());
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
    #[serde(default)]
    last_seq: u64,
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
    loop {
        if !stream_is_current(config) {
            return false;
        }
        futures_util::select! {
            message = task_events.next().fuse() => {
                let Some(message) = message else {
                    return false;
                };
                if handle_task_event_message(config, message, last_seq).await {
                    return true;
                }
            }
            message = progress_events.next().fuse() => {
                let Some(message) = message else {
                    return false;
                };
                handle_progress_message(config, message);
            }
            message = snapshot_events.next().fuse() => {
                let Some(message) = message else {
                    return false;
                };
                handle_snapshot_message(config, message, last_seq);
            }
            message = status_events.next().fuse() => {
                let Some(message) = message else {
                    return false;
                };
                if handle_status_message(config, message, last_seq).await {
                    let _ = refresh_task_detail(config).await;
                    return true;
                }
            }
            message = keepalive_events.next().fuse() => {
                let Some(message) = message else {
                    return false;
                };
                if handle_keepalive_message(config, message, last_seq).await {
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
        return false;
    };
    let Some(payload) = event.data().as_string() else {
        return false;
    };
    match serde_json::from_str::<PersistedTaskEvent>(&payload) {
        Ok(event) => {
            if event.seq > *last_seq {
                *last_seq = event.seq;
                if stream_is_current(config) && event.task_id == config.task_id {
                    append_unique_event(config.set_events, event.clone());
                }
            }
            if event.kind == TaskEventKind::Finished {
                // The event can arrive before task completion details are persisted.
                // Refresh and close only after the task record reports a closed state.
                return refresh_task_detail_closes_stream(config).await;
            }
            false
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
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
            if stream_is_current(config) && message.task_id == config.task_id {
                config.set_progress.set(Some(message.progress));
            }
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
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
            set_error_if_current(config, error.to_string());
        }
    }
}

async fn handle_status_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    last_seq: &mut u64,
) -> bool {
    let Ok((_event_type, event)) = message else {
        return false;
    };
    let Some(payload) = event.data().as_string() else {
        return false;
    };
    match serde_json::from_str::<TaskStatusMessage>(&payload) {
        Ok(event) => {
            if task_status_closes_stream(event.status) {
                let target_last_seq = (event.last_seq > *last_seq).then_some(event.last_seq);
                let _ = backfill_missed_events_until(config, last_seq, target_last_seq).await;
                return true;
            }
            false
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
            false
        }
    }
}

async fn handle_keepalive_message(
    config: &TaskStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    last_seq: &mut u64,
) -> bool {
    if message.is_err() {
        return false;
    }
    // Periodic refresh on keepalive to pick up any missed state.
    // If the task closed while the SSE replay cursor was behind, drain persisted
    // task events before closing the client stream.
    if let Some((status, task_last_event_seq)) = refresh_task_detail(config).await
        && task_status_closes_stream(status)
    {
        let target_last_seq = (task_last_event_seq > *last_seq).then_some(task_last_event_seq);
        let _ = backfill_missed_events_until(config, last_seq, target_last_seq).await;
        return true;
    }
    false
}

// ── Backfill and refresh ─────────────────────────────────────────────────

const TASK_EVENT_BACKFILL_LIMIT: usize = 500;

async fn backfill_missed_events(config: &TaskStreamConfig, last_seq: &mut u64) -> bool {
    backfill_missed_events_until(config, last_seq, None).await
}

async fn backfill_missed_events_until(
    config: &TaskStreamConfig,
    last_seq: &mut u64,
    target_last_seq: Option<u64>,
) -> bool {
    let mut saw_finished = false;
    let mut empty_polls = 0_u8;

    loop {
        let before_seq = *last_seq;
        match config
            .client
            .task_events_page(
                &config.session_id,
                &config.task_id,
                *last_seq,
                TASK_EVENT_BACKFILL_LIMIT,
            )
            .await
        {
            Ok(response) => {
                let has_more = response.has_more;
                let response_last_seq = response.last_seq;
                for event in response.events {
                    if event.seq > *last_seq {
                        *last_seq = event.seq;
                        if stream_is_current(config) && event.task_id == config.task_id {
                            append_unique_event(config.set_events, event.clone());
                        }
                    }
                    if event.kind == TaskEventKind::Finished {
                        saw_finished = true;
                    }
                }

                let target_reached = target_last_seq.is_none_or(|target| *last_seq >= target);
                if saw_finished && target_reached {
                    return refresh_task_detail_closes_stream(config).await;
                }
                if has_more {
                    continue;
                }
                if target_reached {
                    break;
                }
                if response_last_seq <= before_seq {
                    if empty_polls >= 3 {
                        break;
                    }
                    empty_polls = empty_polls.saturating_add(1);
                    TimeoutFuture::new(100).await;
                }
            }
            Err(error) => {
                set_error_if_current(config, error.to_string());
                break;
            }
        }
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
            if stream_is_current(config) {
                config.set_progress.set(response.progress);
            }
            terminal
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
            false
        }
    }
}

async fn refresh_task_detail(config: &TaskStreamConfig) -> Option<(TaskStatus, u64)> {
    match config
        .client
        .get_task(&config.session_id, &config.task_id)
        .await
    {
        Ok(response) => {
            let detail = response.task;
            let summary = task_detail_to_summary(&detail);
            let status = summary.status;
            let last_event_seq = detail.last_event_seq;
            if stream_is_current(config) {
                config.set_progress.set(detail.last_progress.clone());
                if detail.status.is_terminal() {
                    config.set_active_task.set(None);
                } else {
                    config.set_active_task.set(Some(detail));
                }
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
                items.sort_by(|a, b| {
                    a.created_at
                        .cmp(&b.created_at)
                        .then_with(|| a.task_id.cmp(&b.task_id))
                });
            });
            refresh_session_summary(config).await;
            Some((status, last_event_seq))
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
            None
        }
    }
}

async fn refresh_task_detail_closes_stream(config: &TaskStreamConfig) -> bool {
    refresh_task_detail(config)
        .await
        .is_some_and(|(status, _last_event_seq)| task_status_closes_stream(status))
}

const fn task_status_closes_stream(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::WaitingForUserInput) || status.is_terminal()
}

async fn refresh_session_summary(config: &TaskStreamConfig) {
    if !stream_is_current(config) {
        return;
    }
    match config.client.get_session(&config.session_id).await {
        Ok(response) => {
            if !stream_is_current(config) {
                return;
            }
            let summary = session_detail_to_summary(response.session);
            upsert_session_summary(config.set_sessions, summary);
        }
        Err(error) => set_error_if_current(config, error.to_string()),
    }
}

async fn poll_session_summary_after_terminal(config: &TaskStreamConfig) {
    for _ in 0..20 {
        TimeoutFuture::new(1_500).await;
        if !stream_is_current(config) {
            return;
        }
        refresh_session_summary(config).await;
    }
}

async fn finish_terminal_stream(config: &TaskStreamConfig) {
    poll_session_summary_after_terminal(config).await;
    clear_streaming_task_if_current(config);
}

fn stream_is_current(config: &TaskStreamConfig) -> bool {
    config
        .streaming_task_id
        .get_untracked()
        .as_deref()
        .is_some_and(|task_id| task_id == config.task_id.as_str())
}

fn set_error_if_current(config: &TaskStreamConfig, error: String) {
    if stream_is_current(config) {
        config.set_error.set(Some(error));
    }
}

fn clear_streaming_task_if_current(config: &TaskStreamConfig) {
    config.set_streaming_task_id.update(|current| {
        if current.as_deref() == Some(config.task_id.as_str()) {
            *current = None;
        }
    });
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
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.created_at.cmp(&a.created_at))
                .then_with(|| b.session_id.cmp(&a.session_id))
        });
    });
}

fn session_detail_to_summary(session: SessionDetail) -> SessionSummary {
    SessionSummary {
        session_id: session.session_id,
        title: session.title,
        model_selection: session.model_selection,
        agent_profile_id: session.agent_profile_id,
        last_preview: session.last_preview,
        active_task_id: session.active_task_id,
        last_task_status: session.last_task_status,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn append_unique_event(events: WriteSignal<Vec<PersistedTaskEvent>>, event: PersistedTaskEvent) {
    events.update(|items| {
        if !items
            .iter()
            .any(|item| item.task_id == event.task_id && item.seq == event.seq)
        {
            let needs_sort = items
                .last()
                .is_some_and(|last| compare_task_events(last, &event) == Ordering::Greater);
            items.push(event);
            if needs_sort {
                items.sort_by(compare_task_events);
            }
        }
    });
}

fn compare_task_events(a: &PersistedTaskEvent, b: &PersistedTaskEvent) -> Ordering {
    a.created_at
        .cmp(&b.created_at)
        .then_with(|| a.task_id.cmp(&b.task_id))
        .then_with(|| a.seq.cmp(&b.seq))
}

fn task_detail_to_summary(task: &TaskDetail) -> TaskSummary {
    TaskSummary {
        task_id: task.task_id.clone(),
        version_group_id: task.effective_version_group_id().to_string(),
        version_index: task.effective_version_index(),
        parent_task_id: task.parent_task_id.clone(),
        status: task.status,
        input_markdown: task.input_markdown.clone(),
        attachments: task.attachments.clone(),
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
