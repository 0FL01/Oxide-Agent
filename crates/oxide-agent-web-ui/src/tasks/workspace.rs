use crate::api::{ApiClient, ApiClientError};
use crate::auth::use_auth;
use crate::components::ErrorBanner;
use crate::routes::AppRoute;
use crate::utils::{navigate, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileView, CreateSessionRequest, CreateTaskRequest, ErrorCode,
    PersistedTaskEvent, ProgressSnapshot, ResumeTaskRequest, SessionSummary, SseConnectionState,
    TaskDetail, TaskStatus, TaskSummary, UpdateSessionProfileRequest,
};
use std::collections::HashMap;

use super::activity::{ActivityDrawer, ActivityStatusChip};
use super::composer::{
    append_pending_browser_files, browser_files, browser_files_from_input_event, can_submit_input,
    handle_composer_drag, handle_composer_drop, handle_composer_input, handle_composer_paste,
    persist_default_effort, submit_parent_form_on_ctrl_enter, AgentEffortSelect,
    AgentProfileSelect, PendingAttachmentFile, PendingAttachmentList,
};
use super::profile::{
    agent_effort_from_value, agent_profile_selection_from_value, apply_loaded_default_effort,
    profile_value_to_id, PROFILE_VALUE_DEFAULT, PROFILE_VALUE_NONE,
};
use super::state::{
    latest_editable_task_id, latest_task, session_detail_to_summary, summary_to_detail,
    upsert_session_summary, upsert_task_summary,
};
use super::streaming::{start_task_stream, StreamUiSignals};
use super::task_card::{TaskCard, TaskCardModel, TaskCardSignals};
use super::versions::group_task_versions;

const TASK_EVENTS_PAGE_LIMIT: usize = 500;

async fn load_all_task_events(
    client: &ApiClient,
    session_id: &str,
    task_id: &str,
) -> Result<Vec<PersistedTaskEvent>, ApiClientError> {
    let mut after_seq = 0;
    let mut events = Vec::new();

    loop {
        let response = client
            .task_events_page(session_id, task_id, after_seq, TASK_EVENTS_PAGE_LIMIT)
            .await?;
        let next_seq = response.last_seq;
        let has_more = response.has_more;
        events.extend(response.events);

        if !has_more || next_seq <= after_seq {
            break;
        }
        after_seq = next_seq;
    }

    Ok(events)
}

fn merge_task_events(
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    new_events: Vec<PersistedTaskEvent>,
) {
    set_events.update(|items| {
        for event in new_events {
            if !items
                .iter()
                .any(|item| item.task_id == event.task_id && item.seq == event.seq)
            {
                items.push(event);
            }
        }
        items.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
                .then_with(|| a.seq.cmp(&b.seq))
        });
    });
}

fn max_event_seq(events: &[PersistedTaskEvent]) -> u64 {
    events
        .iter()
        .map(|event| event.seq)
        .max()
        .unwrap_or_default()
}

#[component]
pub fn TaskConsole(
    route: AppRoute,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
) -> impl IntoView {
    match route {
        AppRoute::Session(session_id) => view! {
            <SessionWorkspace
                session_id=session_id
                events=events
                progress=progress
                set_events=set_events
                set_sse_state=set_sse_state
                set_progress=set_progress
                set_sessions=set_sessions
            />
        }
        .into_any(),
        _ => view! {
            <WelcomeView set_sessions=set_sessions />
        }
        .into_any(),
    }
}

#[component]
fn WelcomeView(set_sessions: WriteSignal<Vec<SessionSummary>>) -> impl IntoView {
    let auth = use_auth();
    let (input, set_input) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (error, set_error) = signal(None::<String>);
    let (pending_files, set_pending_files) = signal(Vec::<PendingAttachmentFile>::new());
    let (next_pending_file_id, set_next_pending_file_id) = signal(0_usize);
    let (drag_active, set_drag_active) = signal(false);
    let (profiles, set_profiles) = signal(Vec::<AgentProfileView>::new());
    let (profiles_loaded, set_profiles_loaded) = signal(false);
    let (selected_profile, set_selected_profile) = signal(PROFILE_VALUE_DEFAULT.to_string());
    let (selected_effort, set_selected_effort) = signal(AgentEffort::Standard);
    let (effort_touched, set_effort_touched) = signal(false);

    Effect::new(move |_| {
        if profiles_loaded.get() {
            return;
        }
        set_profiles_loaded.set(true);
        spawn_ui(async move {
            let client = auth.client();
            if let Ok(settings) = client.settings().await {
                apply_loaded_default_effort(settings, effort_touched, set_selected_effort);
            }
            if let Ok(response) = client.list_agent_profiles().await {
                set_profiles.set(response.profiles);
            }
        });
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        let files = pending_files.get();
        if !can_submit_input(&text, &files) {
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        let agent_profile_selection = agent_profile_selection_from_value(&selected_profile.get());
        let effort = selected_effort.get();
        spawn_ui(async move {
            let client = auth.client();
            // 1. Create session
            let session_id = match client
                .create_session(&CreateSessionRequest {
                    model_selection: None,
                    agent_profile_selection,
                })
                .await
            {
                Ok(resp) => {
                    let session_id = resp.session.session_id.clone();
                    upsert_session_summary(set_sessions, resp.session);
                    session_id
                }
                Err(e) => {
                    set_error.set(Some(e.to_string()));
                    set_loading.set(false);
                    return;
                }
            };
            let attachments = if files.is_empty() {
                Vec::new()
            } else {
                match client
                    .upload_task_attachments(&session_id, &browser_files(&files))
                    .await
                {
                    Ok(response) => response.attachments,
                    Err(error) => {
                        set_error.set(Some(error.to_string()));
                        set_loading.set(false);
                        return;
                    }
                }
            };
            // 2. Create task with the user's message
            match client
                .create_task(
                    &session_id,
                    &CreateTaskRequest {
                        input_markdown: text,
                        attachments,
                        effort: Some(effort),
                    },
                )
                .await
            {
                Ok(_) => {
                    set_input.set(String::new());
                    set_pending_files.set(Vec::new());
                    navigate(&format!("/app/session/{session_id}"));
                }
                Err(e) => {
                    set_error.set(Some(e.to_string()));
                    set_loading.set(false);
                }
            }
        });
    };

    view! {
        <ErrorBanner message=error />
        <section class="welcome-view">
            <div class="welcome-view-content">
                <svg
                    width="40"
                    height="40"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1.5"
                    class="welcome-view-icon"
                >
                    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>
                </svg>
                <h2 class="welcome-view-title">"What can I help you with?"</h2>
                <p class="welcome-view-text">"Send a message to start a new agent session."</p>
                <form class="welcome-view-composer" on:submit=submit>
                    <div
                        class="composer-inner"
                        class:drag-active=drag_active
                        on:dragenter=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, true);
                        }
                        on:dragover=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, true);
                        }
                        on:dragleave=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, false);
                        }
                        on:drop=move |ev| {
                            handle_composer_drop(
                                &ev,
                                set_drag_active,
                                next_pending_file_id,
                                set_next_pending_file_id,
                                set_pending_files,
                            );
                        }
                    >
                        <textarea
                            placeholder="Message Oxide Agent…"
                            prop:value=input
                            disabled=loading
                            on:input=move |ev| {
                                handle_composer_input(&ev, set_input);
                            }
                            on:paste=move |ev| {
                                handle_composer_paste(
                                    &ev,
                                    next_pending_file_id,
                                    set_next_pending_file_id,
                                    set_pending_files,
                                );
                            }
                            on:keydown=move |ev| {
                                submit_parent_form_on_ctrl_enter(&ev);
                            }
                        />
                        <PendingAttachmentList
                            attachments=pending_files
                            set_attachments=set_pending_files
                        />
                        <div class="composer-footer">
                            <div class="composer-actions" class:btn-hidden=move || !can_submit_input(&input.get(), &pending_files.get())>
                                <AgentProfileSelect
                                    profiles=profiles
                                    selected_profile=selected_profile
                                    disabled=Signal::derive(move || loading.get())
                                    include_default=true
                                    on_change=Callback::new(move |ev| {
                                        set_selected_profile.set(event_target_value(&ev));
                                    })
                                />
                                <AgentEffortSelect
                                    selected_effort=selected_effort
                                    disabled=Signal::derive(move || loading.get())
                                    on_change=Callback::new(move |ev| {
                                        let effort = agent_effort_from_value(&event_target_value(&ev));
                                        set_effort_touched.set(true);
                                        set_selected_effort.set(effort);
                                        persist_default_effort(auth, effort, set_error);
                                    })
                                />
                                <label class="button secondary composer-attach-button">
                                    <input
                                        class="composer-file-input"
                                        type="file"
                                        multiple
                                        disabled=loading
                                        on:change=move |ev| {
                                            append_pending_browser_files(
                                                next_pending_file_id,
                                                set_next_pending_file_id,
                                                set_pending_files,
                                                browser_files_from_input_event(&ev),
                                            );
                                        }
                                    />
                                    "Attach"
                                </label>
                                <button
                                    type="submit"
                                    disabled=move || loading.get() || !can_submit_input(&input.get(), &pending_files.get())
                                    class="btn-primary"
                                >
                                    "Send"
                                </button>
                            </div>
                        </div>
                    </div>
                </form>
            </div>
        </section>
    }
}

#[component]
fn SessionWorkspace(
    session_id: String,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
) -> impl IntoView {
    let auth = use_auth();
    let (_session_title, set_session_title) = signal("Session".to_string());
    let (tasks, set_tasks) = signal(Vec::<TaskSummary>::new());
    let (input, set_input) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (active_task, set_active_task) = signal(None::<TaskDetail>);
    let (streaming_task_id, set_streaming_task_id) = signal(None::<String>);
    let (loaded, set_loaded) = signal(false);
    let (_last_terminal_status, set_last_terminal_status) = signal(None::<TaskStatus>);
    let (selected_versions, set_selected_versions) = signal(HashMap::<String, String>::new());
    let (pending_files, set_pending_files) = signal(Vec::<PendingAttachmentFile>::new());
    let (next_pending_file_id, set_next_pending_file_id) = signal(0_usize);
    let (drag_active, set_drag_active) = signal(false);
    let (profiles, set_profiles) = signal(Vec::<AgentProfileView>::new());
    let (profiles_loaded, set_profiles_loaded) = signal(false);
    let (selected_profile, set_selected_profile) = signal(PROFILE_VALUE_NONE.to_string());
    let (selected_effort, set_selected_effort) = signal(AgentEffort::Standard);
    let (effort_touched, set_effort_touched) = signal(false);

    let (drawer_open, set_drawer_open) = signal(false);

    Effect::new(move |_| {
        if profiles_loaded.get() {
            return;
        }
        set_profiles_loaded.set(true);
        spawn_ui(async move {
            let client = auth.client();
            if let Ok(settings) = client.settings().await {
                apply_loaded_default_effort(settings, effort_touched, set_selected_effort);
            }
            if let Ok(response) = client.list_agent_profiles().await {
                set_profiles.set(response.profiles);
            }
        });
    });

    let session_id_for_load = session_id.clone();
    let load_all = move || {
        set_loading.set(true);
        set_error.set(None);
        // Clear stale state before loading
        set_events.set(Vec::new());
        set_progress.set(None);
        set_active_task.set(None);
        set_streaming_task_id.set(None);
        set_selected_versions.set(HashMap::new());
        let session_id = session_id_for_load.clone();
        spawn_ui(async move {
            let client = auth.client();
            match client.get_session(&session_id).await {
                Ok(response) => {
                    set_session_title.set(response.session.title.clone());
                    set_selected_profile.set(
                        response
                            .session
                            .agent_profile_id
                            .clone()
                            .unwrap_or_else(|| PROFILE_VALUE_NONE.to_string()),
                    );
                    upsert_session_summary(
                        set_sessions,
                        session_detail_to_summary(response.session),
                    );
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            match client.list_tasks(&session_id).await {
                Ok(response) => {
                    set_drawer_open.set(false);
                    let latest = latest_task(&response.tasks);
                    set_tasks.set(response.tasks);
                    if let Some(task) = latest {
                        let task_id = task.task_id.clone();
                        let task_detail = match client.get_task(&session_id, &task_id).await {
                            Ok(response) => Some(response.task),
                            Err(error) => {
                                set_error.set(Some(error.to_string()));
                                None
                            }
                        };
                        let initial_last_seq =
                            match load_all_task_events(&client, &session_id, &task_id).await {
                                Ok(events) => {
                                    let last_seq = max_event_seq(&events);
                                    merge_task_events(set_events, events);
                                    last_seq
                                }
                                Err(error) => {
                                    set_error.set(Some(error.to_string()));
                                    0
                                }
                            };
                        if let Some(task) = task_detail {
                            set_progress.set(task.last_progress.clone());
                            if matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
                                set_active_task.set(Some(task));
                                start_task_stream(
                                    client.clone(),
                                    session_id.clone(),
                                    task_id.clone(),
                                    initial_last_seq,
                                    StreamUiSignals {
                                        set_events,
                                        set_session_title,
                                        set_progress,
                                        set_active_task,
                                        set_tasks,
                                        set_sse_state,
                                        set_error,
                                        streaming_task_id,
                                        set_streaming_task_id,
                                        set_last_terminal_status,
                                        set_sessions,
                                    },
                                );
                            } else if task.status == TaskStatus::WaitingForUserInput {
                                set_active_task.set(Some(task));
                            } else {
                                set_active_task.set(None);
                            }
                        }
                    } else {
                        // Empty session — clear signals
                        set_events.set(Vec::new());
                        set_progress.set(None);
                        set_active_task.set(None);
                    }
                }
                Err(error) => set_error.set(Some(task_submit_error_message(&error))),
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !loaded.get() {
            set_loaded.set(true);
            load_all();
        }
    });

    let session_id_for_submit = session_id.clone();
    let session_id_for_profile = session_id.clone();
    let update_profile = move |ev: leptos::ev::Event| {
        let value = event_target_value(&ev);
        set_selected_profile.set(value.clone());
        let session_id = session_id_for_profile.clone();
        set_error.set(None);
        spawn_ui(async move {
            let request = UpdateSessionProfileRequest {
                agent_profile_id: profile_value_to_id(&value),
            };
            match auth
                .client()
                .update_session_profile(&session_id, &request)
                .await
            {
                Ok(response) => {
                    set_selected_profile.set(
                        response
                            .session
                            .agent_profile_id
                            .clone()
                            .unwrap_or_else(|| PROFILE_VALUE_NONE.to_string()),
                    );
                    upsert_session_summary(
                        set_sessions,
                        session_detail_to_summary(response.session),
                    );
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
        });
    };

    let submit_task = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        let files = pending_files.get();
        if !can_submit_input(&text, &files) {
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        // Clear stale activity for the new task
        set_events.set(Vec::new());
        set_progress.set(None);
        let session_id = session_id_for_submit.clone();
        let effort = selected_effort.get();
        spawn_ui(async move {
            let client = auth.client();
            let attachments = if files.is_empty() {
                Vec::new()
            } else {
                match client
                    .upload_task_attachments(&session_id, &browser_files(&files))
                    .await
                {
                    Ok(response) => response.attachments,
                    Err(error) => {
                        set_error.set(Some(error.to_string()));
                        set_loading.set(false);
                        return;
                    }
                }
            };
            let resume_task_id = active_task
                .get()
                .filter(|task| task.status == TaskStatus::WaitingForUserInput)
                .map(|task| task.task_id);
            let result = match resume_task_id.as_deref() {
                Some(task_id) => client
                    .resume_task(
                        &session_id,
                        task_id,
                        &ResumeTaskRequest {
                            input_markdown: text,
                            attachments,
                            effort: Some(effort),
                        },
                    )
                    .await
                    .map(|response| response.task),
                _ => client
                    .create_task(
                        &session_id,
                        &CreateTaskRequest {
                            input_markdown: text,
                            attachments,
                            effort: Some(effort),
                        },
                    )
                    .await
                    .map(|response| response.task),
            };

            match result {
                Ok(task) => {
                    set_drawer_open.set(false);
                    set_input.set(String::new());
                    set_pending_files.set(Vec::new());
                    set_active_task.set(Some(summary_to_detail(&session_id, &task)));
                    set_last_terminal_status.set(None);
                    set_selected_versions.update(|items| {
                        items.insert(
                            task.effective_version_group_id().to_string(),
                            task.task_id.clone(),
                        );
                    });
                    start_task_stream(
                        client,
                        session_id.clone(),
                        task.task_id.clone(),
                        0,
                        StreamUiSignals {
                            set_events,
                            set_session_title,
                            set_progress,
                            set_active_task,
                            set_tasks,
                            set_sse_state,
                            set_error,
                            streaming_task_id,
                            set_streaming_task_id,
                            set_last_terminal_status,
                            set_sessions,
                        },
                    );
                    let task_summary = task.clone();
                    set_tasks.update(|items| upsert_task_summary(items, task_summary));
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let session_id_for_cancel = session_id.clone();
    let cancel_active = move |_| {
        let Some(task) = active_task.get() else {
            return;
        };
        set_loading.set(true);
        set_error.set(None);
        let session_id = session_id_for_cancel.clone();
        spawn_ui(async move {
            let client = auth.client();
            match client.cancel_task(&session_id, &task.task_id).await {
                Ok(_) => {
                    let task_id = task.task_id.clone();
                    set_active_task.set(None);
                    if streaming_task_id.get_untracked().as_deref() == Some(task_id.as_str()) {
                        set_streaming_task_id.set(None);
                    }
                    set_tasks.update(|items| {
                        for item in items {
                            if item.task_id == task_id {
                                item.status = TaskStatus::Cancelled;
                            }
                        }
                    });
                    if let Ok(response) = client.get_session(&session_id).await {
                        upsert_session_summary(
                            set_sessions,
                            session_detail_to_summary(response.session),
                        );
                    }
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let session_id_for_cards = session_id.clone();

    let is_waiting = move || {
        active_task
            .get()
            .is_some_and(|task| task.status == TaskStatus::WaitingForUserInput)
    };

    let is_running = move || {
        active_task
            .get()
            .is_some_and(|task| matches!(task.status, TaskStatus::Queued | TaskStatus::Running))
    };

    view! {
        <ErrorBanner message=error />
        <section class="session-workspace">
            <div class="chat-wrapper"
                class=("welcome-mode", move || tasks.get().is_empty() && !loading.get())
            >
                // Agent results — task cards with output
                <div class="results-panel">
                    {move || {
                        if loading.get() && tasks.get().is_empty() {
                            view! { <div class="empty-state">"Loading..."</div> }.into_any()
                        } else if tasks.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <div class="empty-state-title">"What can I help you with?"</div>
                                    <div class="empty-state-text">
                                        "Send a message to start a new agent session."
                                    </div>
                                </div>
                            }
                            .into_any()
                        } else {
                            let latest_editable_task_id = latest_editable_task_id(&tasks.get());
                            let session_id_for_cards = session_id_for_cards.clone();
                            view! {
                                <For
                                    each=move || group_task_versions(&tasks.get())
                                    key=|group| group.version_group_id.clone()
                                    children=move |group| {
                                        view! {
                                            <TaskCard
                                                model=TaskCardModel {
                                                    session_id: session_id_for_cards.clone(),
                                                    versions: group.versions,
                                                    editable_task_id: latest_editable_task_id.clone(),
                                                }
                                                signals=TaskCardSignals {
                                                    events,
                                                    selected_versions,
                                                    set_selected_versions,
                                                    drawer_open,
                                                    set_drawer_open,
                                                    stream_signals: StreamUiSignals {
                                                        set_events,
                                                        set_session_title,
                                                        set_progress,
                                                        set_active_task,
                                                        set_tasks,
                                                        set_sse_state,
                                                        set_error,
                                                        streaming_task_id,
                                                        set_streaming_task_id,
                                                        set_last_terminal_status,
                                                        set_sessions,
                                                    },
                                                    set_error,
                                                }
                                            />
                                        }
                                    }
                                />
                            }
                            .into_any()
                        }
                    }}
                    <ActivityStatusChip
                        tasks=tasks
                        active_task=active_task
                        open=drawer_open
                        set_open=set_drawer_open
                    />
                </div>

                // Prompt input
                <form class="composer" on:submit=submit_task>
                    <ComposerNotice active_task=active_task />
                    <div
                        class="composer-inner"
                        class:drag-active=drag_active
                        on:dragenter=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, true);
                        }
                        on:dragover=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, true);
                        }
                        on:dragleave=move |ev| {
                            handle_composer_drag(&ev, set_drag_active, false);
                        }
                        on:drop=move |ev| {
                            handle_composer_drop(
                                &ev,
                                set_drag_active,
                                next_pending_file_id,
                                set_next_pending_file_id,
                                set_pending_files,
                            );
                        }
                    >
                        <textarea
                            placeholder=move || if is_running() { "Agent is working…" } else if is_waiting() { "Reply to resume the task…" } else { "Message Oxide Agent…" }
                            prop:value=input
                            disabled=is_running
                            on:input=move |ev| {
                                handle_composer_input(&ev, set_input);
                            }
                            on:paste=move |ev| {
                                handle_composer_paste(
                                    &ev,
                                    next_pending_file_id,
                                    set_next_pending_file_id,
                                    set_pending_files,
                                );
                            }
                            on:keydown=move |ev| {
                                submit_parent_form_on_ctrl_enter(&ev);
                            }
                        />
                        <PendingAttachmentList
                            attachments=pending_files
                            set_attachments=set_pending_files
                        />
                        <div class="composer-footer">
                            <div class="composer-actions" class:btn-hidden=move || !can_submit_input(&input.get(), &pending_files.get()) && !is_waiting()>
                                <AgentProfileSelect
                                    profiles=profiles
                                    selected_profile=selected_profile
                                    disabled=Signal::derive(move || loading.get() || is_running() || is_waiting())
                                    include_default=false
                                    on_change=Callback::new(update_profile)
                                />
                                <AgentEffortSelect
                                    selected_effort=selected_effort
                                    disabled=Signal::derive(move || loading.get() || is_running())
                                    on_change=Callback::new(move |ev| {
                                        let effort = agent_effort_from_value(&event_target_value(&ev));
                                        set_effort_touched.set(true);
                                        set_selected_effort.set(effort);
                                        persist_default_effort(auth, effort, set_error);
                                    })
                                />
                                <label class="button secondary composer-attach-button">
                                    <input
                                        class="composer-file-input"
                                        type="file"
                                        multiple
                                        disabled=move || loading.get() || is_running()
                                        on:change=move |ev| {
                                            append_pending_browser_files(
                                                next_pending_file_id,
                                                set_next_pending_file_id,
                                                set_pending_files,
                                                browser_files_from_input_event(&ev),
                                            );
                                        }
                                    />
                                    "Attach"
                                </label>
                                <button
                                    type="submit"
                                    disabled=move || loading.get() || is_running() || (!can_submit_input(&input.get(), &pending_files.get()) && !is_waiting())
                                    class="btn-primary"
                                    style=move || if is_running() { "display:none" } else { "" }
                                >
                                    {move || {
                                        if is_waiting() { "Resume" } else { "Send" }
                                    }}
                                </button>
                                <button
                                    class="btn-danger"
                                    type="button"
                                    style=move || if is_running() { "" } else { "display:none" }
                                    on:click=cancel_active
                                >
                                    "Stop"
                                </button>
                            </div>
                        </div>
                    </div>
                </form>
            </div>
            <ActivityDrawer
                open=drawer_open
                set_open=set_drawer_open
                tasks=tasks
                active_task=active_task
                events=events
                progress=progress
            />
        </section>
    }
}

#[component]
fn ComposerNotice(active_task: ReadSignal<Option<TaskDetail>>) -> impl IntoView {
    view! {
        {move || match active_task.get().map(|task| task.status) {
            Some(TaskStatus::WaitingForUserInput) => view! {
                <p class="composer-notice waiting">"The task is waiting for your reply. Sending will resume the same task."</p>
            }.into_any(),
            _ => ().into_any(),
        }}
    }
}

fn task_submit_error_message(error: &crate::api::ApiClientError) -> String {
    match error.error_code() {
        Some(ErrorCode::SessionBusy) => {
            "This session already has an active task. Stop it or wait for it to finish.".to_string()
        }
        Some(ErrorCode::TaskWaitingForUserInput) => {
            "The active task is waiting for input. Reply in the composer to resume it.".to_string()
        }
        _ => error.to_string(),
    }
}
