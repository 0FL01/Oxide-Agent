use crate::auth::use_auth;
use crate::components::ErrorBanner;
use crate::life::activity::LifeActivityDrawer;
use crate::life::composer::LifeComposer;
use crate::life::state::{merge_events, merge_turns};
use crate::life::streaming::{LifeStreamConfig, spawn_life_stream};
use crate::life::transcript::LifeTranscript;
use crate::tasks::composer::PendingAttachmentFile;
use crate::utils::spawn_ui;
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    ApiLifeEventResponse, ApiLifeRunSummary, ApiLifeSubmitResponse, ApiLifeTurnResponse,
};

const LIFE_TURNS_PAGE: usize = 50;
const LIFE_EVENTS_PAGE: usize = 100;

/// Permanent chat console — the main UI for life mode.
///
/// Renders a continuous transcript of turns, a composer for new input,
/// and an activity drawer for live run events. State is self-contained
/// and independent from the task console.
#[component]
pub fn LifeConsole() -> impl IntoView {
    let auth = use_auth();

    // ── Core state ─────────────────────────────────────────────────────
    let (turns, set_turns) = signal(Vec::<ApiLifeTurnResponse>::new());
    let (events, set_events) = signal(Vec::<ApiLifeEventResponse>::new());
    let (active_run, set_active_run) = signal(None::<ApiLifeRunSummary>);
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);

    // ── Composer state ─────────────────────────────────────────────────
    let (input, set_input) = signal(String::new());
    let (pending_files, set_pending_files) = signal(Vec::<PendingAttachmentFile>::new());
    let (next_pending_file_id, set_next_pending_file_id) = signal(0_usize);

    // ── Paging state ───────────────────────────────────────────────────
    let (turn_cursor, set_turn_cursor) = signal(None::<String>);
    let (turns_has_more, set_turns_has_more) = signal(false);
    let (loading_older_turns, set_loading_older_turns) = signal(false);

    let (event_cursor, set_event_cursor) = signal(None::<String>);
    let (events_has_more, set_events_has_more) = signal(false);
    let (loading_older_events, set_loading_older_events) = signal(false);

    // ── Activity drawer ────────────────────────────────────────────────
    let (drawer_open, set_drawer_open) = signal(false);

    // ── SSE streaming ──────────────────────────────────────────────────
    let (streaming, set_streaming) = signal(false);

    let is_running = Signal::derive(move || active_run.get().is_some());

    // ── Initial load on mount ──────────────────────────────────────────
    Effect::new(move |_| {
        // Guard: only run once.
        if streaming.get_untracked() {
            return;
        }
        set_streaming.set(true);

        spawn_ui(async move {
            let client = auth.client();

            // Fetch initial turns (newest first → reverse to ascending).
            let turns_result = client.list_life_turns(None, LIFE_TURNS_PAGE).await;
            if let Ok(response) = turns_result {
                let mut turns_asc = response.turns;
                turns_asc.reverse();
                set_turns.set(turns_asc);
                set_turns_has_more.set(response.next_cursor.is_some());
                set_turn_cursor.set(response.next_cursor);
            }

            // Fetch initial events (newest first → reverse to ascending).
            let events_result = client.list_life_events(None, None, LIFE_EVENTS_PAGE).await;
            if let Ok(response) = events_result {
                let mut events_asc = response.events;
                events_asc.reverse();
                set_events.set(events_asc);
                set_events_has_more.set(response.next_cursor.is_some());
                set_event_cursor.set(response.next_cursor);
            }

            // Fetch life state for active run.
            let _ = client.get_life_state().await;

            // Start SSE stream for live updates.
            spawn_life_stream(LifeStreamConfig {
                set_turns,
                set_events,
                set_active_run,
                set_error,
                streaming,
                set_streaming,
            });
        });
    });

    // ── Load older turns ───────────────────────────────────────────────
    let load_older_turns = Callback::new(move |_| {
        if loading_older_turns.get_untracked() || !turns_has_more.get_untracked() {
            return;
        }
        let cursor = turn_cursor.get_untracked();
        set_loading_older_turns.set(true);
        set_error.set(None);
        spawn_ui(async move {
            let client = auth.client();
            match client
                .list_life_turns(cursor.as_deref(), LIFE_TURNS_PAGE)
                .await
            {
                Ok(response) => {
                    let mut turns_asc = response.turns;
                    turns_asc.reverse();
                    merge_turns(set_turns, turns_asc, true);
                    set_turns_has_more.set(response.next_cursor.is_some());
                    set_turn_cursor.set(response.next_cursor);
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading_older_turns.set(false);
        });
    });

    // ── Load older events ──────────────────────────────────────────────
    let load_older_events = Callback::new(move |_| {
        if loading_older_events.get_untracked() || !events_has_more.get_untracked() {
            return;
        }
        let cursor = event_cursor.get_untracked();
        let run_id = active_run.get_untracked().map(|r| r.run_id);
        set_loading_older_events.set(true);
        set_error.set(None);
        spawn_ui(async move {
            let client = auth.client();
            match client
                .list_life_events(run_id.as_deref(), cursor.as_deref(), LIFE_EVENTS_PAGE)
                .await
            {
                Ok(response) => {
                    let mut events_asc = response.events;
                    events_asc.reverse();
                    merge_events(set_events, events_asc, true);
                    set_events_has_more.set(response.next_cursor.is_some());
                    set_event_cursor.set(response.next_cursor);
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading_older_events.set(false);
        });
    });

    // ── Submit callback ────────────────────────────────────────────────
    let on_submitted = Callback::new(move |_response: ApiLifeSubmitResponse| {
        // The SSE stream will deliver the new turn and events.
        // No local state mutation needed here — the server is source of truth.
    });

    // ── Activity toggle ────────────────────────────────────────────────
    let toggle_drawer = Callback::new(move |_| {
        set_drawer_open.update(|open| *open = !*open);
    });

    view! {
        <ErrorBanner message=error />
        <section class="life-console">
            <div class="life-chat-wrapper">
                <div class="life-results-panel">
                    <LifeTranscript
                        turns=turns
                        has_more=Signal::derive(move || turns_has_more.get())
                        loading_older=Signal::derive(move || loading_older_turns.get())
                        load_older=load_older_turns
                    />
                    {move || {
                        if is_running.get() {
                            view! {
                                <button
                                    class=move || if drawer_open.get() { "life-activity-toggle open" } else { "life-activity-toggle" }
                                    type="button"
                                    on:click=move |ev| toggle_drawer.run(ev)
                                >
                                    <span class="dot"></span>
                                    <span>"Thinking"</span>
                                    <span class="chevron">"›"</span>
                                </button>
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                </div>
                <LifeComposer
                    auth=auth
                    input=input
                    set_input=set_input
                    pending_files=pending_files
                    set_pending_files=set_pending_files
                    next_pending_file_id=next_pending_file_id
                    set_next_pending_file_id=set_next_pending_file_id
                    loading=loading
                    is_running=is_running
                    set_loading=set_loading
                    set_error=set_error
                    on_submitted=on_submitted
                />
            </div>
            <LifeActivityDrawer
                open=drawer_open
                set_open=set_drawer_open
                events=events
                active_run=active_run
                has_older=Signal::derive(move || events_has_more.get())
                loading_older=Signal::derive(move || loading_older_events.get())
                load_older=load_older_events
            />
        </section>
    }
}
