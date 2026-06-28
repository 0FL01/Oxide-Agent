use crate::life::state::{merge_events, merge_turns};
use crate::utils::spawn_ui;
use futures_util::{FutureExt, StreamExt};
use gloo_net::eventsource::futures::{EventSource, EventSourceBuilder, EventSourceSubscription};
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    ApiLifeEventResponse, ApiLifeRunSummary, ApiLifeSseKeepalive, ApiLifeSseSnapshot,
    ApiLifeTurnResponse,
};
use serde::Deserialize;

/// Configuration for the life SSE stream.
#[derive(Clone)]
pub(crate) struct LifeStreamConfig {
    pub set_turns: WriteSignal<Vec<ApiLifeTurnResponse>>,
    pub set_events: WriteSignal<Vec<ApiLifeEventResponse>>,
    pub set_active_run: WriteSignal<Option<ApiLifeRunSummary>>,
    pub set_error: WriteSignal<Option<String>>,
    pub streaming: ReadSignal<bool>,
    pub set_streaming: WriteSignal<bool>,
}

/// Start the life SSE stream. Reconnects with cursors from the last
/// received event IDs until `streaming` is set to `false`.
pub(crate) fn spawn_life_stream(config: LifeStreamConfig) {
    spawn_ui(async move {
        run_life_stream(config).await;
    });
}

async fn run_life_stream(config: LifeStreamConfig) {
    let mut turn_cursor: Option<String> = None;
    let mut event_cursor: Option<String> = None;
    let mut attempts = 0_u8;

    loop {
        if !config.streaming.get() {
            return;
        }

        let url = build_stream_url(&turn_cursor, &event_cursor);
        let mut source = match EventSourceBuilder::new().with_credentials(true).build(&url) {
            Ok(source) => source,
            Err(error) => {
                attempts = attempts.saturating_add(1);
                set_error_if_current(&config, error.to_string());
                if attempts >= 3 {
                    config.set_streaming.set(false);
                    return;
                }
                TimeoutFuture::new(1_000).await;
                continue;
            }
        };

        let Some(streams) = subscribe_life_streams(&mut source, &config, &mut attempts) else {
            if attempts >= 3 {
                config.set_streaming.set(false);
                return;
            }
            TimeoutFuture::new(1_000).await;
            continue;
        };

        let result =
            process_stream_messages(&config, streams, &mut turn_cursor, &mut event_cursor).await;

        if !config.streaming.get() {
            return;
        }

        if result.is_terminal() {
            config.set_streaming.set(false);
            return;
        }

        attempts = attempts.saturating_add(1);
        if attempts >= 3 {
            config.set_streaming.set(false);
            return;
        }
        TimeoutFuture::new(1_000).await;
    }
}

fn build_stream_url(turn_cursor: &Option<String>, event_cursor: &Option<String>) -> String {
    let mut url = "/api/v1/life/stream".to_string();
    let mut first = true;
    if let Some(tc) = turn_cursor {
        url.push_str(if first { "?" } else { "&" });
        first = false;
        url.push_str(&format!("turn_cursor={tc}"));
    }
    if let Some(ec) = event_cursor {
        url.push_str(if first { "?" } else { "&" });
        url.push_str(&format!("event_cursor={ec}"));
    }
    url
}

struct LifeEventStreams {
    snapshot: EventSourceSubscription,
    turn: EventSourceSubscription,
    life_event: EventSourceSubscription,
    run_status: EventSourceSubscription,
    keepalive: EventSourceSubscription,
    error: EventSourceSubscription,
}

fn subscribe_life_streams(
    source: &mut EventSource,
    config: &LifeStreamConfig,
    attempts: &mut u8,
) -> Option<LifeEventStreams> {
    let snapshot = subscribe_one(source, "snapshot", config, attempts)?;
    let turn = subscribe_one(source, "turn", config, attempts)?;
    let life_event = subscribe_one(source, "life_event", config, attempts)?;
    let run_status = subscribe_one(source, "run_status", config, attempts)?;
    let keepalive = subscribe_one(source, "keepalive", config, attempts)?;
    let error = subscribe_one(source, "error", config, attempts)?;
    Some(LifeEventStreams {
        snapshot,
        turn,
        life_event,
        run_status,
        keepalive,
        error,
    })
}

fn subscribe_one(
    source: &mut EventSource,
    event_type: &str,
    config: &LifeStreamConfig,
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

/// Result of processing the SSE stream.
enum StreamResult {
    /// Stream ended normally (connection closed by server or error).
    Closed,
    /// A terminal error was received — stop streaming.
    Terminal,
}

impl StreamResult {
    fn is_terminal(&self) -> bool {
        matches!(self, StreamResult::Terminal)
    }
}

async fn process_stream_messages(
    config: &LifeStreamConfig,
    streams: LifeEventStreams,
    turn_cursor: &mut Option<String>,
    event_cursor: &mut Option<String>,
) -> StreamResult {
    let LifeEventStreams {
        mut snapshot,
        mut turn,
        mut life_event,
        mut run_status,
        mut keepalive,
        mut error,
    } = streams;

    loop {
        if !config.streaming.get() {
            return StreamResult::Closed;
        }
        futures_util::select! {
            message = snapshot.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                handle_snapshot(config, msg);
            }
            message = turn.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                if let Some(id) = handle_turn(config, msg, turn_cursor) {
                    *turn_cursor = Some(id);
                }
            }
            message = life_event.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                if let Some(id) = handle_life_event(config, msg, event_cursor) {
                    *event_cursor = Some(id);
                }
            }
            message = run_status.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                handle_run_status(config, msg);
            }
            message = keepalive.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                handle_keepalive(msg, turn_cursor, event_cursor);
            }
            message = error.next().fuse() => {
                let Some(msg) = message else { return StreamResult::Closed; };
                if handle_error_event(config, msg) {
                    return StreamResult::Terminal;
                }
            }
        }
    }
}

fn handle_snapshot(
    config: &LifeStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) {
    let Ok((_event_type, event)) = message else {
        return;
    };
    let Some(data) = event.data().as_string() else {
        return;
    };
    match serde_json::from_str::<ApiLifeSseSnapshot>(&data) {
        Ok(snapshot) => {
            if config.streaming.get_untracked() {
                config.set_active_run.set(snapshot.active_run);
            }
        }
        Err(error) => set_error_if_current(config, error.to_string()),
    }
}

fn handle_turn(
    config: &LifeStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    turn_cursor: &Option<String>,
) -> Option<String> {
    let Ok((_event_type, event)) = message else {
        return None;
    };
    let data = event.data().as_string()?;
    match serde_json::from_str::<ApiLifeTurnResponse>(&data) {
        Ok(turn) => {
            if config.streaming.get_untracked() {
                merge_turns(config.set_turns, vec![turn], false);
            }
            Some(event.last_event_id())
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
            turn_cursor.clone()
        }
    }
}

fn handle_life_event(
    config: &LifeStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    event_cursor: &Option<String>,
) -> Option<String> {
    let Ok((_event_type, event)) = message else {
        return None;
    };
    let data = event.data().as_string()?;
    match serde_json::from_str::<ApiLifeEventResponse>(&data) {
        Ok(life_event) => {
            if config.streaming.get_untracked() {
                merge_events(config.set_events, vec![life_event], false);
            }
            Some(event.last_event_id())
        }
        Err(error) => {
            set_error_if_current(config, error.to_string());
            event_cursor.clone()
        }
    }
}

#[derive(Deserialize)]
struct RunStatusPayload {
    run_id: String,
    status: String,
}

fn handle_run_status(
    config: &LifeStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) {
    let Ok((_event_type, event)) = message else {
        return;
    };
    let Some(data) = event.data().as_string() else {
        return;
    };
    match serde_json::from_str::<RunStatusPayload>(&data) {
        Ok(payload) => {
            if !config.streaming.get_untracked() {
                return;
            }
            if payload.status == "idle" {
                config.set_active_run.set(None);
            } else {
                config.set_active_run.set(Some(ApiLifeRunSummary {
                    run_id: payload.run_id,
                    status: payload.status,
                    started_at: None,
                }));
            }
        }
        Err(error) => set_error_if_current(config, error.to_string()),
    }
}

fn handle_keepalive(
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
    turn_cursor: &mut Option<String>,
    event_cursor: &mut Option<String>,
) {
    if message.is_err() {
        return;
    }
    let Ok((_event_type, event)) = message else {
        return;
    };
    let Some(data) = event.data().as_string() else {
        return;
    };
    if let Ok(keepalive) = serde_json::from_str::<ApiLifeSseKeepalive>(&data) {
        if let Some(tc) = keepalive.turn_cursor {
            *turn_cursor = Some(tc);
        }
        if let Some(ec) = keepalive.event_cursor {
            *event_cursor = Some(ec);
        }
    }
}

#[derive(Deserialize)]
struct ErrorPayload {
    message: String,
    #[serde(default)]
    retryable: bool,
}

fn handle_error_event(
    config: &LifeStreamConfig,
    message: Result<(String, web_sys::MessageEvent), gloo_net::eventsource::EventSourceError>,
) -> bool {
    let Ok((_event_type, event)) = message else {
        return false;
    };
    let Some(data) = event.data().as_string() else {
        return false;
    };
    if let Ok(payload) = serde_json::from_str::<ErrorPayload>(&data) {
        set_error_if_current(config, payload.message);
        // Non-retryable errors stop the stream.
        !payload.retryable
    } else {
        false
    }
}

fn set_error_if_current(config: &LifeStreamConfig, error: String) {
    if config.streaming.get_untracked() {
        config.set_error.set(Some(error));
    }
}
