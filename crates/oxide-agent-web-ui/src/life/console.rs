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
use std::collections::HashMap;

const LIFE_TURNS_PAGE: usize = 50;
const LIFE_EVENTS_PAGE: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LifeRunActivityPageState {
    next_cursor: Option<String>,
    has_more: bool,
    loading: bool,
}

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
    let (cancel_loading, set_cancel_loading) = signal(false);

    // ── Composer state ─────────────────────────────────────────────────
    let (input, set_input) = signal(String::new());
    let (pending_files, set_pending_files) = signal(Vec::<PendingAttachmentFile>::new());
    let (next_pending_file_id, set_next_pending_file_id) = signal(0_usize);

    // ── Paging state ───────────────────────────────────────────────────
    let (turn_cursor, set_turn_cursor) = signal(None::<String>);
    let (turns_has_more, set_turns_has_more) = signal(false);
    let (loading_older_turns, set_loading_older_turns) = signal(false);

    // ── Activity drawer ────────────────────────────────────────────────
    let (drawer_open, set_drawer_open) = signal(false);
    let (selected_activity_run_id, set_selected_activity_run_id) = signal(None::<String>);
    let (run_activity_pages, set_run_activity_pages) =
        signal(HashMap::<String, LifeRunActivityPageState>::new());

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
        let Some(run_id) = selected_activity_run_id.get_untracked() else {
            return;
        };
        let Some(page) = run_activity_pages.get_untracked().get(&run_id).cloned() else {
            return;
        };
        if page.loading || !page.has_more {
            return;
        }
        let cursor = page.next_cursor;
        set_run_activity_pages.update(|pages| {
            pages.entry(run_id.clone()).or_default().loading = true;
        });
        set_error.set(None);
        spawn_ui(async move {
            let client = auth.client();
            match client
                .list_life_events(Some(&run_id), cursor.as_deref(), LIFE_EVENTS_PAGE)
                .await
            {
                Ok(response) => {
                    let mut events_asc = response.events;
                    events_asc.reverse();
                    merge_events(set_events, events_asc, true);
                    set_run_activity_pages.update(|pages| {
                        pages.insert(
                            run_id.clone(),
                            LifeRunActivityPageState {
                                has_more: response.next_cursor.is_some(),
                                next_cursor: response.next_cursor,
                                loading: false,
                            },
                        );
                    });
                }
                Err(error) => {
                    set_error.set(Some(error.to_string()));
                    set_run_activity_pages.update(|pages| {
                        pages.entry(run_id.clone()).or_default().loading = false;
                    });
                }
            }
        });
    });

    // ── Submit callback ────────────────────────────────────────────────
    let on_submitted = Callback::new(move |_response: ApiLifeSubmitResponse| {
        // The SSE stream will deliver the new turn and events.
        // No local state mutation needed here — the server is source of truth.
    });

    // ── Activity toggle ────────────────────────────────────────────────
    let open_activity_for_run = Callback::new(move |run_id: String| {
        let already_open = drawer_open.get_untracked()
            && selected_activity_run_id.get_untracked().as_deref() == Some(run_id.as_str());
        if already_open {
            set_drawer_open.set(false);
            return;
        }

        set_selected_activity_run_id.set(Some(run_id.clone()));
        set_drawer_open.set(true);

        if run_activity_pages.get_untracked().contains_key(&run_id) {
            return;
        }

        set_run_activity_pages.update(|pages| {
            pages.insert(
                run_id.clone(),
                LifeRunActivityPageState {
                    loading: true,
                    ..LifeRunActivityPageState::default()
                },
            );
        });

        spawn_ui(async move {
            let client = auth.client();
            match client
                .list_life_events(Some(&run_id), None, LIFE_EVENTS_PAGE)
                .await
            {
                Ok(response) => {
                    let mut events_asc = response.events;
                    events_asc.reverse();
                    merge_events(set_events, events_asc, false);
                    set_run_activity_pages.update(|pages| {
                        pages.insert(
                            run_id.clone(),
                            LifeRunActivityPageState {
                                has_more: response.next_cursor.is_some(),
                                next_cursor: response.next_cursor,
                                loading: false,
                            },
                        );
                    });
                }
                Err(error) => {
                    set_error.set(Some(error.to_string()));
                    set_run_activity_pages.update(|pages| {
                        pages.remove(&run_id);
                    });
                }
            }
        });
    });

    let cancel_active_run = Callback::new(move |_| {
        if cancel_loading.get_untracked() {
            return;
        }
        let Some(run) = active_run.get_untracked() else {
            return;
        };
        let run_id = run.run_id;
        set_cancel_loading.set(true);
        set_error.set(None);

        spawn_ui(async move {
            let client = auth.client();
            match client.cancel_life_run(&run_id).await {
                Ok(response) => {
                    if response.status != "running" {
                        set_active_run.set(None);
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_cancel_loading.set(false);
        });
    });

    let selected_run_has_older_events = Signal::derive(move || {
        selected_activity_run_id
            .get()
            .and_then(|run_id| {
                run_activity_pages
                    .get()
                    .get(&run_id)
                    .map(|page| page.has_more)
            })
            .unwrap_or(false)
    });
    let selected_run_events_loading = Signal::derive(move || {
        selected_activity_run_id
            .get()
            .and_then(|run_id| {
                run_activity_pages
                    .get()
                    .get(&run_id)
                    .map(|page| page.loading)
            })
            .unwrap_or(false)
    });

    view! {
        <ErrorBanner message=error />
        <section class="life-console">
            <div class="life-chat-wrapper">
                <div class="life-results-panel">
                    <LifeTranscript
                        turns=turns
                        selected_activity_run_id=selected_activity_run_id
                        drawer_open=drawer_open
                        open_activity=open_activity_for_run
                        has_more=Signal::derive(move || turns_has_more.get())
                        loading_older=Signal::derive(move || loading_older_turns.get())
                        load_older=load_older_turns
                    />
                    {move || {
                        if is_running.get() {
                            view! {
                                <div class="life-active-run-status">
                                    <button
                                        class=move || {
                                            let active_run_id = active_run.get().map(|run| run.run_id);
                                            let is_open = drawer_open.get()
                                                && active_run_id.as_deref().is_some_and(|run_id| {
                                                    selected_activity_run_id.get().as_deref() == Some(run_id)
                                                });
                                            if is_open { "life-activity-toggle open" } else { "life-activity-toggle" }
                                        }
                                        type="button"
                                        on:click=move |_| {
                                            if let Some(run) = active_run.get_untracked() {
                                                open_activity_for_run.run(run.run_id);
                                            }
                                        }
                                    >
                                        <span>"Thinking"</span>
                                        <span class="chevron">"›"</span>
                                    </button>
                                    <button
                                        class="btn-danger life-stop-run"
                                        type="button"
                                        disabled=move || cancel_loading.get()
                                        on:click=move |_| cancel_active_run.run(())
                                    >
                                        {move || if cancel_loading.get() { "Stopping…" } else { "Stop" }}
                                    </button>
                                </div>
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
                selected_run_id=selected_activity_run_id
                has_older=selected_run_has_older_events
                loading_older=selected_run_events_loading
                load_older=load_older_events
            />
        </section>
    }
}
