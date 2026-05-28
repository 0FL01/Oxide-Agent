use crate::api::ApiClient;
use crate::utils::spawn_ui;
use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSourceBuilder;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    PersistedTaskEvent, ProgressSnapshot, SseConnectionState, TaskEventKind,
};

#[derive(Clone)]
pub struct TaskStreamConfig {
    pub client: ApiClient,
    pub session_id: String,
    pub task_id: String,
    pub set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    pub set_progress: WriteSignal<Option<ProgressSnapshot>>,
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
                    refresh_progress(&config).await;
                    if event.kind == TaskEventKind::Finished {
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
                    refresh_progress(config).await;
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

fn append_unique_event(events: WriteSignal<Vec<PersistedTaskEvent>>, event: PersistedTaskEvent) {
    events.update(|items| {
        if !items.iter().any(|item| item.seq == event.seq) {
            items.push(event);
            items.sort_by_key(|item| item.seq);
        }
    });
}
