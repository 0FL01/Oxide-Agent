use crate::auth::{use_auth, AuthContext};
use crate::components::ErrorBanner;
use crate::markdown::MarkdownContent;
use crate::routes::AppRoute;
use crate::sse::{spawn_task_stream, TaskStreamConfig};
use crate::utils::{navigate, spawn_ui};
use leptos::{html, prelude::*};
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileSelection, AgentProfileView, CreateSessionRequest, CreateTaskRequest,
    CreateTaskVersionRequest, ErrorCode, PersistedTaskEvent, ProgressSnapshot, ResumeTaskRequest,
    SessionDetail, SessionSummary, SseConnectionState, TaskAttachment, TaskDetail, TaskEventKind,
    TaskStatus, TaskSummary, UpdateSessionProfileRequest, UpdateUserSettingsRequest,
    UserMessageEventPayload, UserSettingsResponse,
};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

#[derive(Clone)]
struct PendingAttachmentFile {
    id: usize,
    file: web_sys::File,
}

const PROFILE_VALUE_DEFAULT: &str = "__default__";
const PROFILE_VALUE_NONE: &str = "__none__";

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
                            ev.prevent_default();
                            set_drag_active.set(true);
                        }
                        on:dragover=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(true);
                        }
                        on:dragleave=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(false);
                        }
                        on:drop=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(false);
                            append_pending_browser_files(
                                next_pending_file_id,
                                set_next_pending_file_id,
                                set_pending_files,
                                browser_files_from_drag_event(&ev),
                            );
                        }
                    >
                        <textarea
                            placeholder="Message Oxide Agent…"
                            prop:value=input
                            disabled=loading
                            on:input=move |ev| {
                                set_input.set(event_target_value(&ev));
                                use wasm_bindgen::JsCast;
                                let Some(target) = ev.target() else {
                                    return;
                                };
                                let el: web_sys::HtmlElement = target.unchecked_into();
                                el.style().set_property("height", "auto").ok();
                                let scroll = el.scroll_height();
                                let max = 208.0_f64;
                                let h = (scroll as f64).min(max);
                                el.style().set_property("height", &format!("{h}px")).ok();
                            }
                            on:paste=move |ev| {
                                append_pasted_image_files(
                                    &ev,
                                    next_pending_file_id,
                                    set_next_pending_file_id,
                                    set_pending_files,
                                );
                            }
                            on:keydown=move |ev| {
                                if ev.ctrl_key() && ev.key() == "Enter" {
                                    ev.prevent_default();
                                    if let Some(target) = ev.target() {
                                        use wasm_bindgen::JsCast;
                                        let el: web_sys::HtmlElement = target.unchecked_into();
                                        if let Ok(Some(form_el)) = el.closest("form") {
                                            if let Ok(Some(btn)) = form_el.query_selector("button[type=submit]") {
                                                let btn: web_sys::HtmlElement = btn.unchecked_into();
                                                btn.click();
                                            }
                                        }
                                    }
                                }
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
                    let latest = latest_task(response.tasks.clone());
                    set_tasks.set(response.tasks);
                    if let Some(task) = latest {
                        let task_id = task.task_id.clone();
                        if let Ok(response) = client.get_task(&session_id, &task_id).await {
                            set_progress.set(response.task.last_progress.clone());
                            if matches!(
                                response.task.status,
                                TaskStatus::Queued | TaskStatus::Running
                            ) {
                                set_active_task.set(Some(response.task));
                                start_task_stream(
                                    client.clone(),
                                    session_id.clone(),
                                    task_id.clone(),
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
                            } else if response.task.status == TaskStatus::WaitingForUserInput {
                                set_active_task.set(Some(response.task));
                            } else {
                                set_active_task.set(None);
                            }
                        }
                        match client.task_events(&session_id, &task_id, 0).await {
                            Ok(response) => set_events.set(response.events),
                            Err(error) => set_error.set(Some(error.to_string())),
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
                                                session_id=session_id_for_cards.clone()
                                                versions=group.versions
                                                events=events
                                                editable_task_id=latest_editable_task_id.clone()
                                                selected_versions=selected_versions
                                                set_selected_versions=set_selected_versions
                                                drawer_open=drawer_open
                                                set_drawer_open=set_drawer_open
                                                stream_signals=StreamUiSignals {
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
                                                }
                                                set_error=set_error
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
                            ev.prevent_default();
                            set_drag_active.set(true);
                        }
                        on:dragover=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(true);
                        }
                        on:dragleave=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(false);
                        }
                        on:drop=move |ev| {
                            ev.prevent_default();
                            set_drag_active.set(false);
                            append_pending_browser_files(
                                next_pending_file_id,
                                set_next_pending_file_id,
                                set_pending_files,
                                browser_files_from_drag_event(&ev),
                            );
                        }
                    >
                        <textarea
                            placeholder=move || if is_running() { "Agent is working…" } else if is_waiting() { "Reply to resume the task…" } else { "Message Oxide Agent…" }
                            prop:value=input
                            disabled=is_running
                            on:input=move |ev| {
                                set_input.set(event_target_value(&ev));
                                // auto-resize
                                use wasm_bindgen::JsCast;
                                let Some(target) = ev.target() else {
                                    return;
                                };
                                let el: web_sys::HtmlElement = target.unchecked_into();
                                el.style().set_property("height", "auto").ok();
                                let scroll = el.scroll_height();
                                let max = 208.0_f64;
                                let h = (scroll as f64).min(max);
                                el.style().set_property("height", &format!("{h}px")).ok();
                            }
                            on:paste=move |ev| {
                                append_pasted_image_files(
                                    &ev,
                                    next_pending_file_id,
                                    set_next_pending_file_id,
                                    set_pending_files,
                                );
                            }
                            on:keydown=move |ev| {
                                if ev.ctrl_key() && ev.key() == "Enter" {
                                    ev.prevent_default();
                                    if let Some(target) = ev.target() {
                                        use wasm_bindgen::JsCast;
                                        let el: web_sys::HtmlElement = target.unchecked_into();
                                        if let Ok(Some(form_el)) = el.closest("form") {
                                            if let Ok(Some(btn)) = form_el.query_selector("button[type=submit]") {
                                                let btn: web_sys::HtmlElement = btn.unchecked_into();
                                                btn.click();
                                            }
                                        }
                                    }
                                }
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

#[component]
fn ActivityStatusChip(
    tasks: ReadSignal<Vec<TaskSummary>>,
    active_task: ReadSignal<Option<TaskDetail>>,
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        {move || {
            let Some(status) = latest_activity_status(active_task, tasks) else {
                return ().into_any();
            };
            if status == TaskStatus::Completed {
                return ().into_any();
            }
            let class = match status {
                TaskStatus::Queued | TaskStatus::Running => "status-chip active",
                TaskStatus::WaitingForUserInput => "status-chip active waiting",
                TaskStatus::Failed | TaskStatus::Interrupted => "status-chip warning",
                TaskStatus::Cancelled => "status-chip error",
                TaskStatus::Completed => "status-chip",
            };
            let label = match status {
                TaskStatus::Queued | TaskStatus::Running => "Thinking",
                TaskStatus::WaitingForUserInput => "Waiting for your input",
                TaskStatus::Failed => "Task failed",
                TaskStatus::Cancelled => "Cancelled",
                TaskStatus::Interrupted => "Interrupted",
                TaskStatus::Completed => "Completed",
            };
            view! {
                <div class="status-wrap">
                    <button class=move || if open.get() { format!("{class} open") } else { class.to_string() } type="button" on:click=move |_| toggle_drawer(open, set_open)>
                        <span class="dot"></span>
                        <span>{label}</span>
                        <span class="chevron">"›"</span>
                    </button>
                </div>
            }.into_any()
        }}
    }
}

#[component]
fn ThinkingButton(
    label: String,
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
) -> impl IntoView {
    view! {
        <button class=move || if open.get() { "thinking-button open" } else { "thinking-button" } type="button" on:click=move |_| toggle_drawer(open, set_open)>
            <span class="dot"></span>
            <span>{label}</span>
            <span class="chevron">"›"</span>
        </button>
    }
}

fn toggle_drawer(open: ReadSignal<bool>, set_open: WriteSignal<bool>) {
    set_open.set(!open.get());
}

#[component]
fn ActivityDrawer(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    tasks: ReadSignal<Vec<TaskSummary>>,
    active_task: ReadSignal<Option<TaskDetail>>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    progress: ReadSignal<Option<ProgressSnapshot>>,
) -> impl IntoView {
    let (elapsed_now_millis, set_elapsed_now_millis) =
        signal(browser_now_millis().unwrap_or_default());
    if let Ok(handle) = set_interval_with_handle(
        move || {
            let next = browser_now_millis()
                .unwrap_or_else(|| elapsed_now_millis.get_untracked().saturating_add(1_000));
            set_elapsed_now_millis.set(next);
        },
        Duration::from_secs(1),
    ) {
        on_cleanup(move || handle.clear());
    }

    view! {
        <aside class=move || if open.get() && latest_activity_task_id(active_task, tasks).is_some() { "activity-drawer open" } else { "activity-drawer" }>
            <header class="activity-header">
                <div class="activity-title-row">
                    <span class="activity-title">"Activity"</span>
                    {move || latest_activity_elapsed_label(active_task, tasks, elapsed_now_millis).map(|elapsed| view! {
                        <span class="activity-title-separator">"·"</span>
                        <span class="activity-elapsed">{elapsed}</span>
                    })}
                </div>
                <button class="activity-close" type="button" on:click=move |_| set_open.set(false)>"×"</button>
            </header>
            <ContextCard progress=progress />
            <div class="activity-timeline">
                {move || {
                    let Some(task_id) = latest_activity_task_id(active_task, tasks) else {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    };
                    let task_events: Vec<PersistedTaskEvent> = events
                        .get()
                        .into_iter()
                        .filter(|event| event.task_id == task_id)
                        .filter(|event| is_chat_visible_event(&event.kind))
                        .filter(is_useful_event)
                        .collect();
                    let live_owner = active_task
                        .get()
                        .is_some_and(|task| task.task_id == task_id);
                    let todos = if live_owner {
                        progress.get().and_then(|snapshot| snapshot.current_todos)
                    } else {
                        None
                    };
                    let items = group_activity_events(task_events);
                    if items.is_empty() && todos.is_none() {
                        return view! { <div class="activity-empty">"No activity yet."</div> }.into_any();
                    }
                    view! {
                        {todos.map(|value| view! { <TodosCard todos=value /> })}
                        {items.into_iter().map(|item| view! { <ActivityItemCard item=item /> }).collect::<Vec<_>>()}
                    }.into_any()
                }}
            </div>
        </aside>
    }
}

fn compact_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    let value = tokens as f64 / 1000.0;
    format!("{value:.1}k")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn latest_activity_task_id(
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
) -> Option<String> {
    active_task.get().map(|task| task.task_id).or_else(|| {
        tasks
            .get()
            .into_iter()
            .max_by_key(|task| task.updated_at)
            .map(|task| task.task_id)
    })
}

fn latest_activity_status(
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
) -> Option<TaskStatus> {
    active_task.get().map(|task| task.status).or_else(|| {
        tasks
            .get()
            .into_iter()
            .max_by_key(|task| task.updated_at)
            .map(|task| task.status)
    })
}

#[derive(Clone, Copy)]
struct ActivityTiming {
    status: TaskStatus,
    created_at_ms: i64,
    started_at_ms: Option<i64>,
    updated_at_ms: i64,
    finished_at_ms: Option<i64>,
}

impl From<&TaskSummary> for ActivityTiming {
    fn from(task: &TaskSummary) -> Self {
        Self {
            status: task.status,
            created_at_ms: task.created_at.timestamp_millis(),
            started_at_ms: task.started_at.map(|value| value.timestamp_millis()),
            updated_at_ms: task.updated_at.timestamp_millis(),
            finished_at_ms: task.finished_at.map(|value| value.timestamp_millis()),
        }
    }
}

impl From<&TaskDetail> for ActivityTiming {
    fn from(task: &TaskDetail) -> Self {
        Self {
            status: task.status,
            created_at_ms: task.created_at.timestamp_millis(),
            started_at_ms: task.started_at.map(|value| value.timestamp_millis()),
            updated_at_ms: task.updated_at.timestamp_millis(),
            finished_at_ms: task.finished_at.map(|value| value.timestamp_millis()),
        }
    }
}

fn latest_activity_elapsed_label(
    active_task: ReadSignal<Option<TaskDetail>>,
    tasks: ReadSignal<Vec<TaskSummary>>,
    now_millis: ReadSignal<i64>,
) -> Option<String> {
    let timing = active_task
        .get()
        .map(|task| ActivityTiming::from(&task))
        .or_else(|| {
            tasks
                .get()
                .into_iter()
                .max_by_key(|task| task.updated_at)
                .map(|task| ActivityTiming::from(&task))
        })?;
    Some(format_duration(activity_elapsed_seconds(
        timing, now_millis,
    )))
}

fn activity_elapsed_seconds(timing: ActivityTiming, now_millis: ReadSignal<i64>) -> i64 {
    let start_ms = timing.started_at_ms.unwrap_or(timing.created_at_ms);
    let end_ms = if timing.status.is_terminal() {
        timing.finished_at_ms.unwrap_or(timing.updated_at_ms)
    } else {
        now_millis.get().max(timing.updated_at_ms)
    };
    end_ms.saturating_sub(start_ms) / 1_000
}

fn browser_now_millis() -> Option<i64> {
    let performance = web_sys::window()?.performance()?;
    let millis = performance.time_origin() + performance.now();
    millis.is_finite().then_some(millis.round() as i64)
}

fn thought_label(task: &TaskSummary) -> String {
    format!(
        "Thought for {}",
        format_duration(task_duration_seconds(task))
    )
}

fn task_duration_seconds(task: &TaskSummary) -> i64 {
    let start = task.started_at.as_ref().unwrap_or(&task.created_at);
    let end = task.finished_at.as_ref().unwrap_or(&task.updated_at);
    let seconds = end.signed_duration_since(start.to_owned()).num_seconds();
    seconds.max(0)
}

fn format_duration(total_seconds: i64) -> String {
    let seconds = total_seconds.max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        return format!("{hours}h {minutes}m {seconds}s");
    }
    if minutes > 0 {
        return format!("{minutes}m {seconds}s");
    }
    format!("{seconds}s")
}

#[derive(Clone, Copy)]
struct StreamUiSignals {
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_session_title: WriteSignal<String>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_active_task: WriteSignal<Option<TaskDetail>>,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_error: WriteSignal<Option<String>>,
    streaming_task_id: ReadSignal<Option<String>>,
    set_streaming_task_id: WriteSignal<Option<String>>,
    set_last_terminal_status: WriteSignal<Option<TaskStatus>>,
    set_sessions: WriteSignal<Vec<SessionSummary>>,
}

fn start_task_stream(
    client: crate::api::ApiClient,
    session_id: String,
    task_id: String,
    signals: StreamUiSignals,
) {
    signals.set_streaming_task_id.set(Some(task_id.clone()));
    spawn_task_stream(TaskStreamConfig {
        client,
        session_id,
        task_id,
        set_session_title: signals.set_session_title,
        set_sessions: signals.set_sessions,
        set_events: signals.set_events,
        set_progress: signals.set_progress,
        set_active_task: signals.set_active_task,
        set_tasks: signals.set_tasks,
        set_state: signals.set_sse_state,
        set_error: signals.set_error,
        streaming_task_id: signals.streaming_task_id,
        set_streaming_task_id: signals.set_streaming_task_id,
        set_last_terminal_status: signals.set_last_terminal_status,
    });
}

fn summary_to_detail(session_id: &str, task: &TaskSummary) -> TaskDetail {
    TaskDetail {
        task_id: task.task_id.clone(),
        session_id: session_id.to_string(),
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
        last_progress: None,
        last_event_seq: task.last_event_seq,
        created_at: task.created_at,
        started_at: task.started_at,
        updated_at: task.updated_at,
        finished_at: task.finished_at,
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
        model_selection: session.model_selection,
        agent_profile_id: session.agent_profile_id,
        last_preview: session.last_preview,
        active_task_id: session.active_task_id,
        last_task_status: session.last_task_status,
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

// ── Task Card ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskCard(
    session_id: String,
    versions: Vec<TaskSummary>,
    events: ReadSignal<Vec<PersistedTaskEvent>>,
    editable_task_id: Option<String>,
    selected_versions: ReadSignal<HashMap<String, String>>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    drawer_open: ReadSignal<bool>,
    set_drawer_open: WriteSignal<bool>,
    stream_signals: StreamUiSignals,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let version_group_id = versions
        .first()
        .map(|task| task.effective_version_group_id().to_string())
        .unwrap_or_default();
    let version_count = versions.len();
    let (editing, set_editing) = signal(false);
    let (draft, set_draft) = signal(
        versions
            .last()
            .map(|task| task.input_markdown.clone())
            .unwrap_or_default(),
    );
    let (saving, set_saving) = signal(false);
    let selected_task = Memo::new({
        let versions = versions.clone();
        let version_group_id = version_group_id.clone();
        move |_| {
            let selected_task_id = selected_versions.get().get(&version_group_id).cloned();
            let selected_index = selected_version_index(&versions, selected_task_id.as_deref());
            versions[selected_index].clone()
        }
    });

    Effect::new(move |_| {
        let task = selected_task.get();
        if !editing.get() {
            set_draft.set(task.input_markdown.clone());
        }
    });

    view! {
        {move || {
            let task = selected_task.get();
            let selected_index = versions
                .iter()
                .position(|candidate| candidate.task_id == task.task_id)
                .unwrap_or_else(|| version_count.saturating_sub(1));
            let editable = editable_task_id.as_ref() == Some(&task.task_id);
            let card_status_class = match task.status {
                TaskStatus::Running | TaskStatus::Queued => "running",
                TaskStatus::Completed => "success",
                TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Interrupted => "error",
                _ => "",
            };
            let card_class = format!("task-card {card_status_class}");
            let thought_label = thought_label(&task);
            let original_input = task.input_markdown.clone();
            let input_markdown = task.input_markdown.clone();
            let attachments = task.attachments.clone();
            let final_response_markdown = task.final_response_markdown.clone();
            let task_events = events.get();
            let resume_messages = resume_user_messages_for_task(&task_events, &task.task_id);
            let delivered_files = delivered_files_for_task(&task_events, &task.task_id);
            let can_select_previous = selected_index > 0;
            let can_select_next = selected_index + 1 < version_count;
            let version_counter = format!("{}/{}", selected_index + 1, version_count);
            let previous_task = can_select_previous.then(|| versions[selected_index - 1].clone());
            let next_task = can_select_next.then(|| versions[selected_index + 1].clone());

            view! {
                <article class=card_class>
                    <div class="message user-message-wrap">
                        <div class="user-message">
                        {if editing.get() {
                            let session_id = session_id.clone();
                            let task_id = task.task_id.clone();
                            let version_group_id = version_group_id.clone();
                            view! {
                                <TaskInputEditForm
                                    session_id=session_id
                                    task_id=task_id
                                    version_group_id=version_group_id
                                    original_input=original_input
                                    attachments=attachments.clone()
                                    draft=draft
                                    set_draft=set_draft
                                    saving=saving
                                    set_saving=set_saving
                                    set_editing=set_editing
                                    set_selected_versions=set_selected_versions
                                    set_drawer_open=set_drawer_open
                                    stream_signals=stream_signals
                                    set_error=set_error
                                />
                            }.into_any()
                        } else {
                            view! {
                                <UserMessageBody
                                    input_markdown=input_markdown.clone()
                                    attachments=attachments.clone()
                                />
                            }.into_any()
                        }}
                        </div>
                        <div class="user-message-actions" aria-label="User message actions">
                            {editable.then(|| view! {
                                <button
                                    class="message-action-button"
                                    type="button"
                                    title="Edit input"
                                    aria-label="Edit input"
                                    on:click=move |_| set_editing.set(true)
                                >
                                    "✎"
                                </button>
                            })}
                            <button
                                class="message-action-button"
                                type="button"
                                title="Copy user message"
                                aria-label="Copy user message"
                                on:click=move |_| {
                                    if let Some(window) = web_sys::window() {
                                        let _ = window.navigator().clipboard().write_text(&input_markdown);
                                    }
                                }
                            >
                                "⧉"
                            </button>
                            {(version_count > 1).then(|| {
                                let previous_version_group_id = version_group_id.clone();
                                let next_version_group_id = version_group_id.clone();
                                view! {
                                    <div class="message-version-switcher" aria-label="Task version history">
                                        <button
                                            class="message-action-button"
                                            type="button"
                                            title="Previous version"
                                            aria-label="Previous version"
                                            disabled=!can_select_previous
                                            on:click=move |_| {
                                                if let Some(previous_task) = previous_task.clone() {
                                                    set_editing.set(false);
                                                    set_draft.set(previous_task.input_markdown.clone());
                                                    set_selected_versions.update(|items| {
                                                        items.insert(previous_version_group_id.clone(), previous_task.task_id.clone());
                                                    });
                                                }
                                            }
                                        >
                                            "‹"
                                        </button>
                                        <div class="message-version-counter">{version_counter.clone()}</div>
                                        <button
                                            class="message-action-button"
                                            type="button"
                                            title="Next version"
                                            aria-label="Next version"
                                            disabled=!can_select_next
                                            on:click=move |_| {
                                                if let Some(next_task) = next_task.clone() {
                                                    set_editing.set(false);
                                                    set_draft.set(next_task.input_markdown.clone());
                                                    set_selected_versions.update(|items| {
                                                        items.insert(next_version_group_id.clone(), next_task.task_id.clone());
                                                    });
                                                }
                                            }
                                        >
                                            "›"
                                        </button>
                                    </div>
                                }
                            })}
                        </div>
                    </div>
                    {resume_messages
                        .into_iter()
                        .map(|message| {
                            view! {
                                <div class="message user-message-wrap">
                                    <div class="user-message">
                                        <UserMessageBody
                                            input_markdown=message.input_markdown
                                            attachments=message.attachments
                                        />
                                    </div>
                                </div>
                            }
                        })
                        .collect_view()}
                    {editable.then(|| view! {
                        <div class="task-action-row">
                            <ThinkingButton label=thought_label open=drawer_open set_open=set_drawer_open />
                        </div>
                    })}

                    {final_response_markdown.map(|answer| {
                        let raw_answer = answer.clone();
                        let rendered_answer =
                            linkify_delivered_files_in_markdown(&answer, &delivered_files);
                        view! {
                            <div class="message assistant-message-wrap">
                                <div class="assistant-message">
                                    <MarkdownContent markdown=rendered_answer />
                                </div>
                                <div class="assistant-message-actions" aria-label="Assistant message actions">
                                    <button
                                        class="message-action-button"
                                        type="button"
                                        title="Copy final response"
                                        aria-label="Copy final response"
                                        on:click=move |_| {
                                            if let Some(window) = web_sys::window() {
                                                let _ = window.navigator().clipboard().write_text(&raw_answer);
                                            }
                                        }
                                    >
                                        "⧉"
                                    </button>
                                </div>
                            </div>
                        }
                    })}
                    {(!delivered_files.is_empty()).then(|| view! {
                        <DeliveredFilesMessage files=delivered_files.clone() />
                    })}
                    {task.error_message.map(|error| view! {
                        <div class="message error-message">{error}</div>
                    })}
                    {task.pending_user_input.map(|pending| view! {
                        <div class="message pending-message">{pending.prompt}</div>
                    })}
                </article>
            }
                .into_any()
        }}
    }
}

#[component]
fn CollapsibleMessageBody(markdown: String) -> impl IntoView {
    let (expanded, set_expanded) = signal(false);
    let (overflowing, set_overflowing) = signal(false);
    let body_ref = NodeRef::<html::Div>::new();
    let measure_ref = body_ref;

    Effect::new(move |_| {
        let Some(body) = measure_ref.get() else {
            return;
        };
        if expanded.get() {
            return;
        }
        set_overflowing.set(body.scroll_height() > body.client_height() + 1);
    });

    view! {
        <div class="message-collapsible">
            <div
                class=move || {
                    if expanded.get() {
                        "message-collapsible-body is-expanded"
                    } else {
                        "message-collapsible-body is-collapsed"
                    }
                }
                node_ref=body_ref
            >
                <MarkdownContent markdown=markdown />
                {move || {
                    if overflowing.get() && !expanded.get() {
                        view! { <div class="message-collapsible-fade"></div> }.into_any()
                    } else {
                        ().into_any()
                    }
                }}
            </div>
            {move || {
                if overflowing.get() {
                    view! {
                        <button
                            class="message-expand-button secondary"
                            type="button"
                            on:click=move |_| set_expanded.update(|value| *value = !*value)
                        >
                            {move || if expanded.get() { "Show less" } else { "Show more" }}
                        </button>
                    }
                    .into_any()
                } else {
                    ().into_any()
                }
            }}
        </div>
    }
}

#[derive(Clone)]
struct ResumeUserMessage {
    input_markdown: String,
    attachments: Vec<TaskAttachment>,
}

#[component]
fn UserMessageBody(input_markdown: String, attachments: Vec<TaskAttachment>) -> impl IntoView {
    let rendered_input_markdown = input_markdown.clone();

    view! {
        <div class="user-message-body">
            <MessageAttachments attachments=attachments />
            {(!rendered_input_markdown.trim().is_empty()).then(|| view! {
                <CollapsibleMessageBody markdown=rendered_input_markdown />
            })}
        </div>
    }
}

fn resume_user_messages_for_task(
    events: &[PersistedTaskEvent],
    task_id: &str,
) -> Vec<ResumeUserMessage> {
    events
        .iter()
        .filter(|event| event.task_id == task_id && event.kind == TaskEventKind::UserMessage)
        .filter_map(|event| {
            serde_json::from_value::<UserMessageEventPayload>(event.payload.clone()).ok()
        })
        .map(|payload| ResumeUserMessage {
            input_markdown: payload.input_markdown,
            attachments: payload.attachments,
        })
        .collect()
}

fn delivered_files_for_task(
    events: &[PersistedTaskEvent],
    task_id: &str,
) -> Vec<DeliveredFileLink> {
    events
        .iter()
        .filter(|event| event.task_id == task_id)
        .filter_map(delivered_file_link)
        .collect()
}

fn linkify_delivered_files_in_markdown(markdown: &str, files: &[DeliveredFileLink]) -> String {
    if files.is_empty() {
        return markdown.to_string();
    }

    let mut result = String::new();
    let mut in_fenced_code_block = false;

    for segment in markdown.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        if trimmed.starts_with("```") {
            in_fenced_code_block = !in_fenced_code_block;
            result.push_str(segment);
            continue;
        }

        if in_fenced_code_block {
            result.push_str(segment);
            continue;
        }

        let mut rewritten = segment.to_string();
        for file in files {
            let code_span = format!("`{}`", file.file_name);
            let markdown_link = format!("[`{}`]({})", file.file_name, file.download_url);
            rewritten = rewritten.replace(&code_span, &markdown_link);
        }
        result.push_str(&rewritten);
    }

    if !markdown.ends_with('\n') {
        result.truncate(result.trim_end_matches('\n').len());
    }

    result
}

#[component]
fn AgentProfileSelect(
    profiles: ReadSignal<Vec<AgentProfileView>>,
    selected_profile: ReadSignal<String>,
    disabled: Signal<bool>,
    include_default: bool,
    on_change: Callback<leptos::ev::Event>,
) -> impl IntoView {
    view! {
        <select
            class="agent-profile-select"
            prop:value=selected_profile
            disabled=move || disabled.get()
            on:change=move |ev| on_change.run(ev)
        >
            {include_default.then(|| view! {
                <option value=PROFILE_VALUE_DEFAULT>"Default profile"</option>
            })}
            {move || selected_profile_missing_option(profiles, selected_profile)}
            <option value=PROFILE_VALUE_NONE>"No profile"</option>
            <For
                each=move || profiles.get()
                key=|profile| profile.agent_id.clone()
                children=move |profile| {
                    let value = profile.agent_id.clone();
                    view! { <option value=value.clone()>{profile.display_name}</option> }
                }
            />
        </select>
    }
}

#[component]
fn AgentEffortSelect(
    selected_effort: ReadSignal<AgentEffort>,
    disabled: Signal<bool>,
    on_change: Callback<leptos::ev::Event>,
) -> impl IntoView {
    view! {
        <select
            class="composer-effort-select"
            title="Effort controls agent loop depth and research budget"
            aria-label="Agent effort"
            prop:value=move || agent_effort_value(selected_effort.get())
            disabled=move || disabled.get()
            on:change=move |ev| on_change.run(ev)
        >
            <option value="standard">"Standard"</option>
            <option value="extended">"Extended"</option>
            <option value="heavy">"Heavy"</option>
        </select>
    }
}

fn agent_effort_value(effort: AgentEffort) -> &'static str {
    match effort {
        AgentEffort::Standard => "standard",
        AgentEffort::Extended => "extended",
        AgentEffort::Heavy => "heavy",
    }
}

fn agent_effort_from_value(value: &str) -> AgentEffort {
    match value {
        "extended" => AgentEffort::Extended,
        "heavy" => AgentEffort::Heavy,
        _ => AgentEffort::Standard,
    }
}

fn apply_loaded_default_effort(
    settings: UserSettingsResponse,
    effort_touched: ReadSignal<bool>,
    set_selected_effort: WriteSignal<AgentEffort>,
) {
    if !effort_touched.get() {
        set_selected_effort.set(settings.default_effort.unwrap_or(AgentEffort::Standard));
    }
}

fn persist_default_effort(
    auth: AuthContext,
    effort: AgentEffort,
    set_error: WriteSignal<Option<String>>,
) {
    spawn_ui(async move {
        let client = auth.client();
        let settings = match client.settings().await {
            Ok(settings) => settings,
            Err(error) => {
                set_error.set(Some(error.to_string()));
                return;
            }
        };
        let request = UpdateUserSettingsRequest {
            default_model_selection: settings.default_model_selection,
            default_agent_profile_id: settings.default_agent_profile_id,
            default_effort: Some(effort),
        };
        if let Err(error) = client.update_settings(&request).await {
            set_error.set(Some(error.to_string()));
        }
    });
}

fn selected_profile_missing_option(
    profiles: ReadSignal<Vec<AgentProfileView>>,
    selected_profile: ReadSignal<String>,
) -> Option<impl IntoView> {
    let selected = selected_profile.get();
    let label = missing_profile_option_label(&profiles.get(), &selected)?;
    Some(view! {
        <option value=selected.clone()>{label}</option>
    })
}

fn missing_profile_option_label(profiles: &[AgentProfileView], selected: &str) -> Option<String> {
    if selected.is_empty()
        || selected == PROFILE_VALUE_DEFAULT
        || selected == PROFILE_VALUE_NONE
        || profiles.iter().any(|profile| profile.agent_id == selected)
    {
        return None;
    }
    Some(format!("Current profile · {selected}"))
}

fn agent_profile_selection_from_value(value: &str) -> AgentProfileSelection {
    match value {
        PROFILE_VALUE_DEFAULT => AgentProfileSelection::Default,
        PROFILE_VALUE_NONE => AgentProfileSelection::None,
        value => AgentProfileSelection::Profile {
            agent_profile_id: value.to_string(),
        },
    }
}

fn profile_value_to_id(value: &str) -> Option<String> {
    (value != PROFILE_VALUE_NONE && value != PROFILE_VALUE_DEFAULT && !value.trim().is_empty())
        .then(|| value.to_string())
}

#[component]
fn PendingAttachmentList(
    attachments: ReadSignal<Vec<PendingAttachmentFile>>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) -> impl IntoView {
    view! {
        {move || {
            let items = attachments.get();
            if items.is_empty() {
                ().into_any()
            } else {
                view! {
                    <ul class="pending-attachments" aria-label="Pending attachments">
                        {items
                            .into_iter()
                            .map(|attachment| {
                                let attachment_id = attachment.id;
                                let file_name = attachment.file.name();
                                let meta = format_attachment_meta(
                                    attachment.file.size() as u64,
                                    attachment.file.type_(),
                                );
                                view! {
                                    <li class="pending-attachment-item">
                                        <div class="pending-attachment-copy">
                                            <span class="pending-attachment-name">{file_name}</span>
                                            <span class="pending-attachment-meta">{meta}</span>
                                        </div>
                                        <button
                                            class="message-action-button"
                                            type="button"
                                            title="Remove attachment"
                                            aria-label="Remove attachment"
                                            on:click=move |_| {
                                                set_attachments
                                                    .update(|items| items.retain(|item| item.id != attachment_id));
                                            }
                                        >
                                            "✕"
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ul>
                }
                .into_any()
            }
        }}
    }
}

#[component]
fn MessageAttachments(attachments: Vec<TaskAttachment>) -> impl IntoView {
    if attachments.is_empty() {
        return ().into_any();
    }

    view! {
        <ul class="message-attachments" aria-label="Message attachments">
            {attachments
                .into_iter()
                .map(|attachment| {
                    let meta = format_attachment_meta(
                        attachment.size_bytes,
                        attachment.mime_type.clone().unwrap_or_default(),
                    );
                    let sandbox_path = attachment.sandbox_path.clone();
                    let sandbox_title = sandbox_path.clone();
                    view! {
                        <li class="message-attachment-item" title=sandbox_title>
                            <div class="message-attachment-copy">
                                <span class="message-attachment-name">{attachment.file_name}</span>
                                <span class="message-attachment-meta">{meta}</span>
                            </div>
                            <code class="message-attachment-path">{sandbox_path}</code>
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
    .into_any()
}

// ── Activity item model ──────────────────────────────────────────────────

enum ActivityItem {
    Tool {
        call: Option<PersistedTaskEvent>,
        result: Option<PersistedTaskEvent>,
    },
    Event(PersistedTaskEvent),
}

fn group_activity_events(events: Vec<PersistedTaskEvent>) -> Vec<ActivityItem> {
    let mut items: Vec<ActivityItem> = Vec::new();

    for event in events {
        match event.kind {
            TaskEventKind::ToolCall => {
                items.push(ActivityItem::Tool {
                    call: Some(event),
                    result: None,
                });
            }
            TaskEventKind::ToolResult => {
                let id = payload_str_event(&event, "id").filter(|value| !value.is_empty());
                let name = payload_str_event(&event, "name");
                let mut attached = false;

                // Walk backwards to find the matching ToolCall without a result yet.
                // Prefer stable invocation id; fall back to legacy name matching for
                // older persisted events that do not carry an id.
                for item in items.iter_mut().rev() {
                    let ActivityItem::Tool { call, result } = item else {
                        continue;
                    };
                    if result.is_some() {
                        continue;
                    }
                    let call_id = call
                        .as_ref()
                        .and_then(|c| payload_str_event(c, "id"))
                        .filter(|value| !value.is_empty());
                    let call_name = call.as_ref().and_then(|c| payload_str_event(c, "name"));
                    let stable_id_matches = id.is_some() && id == call_id;
                    let legacy_name_matches =
                        id.is_none() && call_id.is_none() && call_name == name;
                    if stable_id_matches || legacy_name_matches {
                        *result = Some(event.clone());
                        attached = true;
                        break;
                    }
                }

                if !attached {
                    items.push(ActivityItem::Tool {
                        call: None,
                        result: Some(event),
                    });
                }
            }
            _ => items.push(ActivityItem::Event(event)),
        }
    }

    items
}

// ── Activity item dispatcher ─────────────────────────────────────────────

#[component]
fn ActivityItemCard(item: ActivityItem) -> impl IntoView {
    match item {
        ActivityItem::Tool { call, result } => {
            view! { <ToolCard call=call result=result /> }.into_any()
        }
        ActivityItem::Event(event) => view! { <AgentEventCard event=event /> }.into_any(),
    }
}

// ── Tool Card (groups call + result) ─────────────────────────────────────

#[component]
fn ToolCard(call: Option<PersistedTaskEvent>, result: Option<PersistedTaskEvent>) -> impl IntoView {
    let tool_name = call
        .as_ref()
        .or(result.as_ref())
        .and_then(|e| payload_str_event(e, "name"))
        .unwrap_or_default();

    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Parse the nested output JSON from output_preview.
    let output_json = result.as_ref().and_then(parse_output_json);

    let display = match tool_name.as_str() {
        "execute_command" => {
            view! { <ShellToolCard call=call result=result output=output_json /> }.into_any()
        }
        "web_search" | "tavily_search" | "duckduckgo_search" | "duckduckgo_news" => view! {
            <SearchToolCard label="Web search" preview_query_first=false call=call result=result output=output_json />
        }
        .into_any(),
        "brave_search" => view! {
            <SearchToolCard label="Brave Search" preview_query_first=true call=call result=result output=output_json />
        }
        .into_any(),
        "searxng_search" => view! {
            <SearchToolCard label="SearXNG" preview_query_first=false call=call result=result output=output_json />
        }
        .into_any(),
        "web_markdown" => {
            view! { <WebMarkdownToolCard call=call result=result output=output_json /> }.into_any()
        }
        "crawl4ai_markdown" => {
            view! { <CrawlToolCard call=call result=result output=output_json /> }.into_any()
        }
        "spawn_sub_agents" => {
            view! { <SpawnSubAgentsToolCard call=call result=result output=output_json /> }.into_any()
        }
        "wait_sub_agents" => {
            view! { <WaitSubAgentsToolCard call=call result=result output=output_json /> }.into_any()
        }
        "write_todos" => {
            view! { <WriteTodosToolCard call=call result=result output=output_json /> }.into_any()
        }
        _ => {
            view! { <GenericToolCard name=tool_name call=call result=result output=output_json /> }
                .into_any()
        }
    };

    let status_class = if is_running {
        "tool-card running"
    } else if success {
        "tool-card success"
    } else {
        "tool-card failure"
    };

    view! {
        <section class=status_class>
            {display}
        </section>
    }
}

// ── Shell Tool Card (execute_command) ────────────────────────────────────

#[component]
fn ShellToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let status_text = if is_running {
        "running".to_string()
    } else if success {
        "ok".to_string()
    } else {
        output
            .as_ref()
            .and_then(|v| field_str(v, "status"))
            .unwrap_or_else(|| "failed".to_string())
    };

    let duration_ms = output
        .as_ref()
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        });
    let exit_code = output.as_ref().and_then(|v| field_i64(v, "exit_code"));
    let command = command_from_events(call.as_ref(), output.as_ref());
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let stderr = output.as_ref().and_then(|v| stream_text(v, "stderr"));
    let error_msg = output.as_ref().and_then(|v| field_str(v, "error_message"));

    let icon = if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    };

    let duration_label = duration_ms.map(|ms| {
        if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        }
    });

    // Default open: running, failed, or has no stream content (nothing to collapse)
    let has_streams = stdout.is_some() || stderr.is_some();
    let default_open = is_running || !success || !has_streams;

    // Compact preview: command or first line of stdout
    let preview_text = command
        .clone()
        .or_else(|| stdout.as_ref().map(|t| first_line(t)));

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Shell"</span>
            <span class="tool-meta">{status_text}</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {exit_code.map(|code| view! {
                <span class=if code != 0 { "tool-meta danger" } else { "tool-meta" }>
                    {format!("exit {code}")}
                </span>
            })}
            {error_msg.map(|msg| view! { <span class="tool-meta danger">{msg}</span> })}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{format!("$ {text}")}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {command.map(|cmd| view! {
                <pre class="tool-command">{format!("$ {cmd}")}</pre>
            })}
            {stdout.clone().map(|text| view! {
                <div class="tool-stream">
                    <div class="tool-stream-label">"stdout"</div>
                    <pre class="tool-stream-pre">{text}</pre>
                </div>
            })}
            {stderr.clone().map(|text| view! {
                <div class="tool-stream">
                    <div class="tool-stream-label">"stderr"</div>
                    <pre class="tool-stream-pre">{text}</pre>
                </div>
            })}
            {(stdout.is_none() && stderr.is_none() && !is_running).then(|| view! {
                <div class="tool-stream">
                    <pre class="tool-stream-pre">"No output"</pre>
                </div>
            })}
            {result.and_then(|e| e.payload.get("output_preview").cloned()).map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

// ── Search Tool Card (web_search / tavily_search) ────────────────────────

#[component]
fn SearchToolCard(
    label: &'static str,
    preview_query_first: bool,
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let duration_ms = output
        .as_ref()
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        });
    let query = call
        .as_ref()
        .and_then(|e| payload_str_event(e, "input_preview"));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    // Try to parse structured results from structured_payload.
    let search_results: Vec<SearchResult> = output
        .as_ref()
        .and_then(|sp| {
            sp.get("structured_payload")
                .and_then(|v| v.get("results"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            Some(SearchResult {
                                title: item.get("title")?.as_str()?.to_string(),
                                url: item.get("url")?.as_str()?.to_string(),
                                snippet: item
                                    .get("snippet")
                                    .or_else(|| item.get("description"))
                                    .or_else(|| item.get("content"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                })
        })
        .unwrap_or_default();

    let result_count = if search_results.is_empty() {
        None
    } else {
        Some(search_results.len())
    };

    let icon = if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    };

    let duration_label = duration_ms.map(|ms| {
        if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        }
    });

    let default_open = is_running || !success;

    let preview_snippet = search_results
        .first()
        .filter(|sr| !sr.snippet.is_empty())
        .map(|sr| sr.snippet.clone());
    let stdout_headline = stdout.as_ref().map(|text| first_line(text));
    // For Brave Search, query is the compact preview (stable, like Crawl's URL host).
    let preview_text = if success && preview_query_first {
        query
            .clone()
            .or_else(|| preview_snippet.clone())
            .or_else(|| stdout_headline.clone())
    } else if success {
        preview_snippet.or_else(|| {
            search_results
                .first()
                .filter(|sr| !sr.title.is_empty())
                .map(|sr| sr.title.clone())
                .or_else(|| stdout_headline.clone())
                .or_else(|| query.clone())
        })
    } else {
        result_summary.clone().or(query.clone())
    };

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">{label}</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {result_count.map(|n| view! {
                <span class="tool-meta">{format!("{n} results")}</span>
            })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {query.map(|q| view! {
                <div class="tool-query">
                    <span class="tool-label">"Query"</span>
                    <code>{q}</code>
                </div>
            })}
            {if !search_results.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {search_results.into_iter().take(8).map(|sr| view! {
                            <li class="search-result-item">
                                <div class="search-result-title">{sr.title}</div>
                                <div class="search-result-url">{sr.url}</div>
                                {(!sr.snippet.is_empty()).then(|| view! {
                                    <p class="search-result-snippet">{sr.snippet}</p>
                                })}
                            </li>
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout {
                view! {
                    <div class="tool-stream">
                        <div class="tool-stream-label">"output"</div>
                        <div class="tool-stream-content">
                            <MarkdownContent markdown=text />
                        </div>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
            {result.and_then(|e| e.payload.get("output_preview").cloned()).map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

// ── Crawl Tool Card (crawl4ai_markdown) ───────────────────────────────────

/// Extract hostname (with port) from a URL string for compact preview.
fn host_from_url_str(raw: &str) -> Option<String> {
    let s = raw
        .strip_prefix("https://")
        .or(raw.strip_prefix("http://"))?;
    let host = s.split('?').next().unwrap_or(s);
    let host = host.split('/').next().unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Human-readable byte/char count (e.g. "5.0K chars").
fn chars_label(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M chars", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K chars", count as f64 / 1_000.0)
    } else {
        format!("{count} chars")
    }
}

#[derive(Clone, Default)]
struct WebMarkdownOutput {
    url: Option<String>,
    content_type: Option<String>,
    fetched_bytes: Option<u64>,
    truncated: bool,
    markdown: Option<String>,
}

fn parse_web_markdown_stdout(text: &str) -> Option<WebMarkdownOutput> {
    let normalized = text.replace("\r\n", "\n");
    let rest = normalized.strip_prefix("## Web Markdown\n\n")?;
    let (metadata, body) = rest.split_once("\n\n").unwrap_or((rest, ""));

    let mut parsed = WebMarkdownOutput::default();
    for line in metadata.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();

        match key.trim() {
            "URL" if !value.is_empty() => parsed.url = Some(value.to_string()),
            "Content-Type" if !value.is_empty() => parsed.content_type = Some(value.to_string()),
            "Fetched-Bytes" => parsed.fetched_bytes = value.parse::<u64>().ok(),
            "Truncated" => {
                parsed.truncated = value.eq_ignore_ascii_case("yes")
                    || value.eq_ignore_ascii_case("true")
                    || value == "1";
            }
            _ => {}
        }
    }

    if !body.is_empty() {
        parsed.markdown = Some(body.to_string());
    }

    Some(parsed)
}

#[component]
fn WebMarkdownToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let duration_ms = output
        .as_ref()
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        });

    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    let stdout_text = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let web_markdown = stdout_text
        .as_ref()
        .and_then(|text| parse_web_markdown_stdout(text));
    let parsed_header = web_markdown.is_some();

    let url = web_markdown
        .as_ref()
        .and_then(|doc| doc.url.clone())
        .or_else(|| {
            output
                .as_ref()
                .and_then(|v| v.get("structured_payload"))
                .and_then(|payload| payload.get("url"))
                .and_then(Value::as_str)
                .map(String::from)
        })
        .or_else(|| {
            call.as_ref()
                .and_then(|e| payload_str_event(e, "input_preview"))
                .and_then(|input| serde_json::from_str::<Value>(&input).ok())
                .and_then(|v| v.get("url").and_then(Value::as_str).map(String::from))
        });
    let content_type = web_markdown
        .as_ref()
        .and_then(|doc| doc.content_type.clone());
    let fetched_bytes = web_markdown.as_ref().and_then(|doc| doc.fetched_bytes);
    let truncated = web_markdown
        .as_ref()
        .map(|doc| doc.truncated)
        .unwrap_or(false);
    let markdown = if parsed_header {
        web_markdown.as_ref().and_then(|doc| doc.markdown.clone())
    } else {
        stdout_text.clone()
    };

    let preview_host = url.as_ref().and_then(|u| host_from_url_str(u));

    let icon = if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    };

    let duration_label = duration_ms.map(|ms| {
        if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        }
    });

    let chars_display = markdown
        .as_ref()
        .map(|text| chars_label(text.chars().count() as u64));
    let default_open = is_running || !success;

    let preview_text = if !success {
        result_summary.clone()
    } else {
        preview_host.clone()
    };

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Web Markdown"</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {chars_display.map(|c| view! { <span class="tool-meta">{c}</span> })}
            {truncated.then(|| view! { <span class="tool-meta">"truncated"</span> })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {url.clone().map(|u| view! {
                <div class="tool-query">
                    <span class="tool-label">"URL"</span>
                    <code>{u}</code>
                </div>
            })}
            {content_type.map(|content_type| view! {
                <div class="tool-query">
                    <span class="tool-label">"Content-Type"</span>
                    <code>{content_type}</code>
                </div>
            })}
            {fetched_bytes.map(|bytes| view! {
                <div class="tool-query">
                    <span class="tool-label">"Fetched"</span>
                    <code>{format!("{bytes} bytes")}</code>
                </div>
            })}
            {parsed_header.then(|| view! {
                <div class="tool-query">
                    <span class="tool-label">"Truncated"</span>
                    <code>{if truncated { "yes" } else { "no" }}</code>
                </div>
            })}
            {if let Some(md) = markdown.filter(|m| !m.is_empty()) {
                view! {
                    <div class="tool-stream">
                        <div class="tool-stream-content">
                            <MarkdownContent markdown=md />
                        </div>
                    </div>
                }.into_any()
            } else if !is_running {
                view! {
                    <div class="tool-stream">
                        <pre class="tool-stream-pre">"No content"</pre>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
            {result.and_then(|e| e.payload.get("output_preview").cloned()).map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

#[component]
fn CrawlToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let duration_ms = output
        .as_ref()
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        });

    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    // Parse the inner JSON from stdout.text (crawl4ai success payload).
    let stdout_text = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let crawl: Option<Value> = stdout_text
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let url: Option<String> = crawl.as_ref().and_then(|v| {
        v.get("final_url")
            .or_else(|| v.get("url"))
            .and_then(Value::as_str)
            .map(String::from)
    });
    let markdown: Option<String> = crawl
        .as_ref()
        .and_then(|v| v.get("markdown").and_then(Value::as_str).map(String::from));
    let chars = crawl
        .as_ref()
        .and_then(|v| v.get("chars").and_then(Value::as_u64));
    let truncated = crawl
        .as_ref()
        .and_then(|v| v.get("truncated").and_then(Value::as_bool))
        .unwrap_or(false);

    // Fallback: try to extract URL from the ToolCall input_preview.
    let url = url.or_else(|| {
        call.as_ref()
            .and_then(|e| payload_str_event(e, "input_preview"))
            .and_then(|input| serde_json::from_str::<Value>(&input).ok())
            .and_then(|v| v.get("url").and_then(Value::as_str).map(String::from))
    });

    let preview_host = url.as_ref().and_then(|u| host_from_url_str(u));

    let icon = if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    };

    let duration_label = duration_ms.map(|ms| {
        if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        }
    });

    let chars_display = chars.map(chars_label);
    let default_open = is_running || !success;

    // Preview line: hostname or error summary
    let preview_text = if !success {
        result_summary.clone()
    } else {
        preview_host.clone()
    };

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Crawl"</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {chars_display.map(|c| view! { <span class="tool-meta">{c}</span> })}
            {truncated.then(|| view! { <span class="tool-meta">"truncated"</span> })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {url.clone().map(|u| view! {
                <div class="search-result-url" style="margin: 6px 0">{u}</div>
            })}
            {if let Some(md) = markdown.filter(|m| !m.is_empty()) {
                view! {
                    <div class="tool-stream">
                        <div class="tool-stream-content">
                            <MarkdownContent markdown=md.to_string() />
                        </div>
                    </div>
                }.into_any()
            } else if !is_running {
                view! {
                    <div class="tool-stream">
                        <pre class="tool-stream-pre">"No content"</pre>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
            // If stdout was not parseable as crawl JSON, show raw stdout as fallback.
            {crawl.is_none()
                .then(|| stdout_text.clone())
                .flatten()
                .map(|text| view! {
                    <div class="tool-stream">
                        <div class="tool-stream-label">"output"</div>
                        <pre class="tool-stream-pre">{text}</pre>
                    </div>
                })}
            {result.and_then(|e| e.payload.get("output_preview").cloned()).map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

// ── Delegation/Todos Tool Cards ───────────────────────────────────────────

#[derive(Clone, Default)]
struct SubAgentTaskView {
    id: Option<String>,
    task: String,
    status: String,
    tools: Vec<String>,
    context: Option<String>,
}

#[derive(Clone, Default)]
struct SubAgentStatusView {
    id: String,
    task: Option<String>,
    status: String,
    output: Option<String>,
    elapsed_ms: Option<u64>,
    completed: Option<bool>,
}

#[derive(Clone)]
struct TodoToolItemView {
    description: String,
    status: String,
}

#[component]
fn SpawnSubAgentsToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let parsed = stdout
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());

    let requested = parse_sub_agent_tasks_from_call(call.as_ref());
    let mut tasks = parsed
        .as_ref()
        .and_then(parse_spawned_sub_agent_tasks)
        .unwrap_or_default();
    if tasks.is_empty() {
        tasks = requested.clone();
        for task in &mut tasks {
            if task.status.is_empty() {
                task.status = if is_running { "starting" } else { "requested" }.to_string();
            }
        }
    } else {
        for (idx, task) in tasks.iter_mut().enumerate() {
            if let Some(requested_task) = requested.get(idx) {
                task.tools = requested_task.tools.clone();
                task.context = requested_task.context.clone();
                if task.task.is_empty() {
                    task.task = requested_task.task.clone();
                }
            }
        }
    }

    let active_count = parsed
        .as_ref()
        .and_then(|v| v.get("active_count"))
        .and_then(Value::as_u64);
    let max_active = parsed
        .as_ref()
        .and_then(|v| v.get("max_active"))
        .and_then(Value::as_u64);
    let active_label = active_count.map(|active| match max_active {
        Some(max) => format!("active {active}/{max}"),
        None => format!("active {active}"),
    });

    let icon = tool_status_icon(is_running, success);
    let started_count = tasks.len();
    let count_label = (started_count > 0).then(|| format!("{started_count} started"));
    let preview_text = if !success {
        result_summary.clone()
    } else {
        tasks
            .first()
            .map(|task| first_line(&task.task))
            .or_else(|| stdout.as_ref().map(|text| first_line(text)))
    };
    let default_open = is_running || !success || tasks.is_empty();
    let raw_output = result
        .as_ref()
        .and_then(|e| e.payload.get("output_preview").cloned());

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Sub-agents"</span>
            {count_label.map(|label| view! { <span class="tool-meta">{label}</span> })}
            {active_label.map(|label| view! { <span class="tool-meta">{label}</span> })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {if !tasks.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {tasks.into_iter().map(|task| {
                            let meta = sub_agent_task_meta(&task);
                            view! {
                                <li class="search-result-item">
                                    <div class="search-result-title">{task.task}</div>
                                    <div class="search-result-url">{meta}</div>
                                    {task.context.filter(|context| !context.is_empty()).map(|context| view! {
                                        <p class="search-result-snippet">{context}</p>
                                    })}
                                </li>
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout.clone() {
                view! {
                    <div class="tool-stream">
                        <div class="tool-stream-label">"output"</div>
                        <pre class="tool-stream-pre">{text}</pre>
                    </div>
                }.into_any()
            } else if !is_running {
                view! {
                    <div class="tool-stream">
                        <pre class="tool-stream-pre">"No sub-agents"</pre>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
            {raw_output.map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

#[component]
fn WaitSubAgentsToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let duration_label = tool_duration_ms(output.as_ref(), result.as_ref()).map(format_duration_ms);
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let parsed = stdout
        .as_ref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());

    let mut statuses = parsed
        .as_ref()
        .and_then(parse_sub_agent_statuses)
        .unwrap_or_default();
    if statuses.is_empty() && is_running {
        statuses = parse_sub_agent_wait_ids_from_call(call.as_ref());
    }

    let timed_out = parsed
        .as_ref()
        .and_then(|v| v.get("timed_out"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let active_count = parsed
        .as_ref()
        .and_then(|v| v.get("active_count"))
        .and_then(Value::as_u64);
    let max_active = parsed
        .as_ref()
        .and_then(|v| v.get("max_active"))
        .and_then(Value::as_u64);
    let active_label = active_count.map(|active| match max_active {
        Some(max) => format!("active {active}/{max}"),
        None => format!("active {active}"),
    });

    let total = statuses.len();
    let completed = statuses
        .iter()
        .filter(|status| status.completed.unwrap_or(false) || status.status == "completed")
        .count();
    let failed = statuses
        .iter()
        .filter(|status| matches!(status.status.as_str(), "failed" | "timed_out" | "cancelled"))
        .count();
    let count_label = (total > 0).then(|| format!("{completed}/{total} done"));
    let failed_label = (failed > 0).then(|| format!("{failed} failed"));

    let icon = tool_status_icon(is_running, success);
    let preview_text = if !success {
        result_summary.clone()
    } else {
        statuses
            .iter()
            .find_map(|status| status.output.as_ref().map(|output| first_line(output)))
            .or_else(|| statuses.first().map(sub_agent_status_preview))
            .or_else(|| stdout.as_ref().map(|text| first_line(text)))
    };
    let default_open = is_running || !success || timed_out || failed > 0;
    let raw_output = result
        .as_ref()
        .and_then(|e| e.payload.get("output_preview").cloned());

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Sub-agent results"</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {count_label.map(|label| view! { <span class="tool-meta">{label}</span> })}
            {active_label.map(|label| view! { <span class="tool-meta">{label}</span> })}
            {timed_out.then(|| view! { <span class="tool-meta danger">"timed out"</span> })}
            {failed_label.map(|label| view! { <span class="tool-meta danger">{label}</span> })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {if !statuses.is_empty() {
                view! {
                    <ol class="search-result-list">
                        {statuses.into_iter().map(|status| {
                            let title = status.task.clone().unwrap_or_else(|| status.id.clone());
                            let meta = sub_agent_status_meta(&status);
                            view! {
                                <li class="search-result-item">
                                    <div class="search-result-title">{title}</div>
                                    <div class="search-result-url">{meta}</div>
                                    {status.output.filter(|output| !output.is_empty()).map(|output| view! {
                                        <div class="tool-stream-content">
                                            <MarkdownContent markdown=output />
                                        </div>
                                    })}
                                </li>
                            }
                        }).collect::<Vec<_>>()}
                    </ol>
                }.into_any()
            } else if let Some(text) = stdout.clone() {
                view! {
                    <div class="tool-stream">
                        <div class="tool-stream-label">"output"</div>
                        <pre class="tool-stream-pre">{text}</pre>
                    </div>
                }.into_any()
            } else if !is_running {
                view! {
                    <div class="tool-stream">
                        <pre class="tool-stream-pre">"No sub-agent results"</pre>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
            {raw_output.map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

#[component]
fn WriteTodosToolCard(
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let todos = parse_todo_items_from_call(call.as_ref());

    let total = todos.len();
    let completed = todos
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let active = todos.iter().find(|item| item.status == "in_progress");
    let blocked = todos.iter().find(|item| item.status == "blocked_on_user");
    let count_label = (total > 0).then(|| format!("{completed}/{total} done"));
    let state_label = blocked
        .map(|_| "blocked")
        .or_else(|| active.map(|_| "doing"));
    let has_todos = !todos.is_empty();

    let icon = tool_status_icon(is_running, success);
    let preview_text = (!success).then(|| result_summary.clone()).flatten();
    let raw_output = result
        .as_ref()
        .and_then(|e| e.payload.get("output_preview").cloned());
    let show_fallback = !has_todos && (stdout.is_some() || !is_running);
    let show_details = raw_output.is_some() || show_fallback;
    let default_open = !success || !has_todos;

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">"Todos"</span>
            {count_label.map(|label| view! { <span class="tool-meta">{label}</span> })}
            {state_label.map(|label| view! {
                <span class=if label == "blocked" { "tool-meta danger" } else { "tool-meta" }>{label}</span>
            })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        {has_todos.then(|| render_todo_list(todos.clone(), false))}
        {show_details.then(|| view! {
            <details class="tool-card-body" open=default_open>
                <summary class="tool-card-expand">"details"</summary>
                {if show_fallback {
                    if let Some(text) = stdout.clone() {
                        view! {
                            <div class="tool-stream">
                                <div class="tool-stream-label">"output"</div>
                                <pre class="tool-stream-pre">{text}</pre>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="tool-stream">
                                <pre class="tool-stream-pre">"No todos"</pre>
                            </div>
                        }.into_any()
                    }
                } else {
                    ().into_any()
                }}
                {raw_output.map(|raw| view! {
                    <details class="tool-raw-details">
                        <summary>"Raw"</summary>
                        <pre class="tool-raw-json">{raw.to_string()}</pre>
                    </details>
                })}
            </details>
        })}
    }
}

// ── Generic Tool Card (fallback) ─────────────────────────────────────────

#[component]
fn GenericToolCard(
    name: String,
    call: Option<PersistedTaskEvent>,
    result: Option<PersistedTaskEvent>,
    output: Option<Value>,
) -> impl IntoView {
    let is_running = result.is_none();
    let success = result
        .as_ref()
        .and_then(|e| e.payload.get("success"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let duration_ms = output
        .as_ref()
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .as_ref()
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        });
    let exit_code = output.as_ref().and_then(|v| field_i64(v, "exit_code"));
    let stdout = output.as_ref().and_then(|v| stream_text(v, "stdout"));
    let stderr = output.as_ref().and_then(|v| stream_text(v, "stderr"));
    let result_summary = result
        .as_ref()
        .and_then(|event| tool_result_summary(event, output.as_ref()));

    let icon = if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    };

    let duration_label = duration_ms.map(|ms| {
        if ms >= 1000 {
            format!("{:.1}s", ms as f64 / 1000.0)
        } else {
            format!("{ms}ms")
        }
    });

    let has_streams = stdout.is_some() || stderr.is_some();
    let default_open = is_running || !success || !has_streams;

    // Compact preview: command_preview or first line of stdout
    let command_preview = call
        .as_ref()
        .and_then(|e| payload_str_event(e, "command_preview"));
    let preview_text = command_preview
        .or_else(|| stdout.as_ref().map(|t| first_line(t)))
        .or_else(|| (!success).then(|| result_summary.clone()).flatten());

    view! {
        <div class="tool-card-header">
            <span class="tool-status-icon">{icon}</span>
            <span class="tool-name">{name}</span>
            {duration_label.map(|d| view! { <span class="tool-meta">{d}</span> })}
            {exit_code.map(|code| view! {
                <span class=if code != 0 { "tool-meta danger" } else { "tool-meta" }>
                    {format!("exit {code}")}
                </span>
            })}
            {(!success).then(|| result_summary.clone().map(|summary| view! {
                <span class="tool-meta danger">{summary}</span>
            })).flatten()}
        </div>
        {preview_text.map(|text| view! {
            <div class="tool-preview">{text}</div>
        })}
        <details class="tool-card-body" open=default_open>
            <summary class="tool-card-expand">"details"</summary>
            {call.as_ref().and_then(|e| payload_str_event(e, "command_preview")).map(|cmd| view! {
                <pre class="tool-command">{format!("$ {cmd}")}</pre>
            })}
            {stdout.map(|text| view! {
                <div class="tool-stream">
                    <div class="tool-stream-label">"output"</div>
                    <pre class="tool-stream-pre">{text}</pre>
                </div>
            })}
            {stderr.map(|text| view! {
                <div class="tool-stream">
                    <div class="tool-stream-label">"stderr"</div>
                    <pre class="tool-stream-pre">{text}</pre>
                </div>
            })}
            {result.and_then(|e| e.payload.get("output_preview").cloned()).map(|raw| view! {
                <details class="tool-raw-details">
                    <summary>"Raw"</summary>
                    <pre class="tool-raw-json">{raw.to_string()}</pre>
                </details>
            })}
        </details>
    }
}

// ── Agent Event Card (non-tool events: reasoning, errors, retries, etc.) ─

#[component]
fn AgentEventCard(event: PersistedTaskEvent) -> impl IntoView {
    if event.kind == TaskEventKind::Reasoning {
        return view! { <ReasoningEventCard event=event /> }.into_any();
    }

    let kind = event.kind.clone();
    let title = event_title(&event);
    let body = event_body(&event);
    let delivered_file = delivered_file_link(&event);

    view! {
        <details class="agent-event-card">
            <summary class="agent-event-summary">
                <span class="agent-event-kind">{event_kind_label(&kind)}</span>
                <span class="agent-event-title">{title}</span>
                {event.truncated.then(|| view! { <span class="agent-event-flag">"truncated"</span> })}
                {event.redacted.then(|| view! { <span class="agent-event-flag danger">"redacted"</span> })}
            </summary>
            {delivered_file.map(|file| {
                let meta = format_attachment_meta(file.size_bytes, file.content_type.clone());
                let preview = delivered_file_preview(&file);
                view! {
                    <div class="agent-event-body">
                        <div class="message-attachment-copy">
                            <a class="message-attachment-name" href=file.download_url download>
                                {file.file_name}
                            </a>
                            <span class="message-attachment-meta">{meta}</span>
                        </div>
                        {preview}
                    </div>
                }
            })}
            {body.map(|text| view! {
                <div class="agent-event-body">
                    <pre class="agent-event-pre">{text}</pre>
                </div>
            })}
        </details>
    }
    .into_any()
}

#[component]
fn ReasoningEventCard(event: PersistedTaskEvent) -> impl IntoView {
    let summary = reasoning_event_summary(&event).unwrap_or_else(|| "Thinking".to_string());
    let preview = compact_reasoning_preview(&summary, 140);
    let show_details = event.truncated || event.redacted || preview != summary;
    let details_summary = summary.clone();

    view! {
        <section class="tool-card agent-event-card reasoning-event-card">
            <div class="tool-card-header">
                <span class="tool-status-icon reasoning-status-icon">"∴"</span>
                <span class="tool-name">"Thinking"</span>
                <span class="tool-meta">"CoT"</span>
                {event.truncated.then(|| view! { <span class="tool-meta">"truncated"</span> })}
                {event.redacted.then(|| view! { <span class="tool-meta danger">"redacted"</span> })}
            </div>
            <div class="tool-preview reasoning-preview">{preview}</div>
            {show_details.then(|| view! {
                <details class="tool-card-body reasoning-details">
                    <summary class="tool-card-expand">"details"</summary>
                    <div class="tool-stream">
                        <div class="tool-stream-label">"reasoning"</div>
                        <pre class="tool-stream-pre">{details_summary}</pre>
                    </div>
                </details>
            })}
        </section>
    }
}

#[component]
fn DeliveredFilesMessage(files: Vec<DeliveredFileLink>) -> impl IntoView {
    view! {
        <div class="message assistant-message-wrap">
            <div class="assistant-message">
                <div class="user-message-body">
                    <strong>"Delivered files"</strong>
                    <DeliveredFilesList files=files />
                </div>
            </div>
        </div>
    }
}

#[component]
fn DeliveredFilesList(files: Vec<DeliveredFileLink>) -> impl IntoView {
    view! {
        <ul class="message-attachments" aria-label="Delivered files">
            {files
                .into_iter()
                .map(|file| {
                    let meta = format_attachment_meta(file.size_bytes, file.content_type.clone());
                    let preview = delivered_file_preview(&file);
                    view! {
                        <li class="message-attachment-item">
                            <div class="message-attachment-copy">
                                <a class="message-attachment-name" href=file.download_url.clone() download>
                                    {file.file_name.clone()}
                                </a>
                                <span class="message-attachment-meta">{meta}</span>
                            </div>
                            {preview}
                        </li>
                    }
                })
                .collect_view()}
        </ul>
    }
}

// ── Context Card ─────────────────────────────────────────────────────────

#[component]
fn ContextCard(progress: ReadSignal<Option<ProgressSnapshot>>) -> impl IntoView {
    let snapshot_memo = Memo::new(move |_| progress.get().and_then(|p| p.latest_token_snapshot));

    view! {
        {move || {
            let Some(snapshot) = snapshot_memo.get() else {
                return ().into_any();
            };
            let free = snapshot.get("headroom_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let flow = snapshot.get("hot_memory_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let prompt = snapshot.get("system_prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let tools = snapshot.get("tool_schema_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let budget = snapshot.get("budget_state").and_then(|v| v.as_str()).unwrap_or("context");
            let health_class = match budget {
                "Healthy" => "context-health-ok",
                "Warning" | "Approaching" => "context-health-warn",
                "Critical" | "Over" => "context-health-over",
                _ => "context-health-ok",
            };
            view! {
                <section class="context-card">
                    <div class="context-card-grid">
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Free"</div>
                            <div class="context-card-cell-value">{compact_tokens(free)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Flow"</div>
                            <div class="context-card-cell-value">{compact_tokens(flow)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Prompt"</div>
                            <div class="context-card-cell-value">{compact_tokens(prompt)}</div>
                        </div>
                        <div class="context-card-cell">
                            <div class="context-card-cell-label">"Tools"</div>
                            <div class="context-card-cell-value">{compact_tokens(tools)}</div>
                        </div>
                    </div>
                    <div class="context-card-health">
                        <span class={format!("context-health-dot {health_class}")}></span>
                        <span class="context-health-label">{budget.to_lowercase()}</span>
                    </div>
                </section>
            }.into_any()
        }}
    }
}

// ── Todos Card ───────────────────────────────────────────────────────────

#[component]
fn TodosCard(todos: Value) -> impl IntoView {
    let items = parse_todo_items_from_value(&todos);

    if items.is_empty() {
        return ().into_any();
    }

    render_todo_list(items, true)
}

// ── Event helpers ────────────────────────────────────────────────────────

fn is_chat_visible_event(kind: &TaskEventKind) -> bool {
    matches!(
        kind,
        TaskEventKind::Reasoning
            | TaskEventKind::ToolCall
            | TaskEventKind::ToolResult
            | TaskEventKind::TodosUpdated
            | TaskEventKind::FileToSend
            | TaskEventKind::Continuation
            | TaskEventKind::Cancelling
            | TaskEventKind::Cancelled
            | TaskEventKind::Error
            | TaskEventKind::LoopDetected
            | TaskEventKind::RuntimeCompactionStarted
            | TaskEventKind::RuntimeCompactionCompleted
            | TaskEventKind::RuntimeCompactionFailed
            | TaskEventKind::RuntimeCompactionSkipped
            | TaskEventKind::RepeatedCompactionWarning
            | TaskEventKind::HistoryRepairApplied
            | TaskEventKind::Finished
    )
}

/// Filter out empty reasoning events and other noise.
fn is_useful_event(event: &PersistedTaskEvent) -> bool {
    if event.kind == TaskEventKind::Reasoning {
        return reasoning_event_summary(event).is_some();
    }
    true
}

fn reasoning_event_summary(event: &PersistedTaskEvent) -> Option<String> {
    payload_str_event(event, "summary")
        .map(|summary| summary.trim().to_string())
        .filter(|summary| !summary.is_empty() && summary != "Reasoning")
        .or_else(|| {
            let summary = event.summary.trim();
            (!summary.is_empty() && summary != "Reasoning").then(|| summary.to_string())
        })
}

fn compact_reasoning_preview(summary: &str, max_chars: usize) -> String {
    let compact = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let preview = chars.by_ref().take(max_chars).collect::<String>();

    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

fn event_kind_label(kind: &TaskEventKind) -> &'static str {
    match kind {
        TaskEventKind::ToolCall => "tool call",
        TaskEventKind::ToolResult => "tool result",
        TaskEventKind::TodosUpdated => "todos",
        TaskEventKind::Reasoning => "reasoning",
        TaskEventKind::Error => "error",
        TaskEventKind::Finished => "done",
        TaskEventKind::Cancelling => "cancelling",
        TaskEventKind::Cancelled => "cancelled",
        TaskEventKind::LoopDetected => "loop detected",
        TaskEventKind::RuntimeCompactionStarted => "compacting",
        TaskEventKind::RuntimeCompactionCompleted => "compacted",
        TaskEventKind::RuntimeCompactionFailed => "compaction failed",
        TaskEventKind::RuntimeCompactionSkipped => "compaction skipped",
        TaskEventKind::RepeatedCompactionWarning => "repeated compaction",
        TaskEventKind::HistoryRepairApplied => "history repair",
        TaskEventKind::RateLimitRetrying => "rate limit retry",
        TaskEventKind::LlmRetrying => "llm retry",
        TaskEventKind::ProviderFailoverActivated => "provider failover",
        TaskEventKind::FileToSend => "file",
        TaskEventKind::Continuation => "continuation",
        _ => "event",
    }
}

fn event_title(event: &PersistedTaskEvent) -> String {
    match event.kind {
        TaskEventKind::ToolCall => {
            payload_str_event(event, "name").unwrap_or_else(|| event.summary.clone())
        }
        TaskEventKind::ToolResult => {
            let name = payload_str_event(event, "name").unwrap_or_else(|| event.summary.clone());
            payload_str_event(event, "result_summary")
                .filter(|summary| !summary.is_empty() && summary != &name)
                .map(|summary| format!("{name} — {summary}"))
                .unwrap_or(name)
        }
        _ => event.summary.clone(),
    }
}

fn event_body(event: &PersistedTaskEvent) -> Option<String> {
    if event.kind == TaskEventKind::FileToSend
        && payload_str_event(event, "download_url").is_some()
        && payload_str_event(event, "delivery_error").is_none()
    {
        return None;
    }
    if event.redacted {
        return Some("Payload redacted".to_string());
    }

    match event.kind {
        TaskEventKind::ToolCall => payload_str_event(event, "command_preview")
            .or_else(|| payload_str_event(event, "input_preview")),
        TaskEventKind::ToolResult => payload_str_event(event, "output_preview"),
        TaskEventKind::TodosUpdated => None,
        TaskEventKind::Reasoning => payload_str_event(event, "summary"),
        _ => serde_json::to_string_pretty(&event.payload).ok(),
    }
}

// ── JSON helpers ─────────────────────────────────────────────────────────

/// Extract a string field from an event's payload.
fn payload_str_event(event: &PersistedTaskEvent, key: &str) -> Option<String> {
    event
        .payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

#[derive(Clone)]
struct DeliveredFileLink {
    file_name: String,
    download_url: String,
    content_type: String,
    size_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveredFilePreviewKind {
    Image,
    Audio,
    Pdf,
}

fn delivered_file_link(event: &PersistedTaskEvent) -> Option<DeliveredFileLink> {
    if event.kind != TaskEventKind::FileToSend {
        return None;
    }
    Some(DeliveredFileLink {
        file_name: payload_str_event(event, "file_name")?,
        download_url: payload_str_event(event, "download_url")?,
        content_type: payload_str_event(event, "content_type").unwrap_or_default(),
        size_bytes: event
            .payload
            .get("size_bytes")
            .and_then(|value| value.as_u64())
            .or_else(|| {
                event
                    .payload
                    .get("byte_len")
                    .and_then(|value| value.as_u64())
            })
            .unwrap_or(0),
    })
}

fn delivered_file_preview(file: &DeliveredFileLink) -> AnyView {
    let Some(kind) = delivered_file_preview_kind(file) else {
        return ().into_any();
    };
    let inline_url = inline_file_url(&file.download_url);
    match kind {
        DeliveredFilePreviewKind::Image => view! {
            <a href=file.download_url.clone() download>
                <img
                    class="agent-event-inline-preview"
                    src=inline_url
                    alt=file.file_name.clone()
                    loading="lazy"
                />
            </a>
        }
        .into_any(),
        DeliveredFilePreviewKind::Audio => view! {
            <audio class="agent-event-inline-preview" controls preload="none" src=inline_url>
                "Your browser does not support audio playback."
            </audio>
        }
        .into_any(),
        DeliveredFilePreviewKind::Pdf => view! {
            <object
                class="agent-event-inline-preview"
                data=inline_url
                type="application/pdf"
                aria-label=format!("PDF preview for {}", file.file_name)
            >
                <a href=file.download_url.clone() download>
                    "Open PDF"
                </a>
            </object>
        }
        .into_any(),
    }
}

fn delivered_file_preview_kind(file: &DeliveredFileLink) -> Option<DeliveredFilePreviewKind> {
    let mime = file
        .content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if mime.starts_with("image/") {
        Some(DeliveredFilePreviewKind::Image)
    } else if mime.starts_with("audio/") {
        Some(DeliveredFilePreviewKind::Audio)
    } else if mime == "application/pdf" {
        Some(DeliveredFilePreviewKind::Pdf)
    } else {
        None
    }
}

fn inline_file_url(download_url: &str) -> String {
    let separator = if download_url.contains('?') { '&' } else { '?' };
    format!("{download_url}{separator}disposition=inline")
}

/// Extract first line from text, truncated to max chars.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("");
    if line.len() > 120 {
        format!("{}...", &line[..120])
    } else {
        line.to_string()
    }
}

/// Parse the nested JSON inside `output_preview` for ToolResult events.
/// The output_preview field contains a JSON string (the ToolOutput encode_model_content).
fn parse_output_json(event: &PersistedTaskEvent) -> Option<Value> {
    let raw = event
        .payload
        .get("output_preview")
        .and_then(|v| v.as_str())?;
    serde_json::from_str::<Value>(raw).ok()
}

/// Extract a string field from a JSON value.
fn field_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

/// Extract an i64 field from a JSON value.
fn field_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| v.as_i64())
}

fn tool_duration_ms(output: Option<&Value>, result: Option<&PersistedTaskEvent>) -> Option<i64> {
    output
        .and_then(|v| field_i64(v, "duration_ms"))
        .or_else(|| {
            result
                .and_then(|e| e.payload.get("duration_ms"))
                .and_then(|v| v.as_i64())
        })
}

fn format_duration_ms(ms: i64) -> String {
    if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn tool_status_icon(is_running: bool, success: bool) -> &'static str {
    if is_running {
        "\u{23f3}"
    } else if success {
        "\u{2713}"
    } else {
        "\u{2717}"
    }
}

fn input_preview_json(call: Option<&PersistedTaskEvent>) -> Option<Value> {
    call.and_then(|event| payload_str_event(event, "input_preview"))
        .and_then(|input| serde_json::from_str::<Value>(&input).ok())
}

fn parse_sub_agent_tasks_from_call(call: Option<&PersistedTaskEvent>) -> Vec<SubAgentTaskView> {
    input_preview_json(call)
        .and_then(|payload| {
            payload.get("tasks").and_then(Value::as_array).map(|tasks| {
                tasks
                    .iter()
                    .map(|task| {
                        let tools = task
                            .get("tools")
                            .and_then(Value::as_array)
                            .map(|tools| {
                                tools
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .map(ToString::to_string)
                                    .collect()
                            })
                            .unwrap_or_default();

                        SubAgentTaskView {
                            id: None,
                            task: task
                                .get("task")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            status: String::new(),
                            tools,
                            context: task
                                .get("context")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                        }
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn parse_spawned_sub_agent_tasks(payload: &Value) -> Option<Vec<SubAgentTaskView>> {
    payload
        .get("started")
        .and_then(Value::as_array)
        .map(|started| {
            started
                .iter()
                .map(|task| SubAgentTaskView {
                    id: task
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    task: task
                        .get("task")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    status: task
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("running")
                        .to_string(),
                    tools: Vec::new(),
                    context: None,
                })
                .collect()
        })
}

fn sub_agent_task_meta(task: &SubAgentTaskView) -> String {
    let mut parts = Vec::new();
    if let Some(id) = &task.id {
        if !id.is_empty() {
            parts.push(format!("id: {id}"));
        }
    }
    if !task.status.is_empty() {
        parts.push(format!("status: {}", task.status));
    }
    if !task.tools.is_empty() {
        parts.push(format!("tools: {}", task.tools.join(", ")));
    }
    if parts.is_empty() {
        "sub-agent".to_string()
    } else {
        parts.join(" · ")
    }
}

fn parse_sub_agent_statuses(payload: &Value) -> Option<Vec<SubAgentStatusView>> {
    payload
        .get("statuses")
        .and_then(Value::as_array)
        .map(|statuses| {
            statuses
                .iter()
                .map(|status| SubAgentStatusView {
                    id: status
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    task: status
                        .get("task")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    status: status
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    output: status
                        .get("output")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    elapsed_ms: status.get("elapsed_ms").and_then(Value::as_u64),
                    completed: status.get("completed").and_then(Value::as_bool),
                })
                .collect()
        })
}

fn parse_sub_agent_wait_ids_from_call(
    call: Option<&PersistedTaskEvent>,
) -> Vec<SubAgentStatusView> {
    input_preview_json(call)
        .and_then(|payload| {
            payload.get("ids").and_then(Value::as_array).map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(|id| SubAgentStatusView {
                        id: id.to_string(),
                        task: None,
                        status: "waiting".to_string(),
                        output: None,
                        elapsed_ms: None,
                        completed: None,
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

fn sub_agent_status_preview(status: &SubAgentStatusView) -> String {
    status
        .task
        .as_ref()
        .map(|task| format!("{}: {}", status.status, first_line(task)))
        .unwrap_or_else(|| format!("{}: {}", status.status, status.id))
}

fn sub_agent_status_meta(status: &SubAgentStatusView) -> String {
    let mut parts = Vec::new();
    if !status.id.is_empty() {
        parts.push(format!("id: {}", status.id));
    }
    if !status.status.is_empty() {
        parts.push(format!("status: {}", status.status));
    }
    if let Some(elapsed_ms) = status.elapsed_ms {
        let elapsed_ms = elapsed_ms.min(i64::MAX as u64) as i64;
        parts.push(format!("elapsed: {}", format_duration_ms(elapsed_ms)));
    }
    if parts.is_empty() {
        "sub-agent".to_string()
    } else {
        parts.join(" · ")
    }
}

fn parse_todo_items_from_call(call: Option<&PersistedTaskEvent>) -> Vec<TodoToolItemView> {
    input_preview_json(call)
        .map(|payload| parse_todo_items_from_value(&payload))
        .unwrap_or_default()
}

fn parse_todo_items_from_value(value: &Value) -> Vec<TodoToolItemView> {
    value
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| {
            value
                .get("todos")
                .and_then(|todos| todos.get("items"))
                .and_then(Value::as_array)
        })
        .or_else(|| value.get("todos").and_then(Value::as_array))
        .map(|items| {
            items
                .iter()
                .map(|item| TodoToolItemView {
                    description: item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("pending")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn render_todo_list(items: Vec<TodoToolItemView>, show_title: bool) -> AnyView {
    if items.is_empty() {
        return ().into_any();
    }

    view! {
        <section class="todos-card">
            {show_title.then(|| view! {
                <div class="todos-card-title">"Todos"</div>
            })}
            <ol class="todo-list">
                {items.into_iter().map(|item| {
                    let label = todo_status_label(&item.status);
                    let marker = todo_status_marker(label);

                    view! {
                        <li class=format!("todo-item {} {label}", item.status)>
                            <span class=format!("todo-check {label}")>{marker}</span>
                            <span class=format!("todo-status-badge {label}")>{label}</span>
                            <span class=format!("todo-description todo-text {label}")>{item.description}</span>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ol>
        </section>
    }
    .into_any()
}

fn tool_result_summary(event: &PersistedTaskEvent, output: Option<&Value>) -> Option<String> {
    payload_str_event(event, "result_summary").or_else(|| {
        let output = output?;
        let payload = output.get("structured_payload")?;
        let error_kind = payload.get("error_kind").and_then(Value::as_str)?;

        match payload.get("provider").and_then(Value::as_str) {
            Some("web_markdown") => {
                let host = payload.get("host").and_then(Value::as_str);
                let status_code = payload.get("status_code").and_then(Value::as_i64);

                Some(match error_kind {
                    "anti_bot" => host
                        .map(|host| format!("anti_bot at {host}"))
                        .unwrap_or_else(|| "anti_bot".to_string()),
                    "http_status" => status_code
                        .map(|status| format!("http_status {status}"))
                        .unwrap_or_else(|| "http_status".to_string()),
                    other => host
                        .map(|host| format!("{other} at {host}"))
                        .unwrap_or_else(|| other.to_string()),
                })
            }
            Some("crawl4ai_markdown") => {
                let host = payload.get("host").and_then(Value::as_str);
                let status_code = payload.get("status_code").and_then(Value::as_i64);

                Some(match error_kind {
                    "crawl4ai_http_status" => status_code
                        .map(|code| format!("http_status {code}"))
                        .unwrap_or_else(|| "http_status".to_string()),
                    "crawl4ai_unavailable" => "crawl4ai unavailable".to_string(),
                    "crawl4ai_auth_failed" => "auth_failed".to_string(),
                    "timeout" => host
                        .map(|host| format!("timeout at {host}"))
                        .unwrap_or_else(|| "timeout".to_string()),
                    "dns_failed" => host
                        .map(|host| format!("dns_failed at {host}"))
                        .unwrap_or_else(|| "dns_failed".to_string()),
                    "network" => host
                        .map(|host| format!("network at {host}"))
                        .unwrap_or_else(|| "network".to_string()),
                    other => other.to_string(),
                })
            }
            Some("duckduckgo") => Some(match error_kind {
                "rate_limited" => "rate_limited".to_string(),
                "blocked" => "blocked".to_string(),
                "parser_break" => "parser_break".to_string(),
                "timeout" => "timeout".to_string(),
                other => other.to_string(),
            }),
            _ => None,
        }
    })
}

/// Extract text from a stream object (stdout/stderr) in the ToolOutput JSON.
/// Handles both `text` field and `head`/`tail` for truncated output.
fn stream_text(output: &Value, stream_name: &str) -> Option<String> {
    let stream = output.get(stream_name)?;

    if stream
        .get("binary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Some("[binary output hidden]".to_string());
    }

    if let Some(text) = stream.get("text").and_then(Value::as_str) {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    let head = stream.get("head").and_then(Value::as_str);
    let tail = stream.get("tail").and_then(Value::as_str);

    match (head, tail) {
        (Some(h), Some(t)) => Some(format!("{h}\n...\n{t}")),
        (Some(h), None) => Some(h.to_string()),
        (None, Some(t)) => Some(t.to_string()),
        _ => None,
    }
}

/// Extract command string from ToolCall + ToolResult pair.
fn command_from_events(
    call: Option<&PersistedTaskEvent>,
    output: Option<&Value>,
) -> Option<String> {
    // 1. ToolCall.command_preview
    call.and_then(|e| payload_str_event(e, "command_preview"))
        .or_else(|| {
            // 2. structured_payload.command from result
            output
                .and_then(|o| o.get("structured_payload"))
                .and_then(|sp| sp.get("command"))
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        })
        .or_else(|| {
            // 3. ToolCall.input_preview
            call.and_then(|e| payload_str_event(e, "input_preview"))
        })
}

fn todo_status_label(status: &str) -> &'static str {
    match status {
        "completed" => "done",
        "in_progress" => "doing",
        "blocked_on_user" => "blocked",
        "cancelled" => "cancelled",
        _ => "todo",
    }
}

fn todo_status_marker(label: &str) -> &'static str {
    match label {
        "done" => "✓",
        "doing" => "•",
        "blocked" => "!",
        "cancelled" => "×",
        _ => "",
    }
}

// ── Search result model ──────────────────────────────────────────────────

#[derive(Clone)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

// ── Task input edit form ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskInputEditForm(
    session_id: String,
    task_id: String,
    version_group_id: String,
    original_input: String,
    attachments: Vec<TaskAttachment>,
    draft: ReadSignal<String>,
    set_draft: WriteSignal<String>,
    saving: ReadSignal<bool>,
    set_saving: WriteSignal<bool>,
    set_editing: WriteSignal<bool>,
    set_selected_versions: WriteSignal<HashMap<String, String>>,
    set_drawer_open: WriteSignal<bool>,
    stream_signals: StreamUiSignals,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let submit_edit = {
        let session_id = session_id.clone();
        let task_id = task_id.clone();
        let version_group_id = version_group_id.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            set_saving.set(true);
            set_error.set(None);
            let request = CreateTaskVersionRequest {
                input_markdown: draft.get(),
                attachments: attachments.clone(),
                effort: None,
            };
            let session_id = session_id.clone();
            let task_id = task_id.clone();
            let version_group_id = version_group_id.clone();
            spawn_ui(async move {
                let client = auth.client();
                match client
                    .create_task_version(&session_id, &task_id, &request)
                    .await
                {
                    Ok(response) => {
                        let task = response.task;
                        stream_signals.set_events.set(Vec::new());
                        stream_signals.set_progress.set(None);
                        stream_signals
                            .set_active_task
                            .set(Some(summary_to_detail(&session_id, &task)));
                        stream_signals.set_last_terminal_status.set(None);
                        stream_signals
                            .set_tasks
                            .update(|items| upsert_task_summary(items, task.clone()));
                        set_selected_versions.update(|items| {
                            items.insert(version_group_id.clone(), task.task_id.clone());
                        });
                        set_drawer_open.set(false);
                        start_task_stream(
                            client,
                            session_id.clone(),
                            task.task_id.clone(),
                            stream_signals,
                        );
                        set_editing.set(false);
                    }
                    Err(error) => set_error.set(Some(error.to_string())),
                }
                set_saving.set(false);
            });
        }
    };
    let cancel_edit = move |_| {
        set_draft.set(original_input.clone());
        set_editing.set(false);
    };

    view! {
        <form class="inline-edit" on:submit=submit_edit>
            <textarea
                rows="14"
                prop:value=draft
                on:input=move |ev| set_draft.set(event_target_value(&ev))
            />
            <div class="composer-actions">
                <button type="submit" disabled=saving>"Save"</button>
                <button class="secondary" type="button" on:click=cancel_edit>"Cancel"</button>
            </div>
        </form>
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn can_submit_input(input: &str, attachments: &[PendingAttachmentFile]) -> bool {
    !input.trim().is_empty() || !attachments.is_empty()
}

fn append_pending_browser_files(
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
    files: Vec<web_sys::File>,
) {
    if files.is_empty() {
        return;
    }
    let start_id = next_id.get_untracked();
    let new_files = into_pending_attachment_files(files, start_id);
    set_next_id.set(start_id + new_files.len());
    set_attachments.update(|items| items.extend(new_files));
}

fn append_pasted_image_files(
    ev: &leptos::ev::ClipboardEvent,
    next_id: ReadSignal<usize>,
    set_next_id: WriteSignal<usize>,
    set_attachments: WriteSignal<Vec<PendingAttachmentFile>>,
) {
    append_pending_browser_files(
        next_id,
        set_next_id,
        set_attachments,
        browser_image_files_from_clipboard_event(ev),
    );
}

fn into_pending_attachment_files(
    files: Vec<web_sys::File>,
    start_id: usize,
) -> Vec<PendingAttachmentFile> {
    files
        .into_iter()
        .enumerate()
        .map(|(offset, file)| PendingAttachmentFile {
            id: start_id + offset,
            file,
        })
        .collect()
}

fn browser_files(attachments: &[PendingAttachmentFile]) -> Vec<web_sys::File> {
    attachments
        .iter()
        .map(|attachment| attachment.file.clone())
        .collect()
}

fn browser_files_from_input_event(ev: &leptos::ev::Event) -> Vec<web_sys::File> {
    use wasm_bindgen::JsCast;

    let Some(target) = ev.target() else {
        return Vec::new();
    };
    let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() else {
        return Vec::new();
    };
    let files = input
        .files()
        .map(browser_files_from_file_list)
        .unwrap_or_default();
    input.set_value("");
    files
}

fn browser_files_from_drag_event(ev: &leptos::ev::DragEvent) -> Vec<web_sys::File> {
    ev.data_transfer()
        .and_then(|transfer| transfer.files())
        .map(browser_files_from_file_list)
        .unwrap_or_default()
}

fn browser_image_files_from_clipboard_event(ev: &leptos::ev::ClipboardEvent) -> Vec<web_sys::File> {
    ev.clipboard_data()
        .and_then(|transfer| transfer.files())
        .map(browser_image_files_from_file_list)
        .unwrap_or_default()
}

fn browser_files_from_file_list(file_list: web_sys::FileList) -> Vec<web_sys::File> {
    (0..file_list.length())
        .filter_map(|index| file_list.item(index))
        .collect()
}

fn browser_image_files_from_file_list(file_list: web_sys::FileList) -> Vec<web_sys::File> {
    browser_files_from_file_list(file_list)
        .into_iter()
        .filter(|file| is_image_file_metadata(&file.type_(), &file.name()))
        .collect()
}

fn is_image_file_metadata(mime_type: &str, file_name: &str) -> bool {
    let mime_type = mime_type.trim().to_ascii_lowercase();
    if mime_type.starts_with("image/") {
        return true;
    }

    let file_name = file_name.trim().to_ascii_lowercase();
    [
        ".avif", ".bmp", ".gif", ".heic", ".heif", ".jpeg", ".jpg", ".png", ".svg", ".tif",
        ".tiff", ".webp",
    ]
    .iter()
    .any(|extension| file_name.ends_with(extension))
}

fn format_attachment_meta(size_bytes: u64, mime_type: String) -> String {
    let size = format_file_size(size_bytes);
    let mime = mime_type.trim();
    if mime.is_empty() {
        size
    } else {
        format!("{size} • {mime}")
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn latest_task(tasks: Vec<TaskSummary>) -> Option<TaskSummary> {
    tasks.into_iter().max_by_key(|task| task.updated_at)
}

fn latest_editable_task_id(tasks: &[TaskSummary]) -> Option<String> {
    tasks
        .iter()
        .max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        })
        .and_then(|task| task.status.is_terminal().then(|| task.task_id.clone()))
}

fn upsert_task_summary(items: &mut Vec<TaskSummary>, task: TaskSummary) {
    if let Some(existing) = items.iter_mut().find(|item| item.task_id == task.task_id) {
        *existing = task;
    } else {
        items.push(task);
    }
    items.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
}

#[derive(Clone)]
struct TaskVersionGroup {
    version_group_id: String,
    versions: Vec<TaskSummary>,
}

fn group_task_versions(tasks: &[TaskSummary]) -> Vec<TaskVersionGroup> {
    let mut grouped = HashMap::<String, Vec<TaskSummary>>::new();
    for task in tasks {
        grouped
            .entry(task.effective_version_group_id().to_string())
            .or_default()
            .push(task.clone());
    }

    let mut groups = grouped
        .into_iter()
        .map(|(version_group_id, mut versions)| {
            versions.sort_by(|a, b| {
                a.effective_version_index()
                    .cmp(&b.effective_version_index())
                    .then_with(|| a.created_at.cmp(&b.created_at))
                    .then_with(|| a.task_id.cmp(&b.task_id))
            });
            TaskVersionGroup {
                version_group_id,
                versions,
            }
        })
        .collect::<Vec<_>>();

    groups.sort_by(|a, b| {
        first_group_task(&a.versions)
            .created_at
            .cmp(&first_group_task(&b.versions).created_at)
            .then_with(|| a.version_group_id.cmp(&b.version_group_id))
    });
    groups
}

fn first_group_task(versions: &[TaskSummary]) -> &TaskSummary {
    versions
        .first()
        .expect("task version groups must contain at least one task")
}

fn selected_version_index(versions: &[TaskSummary], selected_task_id: Option<&str>) -> usize {
    selected_task_id
        .and_then(|task_id| versions.iter().position(|task| task.task_id == task_id))
        .unwrap_or_else(|| versions.len().saturating_sub(1))
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

#[cfg(test)]
mod tests {
    use super::{
        linkify_delivered_files_in_markdown, missing_profile_option_label, DeliveredFileLink,
        PROFILE_VALUE_DEFAULT, PROFILE_VALUE_NONE,
    };

    fn delivered_file(file_name: &str, download_url: &str) -> DeliveredFileLink {
        DeliveredFileLink {
            file_name: file_name.to_string(),
            download_url: download_url.to_string(),
            content_type: "application/octet-stream".to_string(),
            size_bytes: 1,
        }
    }

    #[test]
    fn linkifies_delivered_file_code_spans_in_final_markdown() {
        let markdown = "Done: `duckduckgo.zip`\n\n- File: `duckduckgo.zip`";
        let rendered = linkify_delivered_files_in_markdown(
            markdown,
            &[delivered_file(
                "duckduckgo.zip",
                "/api/v1/files/duckduckgo.zip",
            )],
        );

        assert!(rendered.contains("[`duckduckgo.zip`](/api/v1/files/duckduckgo.zip)"));
        assert!(!rendered.contains("- File: `duckduckgo.zip`"));
    }

    #[test]
    fn does_not_linkify_inside_fenced_code_blocks() {
        let markdown = "Before `duckduckgo.zip`\n\n```text\n`duckduckgo.zip`\n```\n";
        let rendered = linkify_delivered_files_in_markdown(
            markdown,
            &[delivered_file(
                "duckduckgo.zip",
                "/api/v1/files/duckduckgo.zip",
            )],
        );

        assert!(rendered.contains("Before [`duckduckgo.zip`](/api/v1/files/duckduckgo.zip)"));
        assert!(rendered.contains("```text\n`duckduckgo.zip`\n```"));
    }

    #[test]
    fn missing_profile_option_keeps_persisted_selection_visible_before_profiles_load() {
        assert_eq!(
            missing_profile_option_label(&[], "sre-agent"),
            Some("Current profile · sre-agent".to_string())
        );
        assert_eq!(missing_profile_option_label(&[], PROFILE_VALUE_NONE), None);
        assert_eq!(
            missing_profile_option_label(&[], PROFILE_VALUE_DEFAULT),
            None
        );
    }
}
