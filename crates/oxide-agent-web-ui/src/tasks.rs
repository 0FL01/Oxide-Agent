use crate::auth::use_auth;
use crate::components::{EmptyState, ErrorBanner, StatusBadge};
use crate::markdown::MarkdownContent;
use crate::routes::AppRoute;
use crate::sessions::RenameSessionForm;
use crate::sse::{spawn_task_stream, TaskStreamConfig};
use crate::utils::{friendly_time, spawn_ui};
use leptos::prelude::*;
use oxide_agent_web_contracts::{
    CreateTaskRequest, EditTaskInputRequest, ErrorCode, PersistedTaskEvent, ProgressSnapshot,
    ResumeTaskRequest, SseConnectionState, TaskDetail, TaskStatus, TaskSummary,
};

#[component]
pub fn TaskConsole(route: AppRoute) -> impl IntoView {
    match route {
        AppRoute::Session(session_id) => {
            view! { <SessionWorkspace session_id=session_id /> }.into_any()
        }
        _ => view! {
            <section class="console-empty">
                <EmptyState title="Select or create a session" />
            </section>
        }
        .into_any(),
    }
}

#[component]
fn SessionWorkspace(session_id: String) -> impl IntoView {
    let auth = use_auth();
    let (session_title, set_session_title) = signal("Session".to_string());
    let (tasks, set_tasks) = signal(Vec::<TaskSummary>::new());
    let (events, set_events) = signal(Vec::<PersistedTaskEvent>::new());
    let (input, set_input) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (loading, set_loading) = signal(false);
    let (active_task, set_active_task) = signal(None::<TaskDetail>);
    let (progress, set_progress) = signal(None::<ProgressSnapshot>);
    let (sse_state, set_sse_state) = signal(SseConnectionState::Disconnected);
    let (streaming_task_id, set_streaming_task_id) = signal(None::<String>);
    let (loaded, set_loaded) = signal(false);

    let session_id_for_load = session_id.clone();
    let load_all = move || {
        set_loading.set(true);
        set_error.set(None);
        let session_id = session_id_for_load.clone();
        spawn_ui(async move {
            let client = auth.client();
            match client.get_session(&session_id).await {
                Ok(response) => set_session_title.set(response.session.title),
                Err(error) => set_error.set(Some(error.to_string())),
            }
            match client.list_tasks(&session_id).await {
                Ok(response) => {
                    let latest_running = response
                        .tasks
                        .iter()
                        .rev()
                        .find(|task| task.status.is_active())
                        .map(|task| task.task_id.clone());
                    set_tasks.set(response.tasks);
                    if let Some(task_id) = latest_running {
                        if let Ok(response) = client.get_task(&session_id, &task_id).await {
                            set_progress.set(response.task.last_progress.clone());
                            set_active_task.set(Some(response.task));
                            start_task_stream(
                                client,
                                session_id,
                                task_id,
                                StreamUiSignals {
                                    set_events,
                                    set_progress,
                                    set_sse_state,
                                    set_error,
                                    streaming_task_id,
                                    set_streaming_task_id,
                                },
                            );
                        }
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
    let submit_task = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let text = input.get();
        if text.trim().is_empty() {
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        let session_id = session_id_for_submit.clone();
        spawn_ui(async move {
            let client = auth.client();
            let result = match active_task.get() {
                Some(task) if task.status == TaskStatus::WaitingForUserInput => client
                    .resume_task(
                        &session_id,
                        &task.task_id,
                        &ResumeTaskRequest {
                            input_markdown: text,
                        },
                    )
                    .await
                    .map(|response| response.task),
                _ => client
                    .create_task(
                        &session_id,
                        &CreateTaskRequest {
                            input_markdown: text,
                        },
                    )
                    .await
                    .map(|response| response.task),
            };

            match result {
                Ok(task) => {
                    set_input.set(String::new());
                    set_active_task.set(Some(summary_to_detail(&session_id, &task)));
                    start_task_stream(
                        client,
                        session_id.clone(),
                        task.task_id.clone(),
                        StreamUiSignals {
                            set_events,
                            set_progress,
                            set_sse_state,
                            set_error,
                            streaming_task_id,
                            set_streaming_task_id,
                        },
                    );
                    set_tasks.update(|items| items.push(task));
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
            match auth.client().cancel_task(&session_id, &task.task_id).await {
                Ok(_) => {
                    let task_id = task.task_id.clone();
                    set_active_task.set(None);
                    set_tasks.update(|items| {
                        for item in items {
                            if item.task_id == task_id {
                                item.status = TaskStatus::Cancelled;
                            }
                        }
                    });
                }
                Err(error) => set_error.set(Some(error.to_string())),
            }
            set_loading.set(false);
        });
    };

    let session_id_for_events = session_id.clone();
    let refresh_events = move |_| {
        let Some(task) = latest_task(tasks.get()) else {
            return;
        };
        set_error.set(None);
        let session_id = session_id_for_events.clone();
        spawn_ui(async move {
            match auth
                .client()
                .task_events(&session_id, &task.task_id, 0)
                .await
            {
                Ok(response) => set_events.set(response.events),
                Err(error) => set_error.set(Some(error.to_string())),
            }
        });
    };
    let session_id_for_cards = session_id.clone();

    view! {
        <section class="session-workspace">
            <header class="session-header">
                <div>
                    <h1>{move || session_title.get()}</h1>
                    <p>{session_id.clone()}</p>
                </div>
                <RenameSessionForm session_id=session_id.clone() current_title=session_title.get_untracked() />
            </header>
            <ErrorBanner message=error />
            <div class="console-grid">
                <section class="transcript-panel">
                    {move || {
                        if loading.get() && tasks.get().is_empty() {
                            view! { <div class="loading">"Loading"</div> }.into_any()
                        } else if tasks.get().is_empty() {
                            view! { <EmptyState title="No task yet" /> }.into_any()
                        } else {
                            let latest_editable_task_id = latest_terminal_task_id(&tasks.get());
                            let session_id_for_cards = session_id_for_cards.clone();
                            view! {
                                <div class="task-list">
                                    <For
                                        each=move || tasks.get()
                                        key=|task| task.task_id.clone()
                                        children=move |task| {
                                            let editable =
                                                latest_editable_task_id.as_ref() == Some(&task.task_id);
                                            view! {
                                                <TaskCard
                                                    session_id=session_id_for_cards.clone()
                                                    task=task
                                                    editable=editable
                                                    set_tasks=set_tasks
                                                    set_error=set_error
                                                />
                                            }
                                        }
                                    />
                                </div>
                            }.into_any()
                        }
                    }}
                    <form class="composer" on:submit=submit_task>
                        <ComposerNotice active_task=active_task />
                        <textarea
                            placeholder="Markdown input"
                            prop:value=input
                            disabled=move || composer_is_blocked(active_task.get().as_ref())
                            on:input=move |ev| set_input.set(event_target_value(&ev))
                        />
                        <div class="composer-actions">
                            <button
                                type="submit"
                                disabled=move || loading.get() || composer_is_blocked(active_task.get().as_ref())
                            >
                                {move || if active_task.get().is_some_and(|task| task.status == TaskStatus::WaitingForUserInput) {
                                    "Resume"
                                } else {
                                    "Send"
                                }}
                            </button>
                            <button
                                class="secondary"
                                type="button"
                                disabled=move || active_task.get().is_none()
                                on:click=cancel_active
                            >
                                "Stop"
                            </button>
                        </div>
                    </form>
                </section>
                <aside class="events-panel">
                    <div class="panel-header">
                        <h2>"Events"</h2>
                        <button class="secondary" type="button" on:click=refresh_events>"Refresh"</button>
                    </div>
                    <SseStatus state=sse_state />
                    <ProgressPanel progress=progress />
                    {move || {
                        if events.get().is_empty() {
                            view! { <EmptyState title="No events" /> }.into_any()
                        } else {
                            view! {
                                <ol class="event-list">
                                    <For
                                        each=move || events.get()
                                        key=|event| event.seq
                                        children=move |event| view! { <EventRow event=event /> }
                                    />
                                </ol>
                            }.into_any()
                        }
                    }}
                </aside>
            </div>
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
            Some(TaskStatus::Queued | TaskStatus::Running) => view! {
                <p class="composer-notice busy">"This session is busy. Stop the active task before starting another one."</p>
            }.into_any(),
            _ => ().into_any(),
        }}
    }
}

fn composer_is_blocked(task: Option<&TaskDetail>) -> bool {
    task.is_some_and(|task| matches!(task.status, TaskStatus::Queued | TaskStatus::Running))
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

#[derive(Clone, Copy)]
struct StreamUiSignals {
    set_events: WriteSignal<Vec<PersistedTaskEvent>>,
    set_progress: WriteSignal<Option<ProgressSnapshot>>,
    set_sse_state: WriteSignal<SseConnectionState>,
    set_error: WriteSignal<Option<String>>,
    streaming_task_id: ReadSignal<Option<String>>,
    set_streaming_task_id: WriteSignal<Option<String>>,
}

fn start_task_stream(
    client: crate::api::ApiClient,
    session_id: String,
    task_id: String,
    signals: StreamUiSignals,
) {
    if signals.streaming_task_id.get_untracked().as_deref() == Some(task_id.as_str()) {
        return;
    }
    signals.set_streaming_task_id.set(Some(task_id.clone()));
    spawn_task_stream(TaskStreamConfig {
        client,
        session_id,
        task_id,
        set_events: signals.set_events,
        set_progress: signals.set_progress,
        set_state: signals.set_sse_state,
        set_error: signals.set_error,
    });
}

fn summary_to_detail(session_id: &str, task: &TaskSummary) -> TaskDetail {
    TaskDetail {
        task_id: task.task_id.clone(),
        session_id: session_id.to_string(),
        status: task.status,
        input_markdown: task.input_markdown.clone(),
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

#[component]
fn SseStatus(state: ReadSignal<SseConnectionState>) -> impl IntoView {
    view! {
        <div class="sse-state">
            {move || match state.get() {
                SseConnectionState::Connected => "stream connected",
                SseConnectionState::Disconnected => "stream disconnected",
                SseConnectionState::Reconnecting => "stream reconnecting",
                SseConnectionState::TerminalClosed => "stream closed",
            }}
        </div>
    }
}

#[component]
fn ProgressPanel(progress: ReadSignal<Option<ProgressSnapshot>>) -> impl IntoView {
    view! {
        {move || progress.get().map(|progress| view! {
            <section class="progress-panel">
                <div>
                    <strong>"Progress"</strong>
                    <span>{format!("{} / {}", progress.current_iteration, progress.max_iterations)}</span>
                </div>
                {progress.current_thought.map(|thought| view! {
                    <p>{thought}</p>
                })}
                {progress.provider_failover_notice.map(|notice| view! {
                    <p class="notice">{notice}</p>
                })}
                {progress.error.map(|error| view! {
                    <p class="error-text">{error}</p>
                })}
            </section>
        })}
    }
}

#[component]
fn TaskCard(
    session_id: String,
    task: TaskSummary,
    editable: bool,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let task_id = task.task_id.clone();
    let (editing, set_editing) = signal(false);
    let (draft, set_draft) = signal(task.input_markdown.clone());
    let (saving, set_saving) = signal(false);
    let original_input = task.input_markdown.clone();

    view! {
        <article class="task-card">
            <div class="task-card-header">
                <StatusBadge status=task.status />
                <time>{friendly_time(task.updated_at)}</time>
            </div>
            <div class="message user-message">
                {move || if editing.get() {
                    let session_id = session_id.clone();
                    let task_id = task_id.clone();
                    let original_input = original_input.clone();
                    view! {
                        <TaskInputEditForm
                            session_id=session_id
                            task_id=task_id
                            original_input=original_input
                            draft=draft
                            set_draft=set_draft
                            saving=saving
                            set_saving=set_saving
                            set_editing=set_editing
                            set_tasks=set_tasks
                            set_error=set_error
                        />
                    }.into_any()
                } else {
                    view! {
                        <MarkdownContent markdown=task.input_markdown.clone() />
                    }.into_any()
                }}
            </div>
            {editable.then(|| view! {
                <button
                    class="secondary edit-input-button"
                    type="button"
                    on:click=move |_| set_editing.set(true)
                >
                    "Edit input"
                </button>
            })}
            {task.final_response_markdown.map(|answer| view! {
                <div class="message assistant-message">
                    <MarkdownContent markdown=answer />
                </div>
            })}
            {task.error_message.map(|error| view! {
                <div class="message error-message">{error}</div>
            })}
            {task.pending_user_input.map(|pending| view! {
                <div class="message pending-message">{pending.prompt}</div>
            })}
        </article>
    }
}

#[allow(clippy::too_many_arguments)]
#[component]
fn TaskInputEditForm(
    session_id: String,
    task_id: String,
    original_input: String,
    draft: ReadSignal<String>,
    set_draft: WriteSignal<String>,
    saving: ReadSignal<bool>,
    set_saving: WriteSignal<bool>,
    set_editing: WriteSignal<bool>,
    set_tasks: WriteSignal<Vec<TaskSummary>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let auth = use_auth();
    let submit_edit = {
        let session_id = session_id.clone();
        let task_id = task_id.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            set_saving.set(true);
            set_error.set(None);
            let request = EditTaskInputRequest {
                input_markdown: draft.get(),
            };
            let session_id = session_id.clone();
            let task_id = task_id.clone();
            spawn_ui(async move {
                match auth
                    .client()
                    .edit_task_input(&session_id, &task_id, &request)
                    .await
                {
                    Ok(response) => {
                        set_tasks.update(|items| {
                            if let Some(item) =
                                items.iter_mut().find(|item| item.task_id == task_id)
                            {
                                *item = response.task;
                            }
                        });
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

#[component]
fn EventRow(event: PersistedTaskEvent) -> impl IntoView {
    view! {
        <li class="event-row">
            <span class="event-seq">{event.seq}</span>
            <span class="event-kind">{format!("{:?}", event.kind)}</span>
            <span class="event-summary">{event.summary}</span>
        </li>
    }
}

fn latest_task(tasks: Vec<TaskSummary>) -> Option<TaskSummary> {
    tasks.into_iter().max_by_key(|task| task.updated_at)
}

fn latest_terminal_task_id(tasks: &[TaskSummary]) -> Option<String> {
    tasks
        .iter()
        .filter(|task| task.status.is_terminal())
        .max_by_key(|task| task.updated_at)
        .map(|task| task.task_id.clone())
}
