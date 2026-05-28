use crate::api::ApiClient;
use crate::utils::spawn_ui;
use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSourceBuilder;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SseConnectionState, TaskDetail, TaskEventKind,
    TaskSummary,
};

#[derive(Clone)]
pub struct TaskStreamConfig {
    pub client: ApiClient,
    pub session_id: String,
    pub task_id: String,
    pub set_session_title: WriteSignal<String>,
    pub set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub set_progress: WriteSignal<Option<ProgressSnapshot>>,
    pub set_active_task: WriteSignal<Option<TaskDetail>>,
    pub set_tasks: WriteSignal<Vec<TaskSummary>>,
    pub set_state: WriteSignal<SseConnectionState>,
    pub set_error: WriteSignal<Option<String>>,
}

pub fn spawn_task_stream(config: TaskStreamConfig) {
    spawn_ui(async move {
        run_task_stream(config).await;
    });
}

async fn run_task_stream(config: TaskStreamConfig) {
    let mut last_seq = 0;
    let mut attempts = 0_u8;

    loop {
        if backfill_missed_events(&config, &mut last_seq).await {
            config.set_state.set(SseConnectionState::TerminalClosed);
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
                    return;
                }
                continue;
            }
        };
        let mut task_events = match source.subscribe("task_event") {
            Ok(events) => events,
            Err(error) => {
                attempts = attempts.saturating_add(1);
                config.set_error.set(Some(error.to_string()));
                if attempts >= 3 {
                    config.set_state.set(SseConnectionState::Disconnected);
                    return;
                }
                continue;
            }
        };

        config.set_state.set(SseConnectionState::Connected);
        while let Some(message) = task_events.next().await {
            let Ok((_event_type, event)) = message else {
                config.set_state.set(SseConnectionState::Disconnected);
                break;
            };
            let Some(payload) = event.data().as_string() else {
                continue;
            };
            match serde_json::from_str::<PersistedTaskEvent>(&payload) {
                Ok(event) => {
                    if event.seq > last_seq {
                        last_seq = event.seq;
                        append_unique_event(config.set_events, event.clone());
                    }
                    let terminal = refresh_progress(&config).await;
                    if event.kind == TaskEventKind::Finished || terminal {
                        refresh_task_detail(&config).await;
                        config.set_state.set(SseConnectionState::TerminalClosed);
                        return;
                    }
                }
                Err(error) => config.set_error.set(Some(error.to_string())),
            }
        }

        attempts = attempts.saturating_add(1);
        if attempts >= 3 {
            config.set_state.set(SseConnectionState::Disconnected);
            return;
        }
    }
}

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
                    refresh_task_detail(config).await;
                    return true;
                }
            }
        }
        Err(error) => config.set_error.set(Some(error.to_string())),
    }

    refresh_progress(config).await
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

async fn refresh_task_detail(config: &TaskStreamConfig) {
    match config
        .client
        .get_task(&config.session_id, &config.task_id)
        .await
    {
        Ok(response) => {
            let detail = response.task;
            let summary = task_detail_to_summary(&detail);
            config.set_progress.set(detail.last_progress.clone());
            if detail.status.is_terminal() {
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
        }
        Err(error) => config.set_error.set(Some(error.to_string())),
    }

    match config.client.get_session(&config.session_id).await {
        Ok(response) => config.set_session_title.set(response.session.title),
        Err(error) => config.set_error.set(Some(error.to_string())),
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
